//! Filesystem locator for debug-info companion files (docs/04 §2–3, docs/11 §2.4).
//!
//! It only finds candidate bytes. Identity (build id, UUID, GUID/age) is verified by the engine
//! against what the debug backend reads, except for `.gnu_debuglink`, whose CRC is checked here
//! because the CRC is the link's only identity.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use libstratum_core::SpiError;
use libstratum_core::host::{ByteSource, FileLocator, InMemorySource, LocateRequest};
use libstratum_core::ir::DebugLocation;

use crate::source::FileSource;

/// Finds companion files on the local filesystem. All search locations are explicit:
/// the library never reads environment variables or assumes system directories.
#[derive(Debug, Clone, Default)]
pub struct FsLocator {
    search_dirs: Vec<PathBuf>,
    debug_roots: Vec<PathBuf>,
    symbol_stores: Vec<PathBuf>,
    prefix_maps: Vec<(PathBuf, PathBuf)>,
}

impl FsLocator {
    /// Extra directories searched by file name (after the binary's own directory).
    pub fn search_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.search_dirs.push(dir.into());
        self
    }

    /// Roots holding `.build-id/xx/rest.debug` trees and debuglink targets (e.g. `/usr/lib/debug`).
    pub fn debug_root(mut self, dir: impl Into<PathBuf>) -> Self {
        self.debug_roots.push(dir.into());
        self
    }

    /// Local symbol stores laid out as `<store>/<name>/<GUID><AGE>/<name>` (Windows SymSrv layout).
    pub fn symbol_store(mut self, dir: impl Into<PathBuf>) -> Self {
        self.symbol_stores.push(dir.into());
        self
    }

    /// Rewrites recorded paths that start with `from` to start with `to` (build-machine paths).
    pub fn prefix_map(mut self, from: impl Into<PathBuf>, to: impl Into<PathBuf>) -> Self {
        self.prefix_maps.push((from.into(), to.into()));
        self
    }

    fn remap(&self, path: &Path) -> Vec<PathBuf> {
        let mut out: Vec<PathBuf> = self
            .prefix_maps
            .iter()
            .filter_map(|(from, to)| path.strip_prefix(from).ok().map(|rest| to.join(rest)))
            .collect();
        out.push(path.to_path_buf());
        out
    }

    /// Candidate paths for a recorded path: as recorded (after remapping), then by file name in the
    /// binary's directory and the search directories.
    fn candidates(&self, recorded: &Path, image_dir: Option<&Path>) -> Vec<PathBuf> {
        let mut out = Vec::new();
        if recorded.is_absolute() {
            out.extend(self.remap(recorded));
        } else if let Some(dir) = image_dir {
            out.push(dir.join(recorded));
        }
        if let Some(name) = file_name(recorded) {
            out.extend(image_dir.into_iter().map(|d| d.join(&name)));
            out.extend(self.search_dirs.iter().map(|d| d.join(&name)));
        }
        out
    }
}

/// File name of a recorded path, accepting both `/` and `\` separators: PDB paths recorded on
/// Windows must resolve on macOS and Linux hosts too.
fn file_name(path: &Path) -> Option<String> {
    let s = path.to_string_lossy();
    s.rsplit(['/', '\\']).next().filter(|n| !n.is_empty()).map(str::to_owned)
}

fn read_first(candidates: impl IntoIterator<Item = PathBuf>) -> Result<Option<Arc<dyn ByteSource>>, SpiError> {
    for candidate in candidates {
        if candidate.is_file() {
            return Ok(Some(Arc::new(FileSource::read(&candidate)?)));
        }
    }
    Ok(None)
}

