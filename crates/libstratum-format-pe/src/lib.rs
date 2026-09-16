//! PE/COFF container plugin: Windows executables and DLLs (x64, arm64).
//!
//! Parsed once at open into the neutral IR. Addresses are absolute virtual addresses
//! (image base + RVA). Linked MSVC images carry no COFF symbol table; symbols come from the
//! PDB backend. Identity is the CodeView `RSDS` record (GUID + age), which also names the PDB.

use std::borrow::Cow;
use std::path::PathBuf;
use std::sync::Arc;

use libstratum_core::SpiError;
use libstratum_core::host::ByteSource;
use libstratum_core::ir::{
    Access, AddrRange, Arch, BinaryId, DebugLocation, Endian, Extent, Section, SectionId, SectionKind, Segment, Symbol,
};
use libstratum_core::spi::{BinaryFormat, Image, OpenOptions, ProbeResult};
use object::pe;
use object::read::pe::{ImageNtHeaders, ImageOptionalHeader, PeFile};
use object::{LittleEndian as LE, Object, ObjectSection};

const E_LFANEW_OFFSET: usize = 0x3c;
const PE_SIGNATURE: &[u8; 4] = b"PE\0\0";
/// Offset of the optional header magic from the PE signature (4-byte signature + 20-byte file header).
const OPTIONAL_MAGIC_OFFSET: usize = 24;

#[derive(Debug, Clone, Copy, Default)]
pub struct Pe;

fn pe_header_offset(header: &[u8]) -> Option<usize> {
    if !header.starts_with(b"MZ") {
        return None;
    }
    let lfanew = u32::from_le_bytes(header.get(E_LFANEW_OFFSET..E_LFANEW_OFFSET + 4)?.try_into().ok()?) as usize;
    (header.get(lfanew..lfanew.checked_add(4)?)? == PE_SIGNATURE).then_some(lfanew)
}

impl BinaryFormat for Pe {
    fn id(&self) -> &'static str {
        "pe"
    }

    fn probe(&self, header: &[u8]) -> ProbeResult {
        // TODO(Tier 2): ARM64X (ARM64 + ARM64EC in one image) → YesContainer via CHPE metadata (docs/11 §2.1).
        if pe_header_offset(header).is_some() { ProbeResult::Yes } else { ProbeResult::No }
    }

    fn open(&self, source: Arc<dyn ByteSource>, _options: &OpenOptions) -> Result<Box<dyn Image>, SpiError> {
        let data = source.bytes()?;
        let offset = pe_header_offset(data).ok_or_else(|| SpiError::Malformed("missing PE signature".into()))?;
        let magic = data
            .get(offset.saturating_add(OPTIONAL_MAGIC_OFFSET)..offset.saturating_add(OPTIONAL_MAGIC_OFFSET + 2))
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
            .ok_or_else(|| SpiError::Malformed("truncated PE optional header".into()))?;
        let image = match magic {
            pe::IMAGE_NT_OPTIONAL_HDR64_MAGIC => parse::<pe::ImageNtHeaders64>(data, source.clone())?,
            pe::IMAGE_NT_OPTIONAL_HDR32_MAGIC => parse::<pe::ImageNtHeaders32>(data, source.clone())?,
            other => return Err(SpiError::Malformed(format!("unknown PE optional header magic {other:#x}"))),
        };
        Ok(Box::new(image))
    }
}

fn malformed(err: object::Error) -> SpiError {
    SpiError::Malformed(err.to_string())
}

/// A parsed PE image.
#[derive(Debug)]
pub struct PeImage {
    source: Arc<dyn ByteSource>,
    arch: Arch,
    address_size: u8,
    image_base: u64,
    binary_id: BinaryId,
    headers_size: u64,
    segments: Vec<Segment>,
    sections: Vec<Section>,
    debug_locations: Vec<DebugLocation>,
}

