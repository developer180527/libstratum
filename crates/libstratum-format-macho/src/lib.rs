//! Mach-O container plugin: macOS and iOS, thin and universal binaries.
//!
//! Parsed once at open into the neutral IR. Section ids are the 1-based section ordinals used by
//! `nlist.n_sect`, so symbols map to sections directly. Debug-map stabs (`N_OSO`) become
//! `DebugLocation::MachOObject` entries (docs/04 §3).

use std::borrow::Cow;
use std::path::PathBuf;
use std::sync::Arc;

use libstratum_core::SpiError;
use libstratum_core::host::ByteSource;
use libstratum_core::ir::{
    AddrRange, Arch, BinaryId, Binding, DebugLocation, Endian, Extent, Section, SectionId, SectionKind, Segment,
    Symbol, SymbolId, SymbolKind,
};
use libstratum_core::spi::{BinaryFormat, Image, OpenOptions, ProbeResult};
use object::Endianness;
use object::macho;
use object::read::macho::{FatArch, LoadCommandVariant, MachHeader, MachOFile, Nlist, Section as _};

const MH_MAGIC: u32 = 0xfeed_face;
const MH_MAGIC_64: u32 = 0xfeed_facf;
const FAT_MAGIC: u32 = 0xcafe_babe;
const FAT_MAGIC_64: u32 = 0xcafe_babf;
/// Java class files share `0xcafebabe`; their "count" field is a version >= 45.
const MAX_FAT_ARCHS: u32 = 45;

#[derive(Debug, Clone, Copy, Default)]
pub struct MachO;

fn read_u32(bytes: &[u8], at: usize, big_endian: bool) -> Option<u32> {
    let b: [u8; 4] = bytes.get(at..at + 4)?.try_into().ok()?;
    Some(if big_endian { u32::from_be_bytes(b) } else { u32::from_le_bytes(b) })
}

impl BinaryFormat for MachO {
    fn id(&self) -> &'static str {
        "macho"
    }

    fn probe(&self, header: &[u8]) -> ProbeResult {
        let (Some(le), Some(be)) = (read_u32(header, 0, false), read_u32(header, 0, true)) else {
            return ProbeResult::No;
        };
        if matches!(le, MH_MAGIC | MH_MAGIC_64) || matches!(be, MH_MAGIC | MH_MAGIC_64) {
            return ProbeResult::Yes;
        }
        if matches!(be, FAT_MAGIC | FAT_MAGIC_64)
            && read_u32(header, 4, true).is_some_and(|n| n > 0 && n < MAX_FAT_ARCHS)
        {
            return ProbeResult::YesContainer;
        }
        ProbeResult::No
    }

    fn open(&self, source: Arc<dyn ByteSource>, options: &OpenOptions) -> Result<Box<dyn Image>, SpiError> {
        let data = source.bytes()?;
        let (base, slice) = match read_u32(data, 0, true) {
            Some(FAT_MAGIC) => select_slice(
                object::read::macho::MachOFatFile32::parse(data).map_err(malformed)?.arches(),
                data,
                options,
            )?,
            Some(FAT_MAGIC_64) => select_slice(
                object::read::macho::MachOFatFile64::parse(data).map_err(malformed)?.arches(),
                data,
                options,
            )?,
            _ => (0, data),
        };
        let image = match read_u32(slice, 0, false) {
            Some(MH_MAGIC_64) | None => parse::<macho::MachHeader64<Endianness>>(slice, base, source.clone())?,
            Some(MH_MAGIC) => parse::<macho::MachHeader32<Endianness>>(slice, base, source.clone())?,
            _ => match read_u32(slice, 0, true) {
                Some(MH_MAGIC) => parse::<macho::MachHeader32<Endianness>>(slice, base, source.clone())?,
                _ => parse::<macho::MachHeader64<Endianness>>(slice, base, source.clone())?,
            },
        };
        Ok(Box::new(image))
    }
}