impl FileLocator for FsLocator {
    fn locate(&self, request: &LocateRequest) -> Result<Option<Arc<dyn ByteSource>>, SpiError> {
        let image_dir = request.image_path.as_deref().and_then(Path::parent);
        let image_name = request.image_path.as_deref().and_then(Path::file_name);

        match &request.location {
            DebugLocation::Embedded => Ok(None),

            DebugLocation::Dsym { .. } => {
                let (Some(dir), Some(name)) = (image_dir, image_name) else { return Ok(None) };
                let bundle = |base: &Path| {
                    let mut bundle_name = name.to_os_string();
                    bundle_name.push(".dSYM");
                    base.join(bundle_name).join("Contents/Resources/DWARF").join(name)
                };
                read_first(std::iter::once(bundle(dir)).chain(self.search_dirs.iter().map(|d| bundle(d))))
            }

            DebugLocation::MachOObject { path, .. } => {
                // `lib.a(member.o)` names an archive member.
                let text = path.to_string_lossy();
                if let Some((archive, member)) = text.strip_suffix(')').and_then(|t| t.split_once('(')) {
                    for candidate in self.candidates(Path::new(archive), image_dir) {
                        if candidate.is_file() {
                            return archive_member(&candidate, member);
                        }
                    }
                    return Ok(None);
                }
                read_first(self.candidates(path, image_dir))
            }

            DebugLocation::Pdb { path, guid, age } => {
                let mut candidates = self.candidates(path, image_dir);
                if let Some(name) = file_name(path) {
                    let key = format!("{}{:X}", guid_hex(guid), age);
                    candidates.extend(self.symbol_stores.iter().map(|store| store.join(&name).join(&key).join(&name)));
                }
                read_first(candidates)
            }

            DebugLocation::SplitDwo { name, comp_dir, .. } => {
                let recorded = comp_dir.as_ref().map(|dir| dir.join(name)).unwrap_or_else(|| PathBuf::from(name));
                read_first(self.candidates(&recorded, image_dir))
            }

            DebugLocation::Dwp { path } => read_first(self.candidates(path, image_dir)),

            DebugLocation::DebugLink { name, crc32: expected_crc } => {
                let mut candidates = Vec::new();
                if let Some(dir) = image_dir {
                    candidates.push(dir.join(name));
                    candidates.push(dir.join(".debug").join(name));
                    for root in &self.debug_roots {
                        let relative = dir.strip_prefix("/").unwrap_or(dir);
                        candidates.push(root.join(relative).join(name));
                    }
                }
                candidates.extend(self.search_dirs.iter().map(|d| d.join(name)));
                for candidate in candidates {
                    if !candidate.is_file() {
                        continue;
                    }
                    let source = FileSource::read(&candidate)?;
                    if crc32(source.bytes()?) == *expected_crc {
                        return Ok(Some(Arc::new(source)));
                    }
                }
                Ok(None)
            }

            DebugLocation::BuildId { id } => {
                let Some((first, rest)) = id.split_first() else { return Ok(None) };
                let rest: String = rest.iter().map(|b| format!("{b:02x}")).collect();
                let relative = PathBuf::from(".build-id").join(format!("{first:02x}")).join(format!("{rest}.debug"));
                read_first(self.debug_roots.iter().map(|root| root.join(&relative)))
            }

            _ => Ok(None),
        }
    }
}

/// GUID as used in symbol-store directory names: Data1-3 as big-endian hex, then Data4 bytes.
/// The 16 bytes are the in-file (little-endian Data1-3) GUID layout from the CodeView record.
fn guid_hex(guid: &[u8; 16]) -> String {
    let d1 = u32::from_le_bytes(guid[0..4].try_into().expect("4 bytes"));
    let d2 = u16::from_le_bytes(guid[4..6].try_into().expect("2 bytes"));
    let d3 = u16::from_le_bytes(guid[6..8].try_into().expect("2 bytes"));
    let d4: String = guid[8..16].iter().map(|b| format!("{b:02X}")).collect();
    format!("{d1:08X}{d2:04X}{d3:04X}{d4}")
}

fn archive_member(archive: &Path, member: &str) -> Result<Option<Arc<dyn ByteSource>>, SpiError> {
    let bytes = std::fs::read(archive).map_err(|e| SpiError::Io(format!("{}: {e}", archive.display())))?;
    let file = object::read::archive::ArchiveFile::parse(&*bytes).map_err(|e| SpiError::Malformed(e.to_string()))?;
    for entry in file.members() {
        let entry = entry.map_err(|e| SpiError::Malformed(e.to_string()))?;
        if entry.name() == member.as_bytes() {
            let data = entry.data(&*bytes).map_err(|e| SpiError::Malformed(e.to_string()))?;
            return Ok(Some(Arc::new(InMemorySource(data.to_vec()))));
        }
    }
    Ok(None)
}

/// CRC-32 (IEEE, reflected), as used by `.gnu_debuglink`.
fn crc32(data: &[u8]) -> u32 {
    const TABLE: [u32; 256] = {
        let mut table = [0u32; 256];
        let mut i = 0;
        while i < 256 {
            let mut c = i as u32;
            let mut k = 0;
            while k < 8 {
                c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
                k += 1;
            }
            table[i] = c;
            i += 1;
        }
        table
    };
    !data.iter().fold(!0u32, |crc, &b| TABLE[((crc ^ u32::from(b)) & 0xff) as usize] ^ (crc >> 8))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc32_matches_reference() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b""), 0);
    }

    #[test]
    fn guid_formatting_matches_symsrv() {
        // {1B72224D-37B8-1792-2820-0ED8994498B2} stored little-endian in Data1-3.
        let guid = [0x4D, 0x22, 0x72, 0x1B, 0xB8, 0x37, 0x92, 0x17, 0x28, 0x20, 0x0E, 0xD8, 0x99, 0x44, 0x98, 0xB2];
        assert_eq!(guid_hex(&guid), "1B72224D37B8179228200ED8994498B2");
    }

    #[test]
    fn windows_paths_resolve_by_file_name() {
        assert_eq!(file_name(Path::new(r"D:\a\repo\out\game.pdb")).as_deref(), Some("game.pdb"));
        assert_eq!(file_name(Path::new("/build/out/game.pdb")).as_deref(), Some("game.pdb"));
    }
}