fn parse<H: ImageNtHeaders>(data: &[u8], source: Arc<dyn ByteSource>) -> Result<PeImage, SpiError> {
    let file = PeFile::<H, &[u8]>::parse(data).map_err(malformed)?;
    let headers = file.nt_headers();
    let machine = headers.file_header().machine.get(LE);
    let arch = match machine {
        pe::IMAGE_FILE_MACHINE_AMD64 => Arch::X86_64,
        pe::IMAGE_FILE_MACHINE_ARM64 => Arch::Aarch64,
        pe::IMAGE_FILE_MACHINE_ARMNT => Arch::Arm,
        other => Arch::Other(u32::from(other.0)),
    };
    let address_size = if headers.is_type_64() { 8 } else { 4 };
    let image_base = headers.optional_header().image_base();
    let size_of_image = u64::from(headers.optional_header().size_of_image());
    let size_of_headers = u64::from(headers.optional_header().size_of_headers());
    let read_only = Access { read: true, write: false, execute: false };
    // PE has no segments; model the loader's view: one mapping of `SizeOfImage` bytes, and the
    // headers, which are stored in the file and mapped at the image base.
    let segments = vec![
        Segment {
            name: None,
            extent: Extent { vm: Some(AddrRange { start: image_base, size: size_of_image }), file: None, load: None },
            access: read_only,
            flags: 0,
        },
        Segment {
            name: None,
            extent: Extent {
                vm: Some(AddrRange { start: image_base, size: size_of_headers.min(size_of_image) }),
                file: Some(AddrRange { start: 0, size: size_of_headers }),
                load: Some(AddrRange { start: image_base, size: size_of_headers.min(size_of_image) }),
            },
            access: read_only,
            flags: 0,
        },
    ];

    let mut sections = Vec::new();
    for section in file.sections() {
        let header = section.pe_section();
        let name = section.name().unwrap_or_default().to_owned();
        let rva = u64::from(header.virtual_address.get(LE));
        let raw_size = u64::from(header.size_of_raw_data.get(LE));
        let raw_ptr = u64::from(header.pointer_to_raw_data.get(LE));
        let virtual_size = match u64::from(header.virtual_size.get(LE)) {
            0 => raw_size,
            size => size,
        };
        let flags = header.characteristics.get(LE);
        let file_extent = (raw_size > 0 && raw_ptr > 0).then_some(AddrRange { start: raw_ptr, size: raw_size });
        let va = image_base
            .checked_add(rva)
            .ok_or_else(|| SpiError::Malformed(format!("section {name} address overflows (base {image_base:#x})")))?;
        sections.push(Section {
            id: SectionId(section.index().0 as u32),
            segment: None,
            kind: classify_section(&name, flags),
            extent: Extent {
                vm: Some(AddrRange { start: va, size: virtual_size }),
                file: file_extent,
                // Initialized bytes stored for the mapping: the raw data, capped by the virtual size
                // (raw data is padded to FileAlignment).
                load: file_extent.map(|_| AddrRange { start: va, size: raw_size.min(virtual_size) }),
            },
            access: Access {
                read: flags.contains(pe::IMAGE_SCN_MEM_READ),
                write: flags.contains(pe::IMAGE_SCN_MEM_WRITE),
                execute: flags.contains(pe::IMAGE_SCN_MEM_EXECUTE),
            },
            name,
            flags: u64::from(flags.0),
        });
    }

    let mut debug_locations = Vec::new();
    if sections.iter().any(|s| s.name == ".debug_info") {
        debug_locations.push(DebugLocation::Embedded); // MinGW / llvm-mingw DWARF
    }
    let binary_id = match file.pdb_info().map_err(malformed)? {
        Some(codeview) => {
            let guid = codeview.guid();
            let age = codeview.age();
            debug_locations.push(DebugLocation::Pdb {
                path: PathBuf::from(String::from_utf8_lossy(codeview.path()).into_owned()),
                guid,
                age,
            });
            BinaryId::PdbGuidAge { guid, age }
        }
        None => BinaryId::None,
    };

    Ok(PeImage {
        source,
        arch,
        address_size,
        image_base,
        binary_id,
        headers_size: size_of_headers,
        segments,
        sections,
        debug_locations,
    })
}