fn map_cpu(cpu: macho::CpuType) -> Arch {
    match cpu {
        macho::CPU_TYPE_ARM64 => Arch::Aarch64,
        macho::CPU_TYPE_X86_64 => Arch::X86_64,
        macho::CPU_TYPE_ARM => Arch::Arm,
        other => Arch::Other(other.0),
    }
}

/// Picks one architecture slice from a universal binary: `options.arch`, else the host arch.
fn select_slice<'d, A: FatArch>(
    arches: &[A],
    data: &'d [u8],
    options: &OpenOptions,
) -> Result<(u64, &'d [u8]), SpiError> {
    let host = if cfg!(target_arch = "aarch64") { Arch::Aarch64 } else { Arch::X86_64 };
    let wanted = options.arch.unwrap_or(host);
    let chosen = arches.iter().find(|a| map_cpu(a.cputype()) == wanted).ok_or_else(|| {
        let available: Vec<String> = arches.iter().map(|a| format!("{:?}", map_cpu(a.cputype()))).collect();
        SpiError::Malformed(format!("universal binary has no {wanted:?} slice (available: {})", available.join(", ")))
    })?;
    let offset: u64 = chosen.offset().into();
    Ok((offset, chosen.data(data).map_err(malformed)?))
}

fn malformed(err: object::Error) -> SpiError {
    SpiError::Malformed(err.to_string())
}

fn name16(raw: &[u8; 16]) -> String {
    let end = raw.iter().position(|&b| b == 0).unwrap_or(16);
    String::from_utf8_lossy(&raw[..end]).into_owned()
}

/// A parsed Mach-O image (one architecture slice).
#[derive(Debug)]
pub struct MachOImage {
    source: Arc<dyn ByteSource>,
    /// Offset of this slice inside the source (non-zero for universal binaries).
    base: u64,
    arch: Arch,
    endian: Endian,
    address_size: u8,
    binary_id: BinaryId,
    segments: Vec<Segment>,
    sections: Vec<Section>,
    symbols: Vec<Symbol>,
    debug_locations: Vec<DebugLocation>,
}