fn classify_section(name: &str, flags: pe::SectionFlags) -> SectionKind {
    if name.starts_with(".debug") {
        return SectionKind::Debug;
    }
    match name {
        ".pdata" | ".xdata" => return SectionKind::Unwind,
        ".reloc" => return SectionKind::Relocs,
        ".tls" => return SectionKind::Tls,
        _ => {}
    }
    if flags.intersects(pe::IMAGE_SCN_CNT_CODE | pe::IMAGE_SCN_MEM_EXECUTE) {
        SectionKind::Code
    } else if flags.contains(pe::IMAGE_SCN_CNT_UNINITIALIZED_DATA)
        && !flags.contains(pe::IMAGE_SCN_CNT_INITIALIZED_DATA)
    {
        SectionKind::ZeroInit
    } else if flags.contains(pe::IMAGE_SCN_MEM_WRITE) {
        SectionKind::Data
    } else if flags.contains(pe::IMAGE_SCN_MEM_READ) {
        SectionKind::ReadOnlyData
    } else {
        SectionKind::Other
    }
}

impl Image for PeImage {
    fn format_id(&self) -> &'static str {
        "pe"
    }

    fn arch(&self) -> Arch {
        self.arch
    }

    fn endian(&self) -> Endian {
        Endian::Little
    }

    fn address_size(&self) -> u8 {
        self.address_size
    }

    fn image_base(&self) -> u64 {
        self.image_base
    }

    fn binary_id(&self) -> BinaryId {
        self.binary_id.clone()
    }

    fn headers_size(&self) -> u64 {
        self.headers_size
    }

    fn segments(&self) -> &[Segment] {
        &self.segments
    }

    fn sections(&self) -> &[Section] {
        &self.sections
    }

    fn symbols(&self) -> &[Symbol] {
        &[]
    }

    fn section_data(&self, id: SectionId) -> Result<Cow<'_, [u8]>, SpiError> {
        let section =
            self.sections.iter().find(|s| s.id == id).ok_or(SpiError::Malformed(format!("no section {id:?}")))?;
        let Some(file) = section.extent.file else { return Ok(Cow::Borrowed(&[])) };
        let bytes = self.source.bytes()?;
        let start = usize::try_from(file.start).map_err(|_| SpiError::Malformed("section offset".into()))?;
        let end = usize::try_from(file.size)
            .ok()
            .and_then(|size| start.checked_add(size))
            .ok_or(SpiError::Malformed("section range overflow".into()))?;
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

    fn pe_header(lfanew: u32) -> Vec<u8> {
        let mut h = vec![0u8; 0x100];
        h[..2].copy_from_slice(b"MZ");
        h[E_LFANEW_OFFSET..E_LFANEW_OFFSET + 4].copy_from_slice(&lfanew.to_le_bytes());
        h[lfanew as usize..lfanew as usize + 4].copy_from_slice(PE_SIGNATURE);
        h
    }

    #[test]
    fn probe_pe() {
        assert_eq!(Pe.probe(&pe_header(0x80)), ProbeResult::Yes);
    }

    #[test]
    fn probe_rejects_dos_stub_and_truncated_input() {
        let mut dos = pe_header(0x80);
        dos[0x80..0x84].copy_from_slice(b"NE\0\0");
        assert_eq!(Pe.probe(&dos), ProbeResult::No);
        assert_eq!(Pe.probe(b"MZ"), ProbeResult::No);
        let mut far = vec![0u8; 0x40];
        far[..2].copy_from_slice(b"MZ");
        far[E_LFANEW_OFFSET..E_LFANEW_OFFSET + 4].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(Pe.probe(&far), ProbeResult::No);
    }
}