fn parse<H: MachHeader<Endian = Endianness>>(
    data: &[u8],
    base: u64,
    source: Arc<dyn ByteSource>,
) -> Result<MachOImage, SpiError> {
    let file = MachOFile::<H, &[u8]>::parse(data).map_err(malformed)?;
    let endian = file.endian();
    let header = file.macho_header();
    let arch = map_cpu(header.cputype(endian));
    let address_size = if header.is_type_64() { 8 } else { 4 };

    let mut segments = Vec::new();
    let mut sections = Vec::new();
    let mut binary_id = BinaryId::None;
    let mut has_dwarf_segment = false;

    let mut commands = file.macho_load_commands().map_err(malformed)?;
    while let Some(command) = commands.next().map_err(malformed)? {
        match command.variant().map_err(malformed)? {
            LoadCommandVariant::Uuid(uuid) => binary_id = BinaryId::Uuid(uuid.uuid),
            LoadCommandVariant::Segment64(segment, section_data) => {
                push_segment(segment, section_data, endian, &mut segments, &mut sections, &mut has_dwarf_segment)?
            }
            LoadCommandVariant::Segment32(segment, section_data) => {
                push_segment(segment, section_data, endian, &mut segments, &mut sections, &mut has_dwarf_segment)?
            }
            _ => {}
        }
    }

    // Symbols and debug-map stabs.
    let symtab = file.macho_symbol_table();
    let strings = symtab.strings();
    let mut symbols = Vec::new();
    let mut debug_locations = Vec::new();
    if has_dwarf_segment {
        debug_locations.push(DebugLocation::Embedded);
    }
    if let BinaryId::Uuid(uuid) = binary_id {
        debug_locations.push(DebugLocation::Dsym { uuid });
    }
    for nlist in symtab.iter() {
        let flags = nlist.n_type();
        let Ok(name) = nlist.name(endian, strings) else { continue };
        if let Some(stab) = flags.stab() {
            // `n_strx == 0` is the string table's "no name" sentinel: offset 0 holds a single
            // space, so the name reads as " " rather than empty. LTO links emit one such N_OSO per
            // merged object; `nm` and `dsymutil` drop them, and so must we, or every query carries
            // a phantom "object not found" diagnostic per entry (docs/04 §3).
            if stab == macho::N_OSO && nlist.n_strx(endian) != 0 && !name.is_empty() {
                debug_locations.push(DebugLocation::MachOObject {
                    path: PathBuf::from(String::from_utf8_lossy(name).into_owned()),
                    mtime: nlist.n_value(endian).into(),
                });
            }
            continue;
        }
        let address: u64 = nlist.n_value(endian).into();
        let mut section = match flags.typ() {
            macho::N_SECT => Some(SectionId(u32::from(nlist.n_sect()))),
            macho::N_ABS => None,
            _ => continue, // undefined, indirect, prebound
        };
        // Linker-synthesized symbols such as `__mh_execute_header` name section 1 but point at the
        // Mach header at the start of `__TEXT`, outside any section.
        if let Some(vm) = section.and_then(|id| sections.iter().find(|s| s.id == id)).and_then(|s| s.extent.vm)
            && !(address >= vm.start && address <= vm.end())
        {
            section = None;
        }
        let raw_name = String::from_utf8_lossy(name).into_owned();
        if raw_name.is_empty() {
            continue;
        }
        let in_code = section
            .and_then(|id| sections.iter().find(|s: &&Section| s.id == id))
            .is_some_and(|s| s.kind == SectionKind::Code);
        let binding = if flags.is_ext() && !flags.is_pext() {
            if nlist.n_desc(endian).0 & macho::N_WEAK_DEF.0 != 0 { Binding::Weak } else { Binding::Global }
        } else {
            Binding::Local
        };
        symbols.push(Symbol {
            id: SymbolId(symbols.len() as u32),
            raw_name,
            address,
            size: None, // nlist carries no size (docs/05 §3)
            kind: if section.is_none() {
                SymbolKind::Label
            } else if in_code {
                SymbolKind::Function
            } else {
                SymbolKind::Object
            },
            binding,
            section,
            is_thumb: false,
        });
    }

    let endian = if header.is_big_endian() { Endian::Big } else { Endian::Little };
    Ok(MachOImage { source, base, arch, endian, address_size, binary_id, segments, sections, symbols, debug_locations })
}

fn push_segment<S: object::read::macho::Segment<Endian = Endianness>>(
    segment: &S,
    section_data: &[u8],
    endian: Endianness,
    segments: &mut Vec<Segment>,
    sections: &mut Vec<Section>,
    has_dwarf_segment: &mut bool,
) -> Result<(), SpiError> {
    let segname = name16(segment.segname());
    let vmaddr: u64 = segment.vmaddr(endian).into();
    let vmsize: u64 = segment.vmsize(endian).into();
    let fileoff: u64 = segment.fileoff(endian).into();
    let filesize: u64 = segment.filesize(endian).into();
    *has_dwarf_segment |= segname == "__DWARF";
    let is_linkedit = segname == "__LINKEDIT";
    segments.push(Segment {
        extent: Extent {
            vm: (vmsize > 0 && segname != "__DWARF").then_some(AddrRange { start: vmaddr, size: vmsize }),
            file: (filesize > 0).then_some(AddrRange { start: fileoff, size: filesize }),
            // Mach-O loads at the run address; stored bytes = file-backed part of the mapping.
            load: (filesize > 0 && vmsize > 0 && !is_linkedit && segname != "__DWARF")
                .then_some(AddrRange { start: vmaddr, size: filesize.min(vmsize) }),
        },
        flags: u64::from(segment.initprot(endian).0),
        name: Some(segname.clone()),
    });

    for raw in segment.sections(endian, section_data).map_err(malformed)? {
        let sectname = name16(raw.sectname());
        let flags = raw.flags(endian);
        let typ = flags.typ();
        let addr: u64 = raw.addr(endian).into();
        let size: u64 = raw.size(endian).into();
        let offset = u64::from(raw.offset(endian));
        let zerofill = matches!(typ, macho::S_ZEROFILL | macho::S_GB_ZEROFILL | macho::S_THREAD_LOCAL_ZEROFILL);
        let debug = segname == "__DWARF";
        let kind = if debug {
            SectionKind::Debug
        } else if matches!(
            typ,
            macho::S_THREAD_LOCAL_REGULAR | macho::S_THREAD_LOCAL_ZEROFILL | macho::S_THREAD_LOCAL_VARIABLES
        ) {
            SectionKind::Tls
        } else if matches!(sectname.as_str(), "__unwind_info" | "__eh_frame") {
            SectionKind::Unwind
        } else if flags.0 & (macho::S_ATTR_PURE_INSTRUCTIONS.0 | macho::S_ATTR_SOME_INSTRUCTIONS.0) != 0 {
            SectionKind::Code
        } else if zerofill {
            SectionKind::ZeroInit
        } else if segname == "__TEXT" || segname == "__DATA_CONST" {
            SectionKind::ReadOnlyData
        } else {
            SectionKind::Data
        };
        let file = (!zerofill && offset > 0 && size > 0).then_some(AddrRange { start: offset, size });
        sections.push(Section {
            id: SectionId(sections.len() as u32 + 1),
            segment: Some(segname.clone()),
            name: sectname,
            kind,
            extent: Extent {
                vm: (!debug).then_some(AddrRange { start: addr, size }),
                file,
                load: (!debug && file.is_some()).then_some(AddrRange { start: addr, size }),
            },
            flags: u64::from(flags.0),
        });
    }
    Ok(())
}

impl Image for MachOImage {
    fn format_id(&self) -> &'static str {
        "macho"
    }

    fn arch(&self) -> Arch {
        self.arch
    }

    fn endian(&self) -> Endian {
        self.endian
    }

    fn address_size(&self) -> u8 {
        self.address_size
    }

    fn binary_id(&self) -> BinaryId {
        self.binary_id.clone()
    }

    fn segments(&self) -> &[Segment] {
        &self.segments
    }

    fn sections(&self) -> &[Section] {
        &self.sections
    }

    fn symbols(&self) -> &[Symbol] {
        &self.symbols
    }

    fn section_data(&self, id: SectionId) -> Result<Cow<'_, [u8]>, SpiError> {
        let section =
            self.sections.iter().find(|s| s.id == id).ok_or(SpiError::Malformed(format!("no section {id:?}")))?;
        let Some(file) = section.extent.file else { return Ok(Cow::Borrowed(&[])) };
        let bytes = self.source.bytes()?;
        let overflow = || SpiError::Malformed("section range overflow".into());
        let start = usize::try_from(self.base.checked_add(file.start).ok_or_else(overflow)?).map_err(|_| overflow())?;
        let end = start.checked_add(usize::try_from(file.size).map_err(|_| overflow())?).ok_or_else(overflow)?;
        bytes
            .get(start..end)
            .map(Cow::Borrowed)
            .ok_or(SpiError::Malformed(format!("section {} out of bounds", section.name)))
    }

    fn debug_locations(&self) -> Vec<DebugLocation> {
        self.debug_locations.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_thin_64_little_endian() {
        assert_eq!(MachO.probe(&[0xcf, 0xfa, 0xed, 0xfe, 0, 0, 0, 0]), ProbeResult::Yes);
    }

    #[test]
    fn probe_universal_but_not_java_class() {
        assert_eq!(MachO.probe(&[0xca, 0xfe, 0xba, 0xbe, 0, 0, 0, 2]), ProbeResult::YesContainer);
        assert_eq!(MachO.probe(&[0xca, 0xfe, 0xba, 0xbe, 0, 0, 0, 65]), ProbeResult::No);
    }

    #[test]
    fn probe_rejects_other_formats() {
        assert_eq!(MachO.probe(b"\x7fELF"), ProbeResult::No);
        assert_eq!(MachO.probe(b"MZ"), ProbeResult::No);
    }
}
