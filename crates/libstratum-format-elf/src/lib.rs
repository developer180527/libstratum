//! ELF container plugin: Linux, Android, and bare-metal firmware (Cortex-M, RV32).
//!
//! The image is parsed once at open into the neutral IR; `object` types never leave this crate.
//! Section ids are ELF section header indices, so symbol `st_shndx` values map directly.

use std::borrow::Cow;
use std::sync::Arc;

use libstratum_arch::{is_mapping_symbol, split_thumb_bit};
use libstratum_core::SpiError;
use libstratum_core::host::ByteSource;
use libstratum_core::ir::{
    AddrRange, Arch, BinaryId, Binding, DebugLocation, Endian, Extent, Section, SectionId, SectionKind, Segment,
    Symbol, SymbolId, SymbolKind,
};
use libstratum_core::spi::{BinaryFormat, Image, OpenOptions, ProbeResult};
use object::elf;
use object::read::elf::{ElfFile, FileHeader, ProgramHeader, SectionHeader, Sym};
use object::{Endianness, Object, ObjectSection};

const ELF_MAGIC: &[u8; 4] = b"\x7fELF";
const EI_CLASS: usize = 4;
const ELFCLASS64: u8 = 2;

#[derive(Debug, Clone, Copy, Default)]
pub struct Elf;

impl BinaryFormat for Elf {
    fn id(&self) -> &'static str {
        "elf"
    }

    fn probe(&self, header: &[u8]) -> ProbeResult {
        if header.starts_with(ELF_MAGIC) { ProbeResult::Yes } else { ProbeResult::No }
    }

    fn open(&self, source: Arc<dyn ByteSource>, _options: &OpenOptions) -> Result<Box<dyn Image>, SpiError> {
        let data = source.bytes()?;
        let image = if data.get(EI_CLASS) == Some(&ELFCLASS64) {
            parse::<elf::FileHeader64<Endianness>>(data, source.clone())?
        } else {
            parse::<elf::FileHeader32<Endianness>>(data, source.clone())?
        };
        Ok(Box::new(image))
    }
}

/// A parsed ELF image. Holds the source so section data can be read lazily.
#[derive(Debug)]
pub struct ElfImage {
    source: Arc<dyn ByteSource>,
    arch: Arch,
    endian: Endian,
    address_size: u8,
    binary_id: BinaryId,
    segments: Vec<Segment>,
    sections: Vec<Section>,
    symbols: Vec<Symbol>,
    debug_locations: Vec<DebugLocation>,
}

fn malformed(err: object::Error) -> SpiError {
    SpiError::Malformed(err.to_string())
}

fn range(start: u64, size: u64) -> Option<AddrRange> {
    Some(AddrRange { start, size })
}

fn parse<H: FileHeader<Endian = Endianness>>(data: &[u8], source: Arc<dyn ByteSource>) -> Result<ElfImage, SpiError> {
    let file = ElfFile::<H>::parse(data).map_err(malformed)?;
    let endian = file.endian();
    let header = file.elf_header();
    let address_size = if header.is_class_64() { 8 } else { 4 };
    let arch = map_arch(header.e_machine(endian), address_size);

    // Segments. Load extent = initialized bytes stored at the load address (p_paddr, p_filesz);
    // for MCUs that's flash, for hosted images it equals the file-backed part of the mapping.
    let segments: Vec<Segment> = file
        .elf_program_headers()
        .iter()
        .map(|ph| {
            let vaddr: u64 = ph.p_vaddr(endian).into();
            let paddr: u64 = ph.p_paddr(endian).into();
            let memsz: u64 = ph.p_memsz(endian).into();
            let filesz: u64 = ph.p_filesz(endian).into();
            let offset: u64 = ph.p_offset(endian).into();
            let loadable = ph.p_type(endian) == elf::PT_LOAD;
            Segment {
                name: None,
                extent: Extent {
                    vm: loadable.then_some(AddrRange { start: vaddr, size: memsz }),
                    file: (filesz > 0).then_some(AddrRange { start: offset, size: filesz }),
                    load: (loadable && filesz > 0).then_some(AddrRange { start: paddr, size: filesz }),
                },
                flags: (u64::from(ph.p_type(endian).0) << 32) | u64::from(ph.p_flags(endian).0),
            }
        })
        .collect();
    let load_segments: Vec<(u64, u64, u64)> = file
        .elf_program_headers()
        .iter()
        .filter(|ph| ph.p_type(endian) == elf::PT_LOAD)
        .map(|ph| (ph.p_vaddr(endian).into(), ph.p_memsz(endian).into(), ph.p_paddr(endian).into()))
        .collect();

    // Sections (index 0 is the null section).
    let section_table = file.elf_section_table();
    let mut sections = Vec::new();
    for (index, sh) in section_table.iter().enumerate().skip(1) {
        let name =
            section_table.section_name(endian, sh).map(|n| String::from_utf8_lossy(n).into_owned()).unwrap_or_default();
        let sh_type = sh.sh_type(endian);
        let flags = sh.sh_flags(endian);
        let addr: u64 = sh.sh_addr(endian).into();
        let size: u64 = sh.sh_size(endian).into();
        let offset: u64 = sh.sh_offset(endian).into();
        let alloc = flags.contains(elf::SHF_ALLOC);
        let nobits = sh_type == elf::SHT_NOBITS;

        let load = if alloc && !nobits && size > 0 {
            load_segments
                .iter()
                .find(|(vaddr, memsz, _)| addr >= *vaddr && addr + size <= vaddr + memsz)
                .map(|(vaddr, _, paddr)| AddrRange { start: paddr + (addr - vaddr), size })
        } else {
            None
        };
        sections.push(Section {
            id: SectionId(index as u32),
            segment: None,
            kind: classify_section(&name, sh_type, flags),
            extent: Extent {
                vm: if alloc { range(addr, size) } else { None },
                file: if nobits || sh_type == elf::SHT_NULL { None } else { range(offset, size) },
                load,
            },
            name,
            flags: flags.0,
        });
    }

    // Symbols: static symtab if present, else the dynamic one.
    let symtab =
        if file.elf_symbol_table().is_empty() { file.elf_dynamic_symbol_table() } else { file.elf_symbol_table() };
    let strings = symtab.strings();
    let mut symbols = Vec::new();
    for (index, sym) in symtab.symbols().iter().enumerate().skip(1) {
        let st_type = sym.st_type();
        let shndx = sym.st_shndx(endian);
        if shndx == elf::SHN_UNDEF || st_type == elf::STT_FILE {
            continue;
        }
        let Ok(raw_name) = sym.name(endian, strings) else { continue };
        let raw_name = String::from_utf8_lossy(raw_name).into_owned();
        if raw_name.is_empty() && st_type != elf::STT_SECTION {
            continue;
        }
        if is_mapping_symbol(arch, &raw_name) {
            continue;
        }
        let kind = match st_type {
            elf::STT_FUNC | elf::STT_GNU_IFUNC => SymbolKind::Function,
            elf::STT_OBJECT | elf::STT_COMMON => SymbolKind::Object,
            elf::STT_TLS => SymbolKind::Tls,
            elf::STT_SECTION => SymbolKind::Section,
            elf::STT_NOTYPE => SymbolKind::Label,
            _ => SymbolKind::Unknown,
        };
        let (address, is_thumb) = split_thumb_bit(arch, sym.st_value(endian).into(), kind == SymbolKind::Function);
        let binding = match sym.st_bind() {
            elf::STB_LOCAL => Binding::Local,
            elf::STB_WEAK => Binding::Weak,
            _ => Binding::Global,
        };
        let section_index = if shndx == elf::SHN_XINDEX {
            symtab.shndx(endian, object::SymbolIndex(index))
        } else if shndx.0 < elf::SHN_LORESERVE {
            Some(u32::from(shndx.0))
        } else {
            None // SHN_ABS, SHN_COMMON
        };
        let mut section = section_index.filter(|&i| (i as usize) < section_table.len()).map(SectionId);
        // Linker-script symbols defined outside any output section (e.g. `__stack_top = ORIGIN(RAM) +
        // LENGTH(RAM)`) are attached by GNU ld to an arbitrary section. A label whose address lies outside
        // its section is really absolute.
        if kind == SymbolKind::Label {
            let inside = section
                .and_then(|id| sections.iter().find(|s| s.id == id))
                .and_then(|s| s.extent.vm)
                .is_none_or(|vm| address >= vm.start && address <= vm.end());
            if !inside {
                section = None;
            }
        }
        symbols.push(Symbol {
            id: SymbolId(symbols.len() as u32),
            raw_name,
            address,
            size: Some(sym.st_size(endian).into()),
            kind,
            binding,
            section,
            is_thumb,
        });
    }

    let binary_id = match file.build_id().map_err(malformed)? {
        Some(id) => BinaryId::BuildId(id.to_vec()),
        None => BinaryId::None,
    };

    let mut debug_locations = Vec::new();
    if sections.iter().any(|s| s.name == ".debug_info" || s.name == ".zdebug_info") {
        debug_locations.push(DebugLocation::Embedded);
    }
    if let Some((name, crc32)) = file.gnu_debuglink().map_err(malformed)? {
        debug_locations.push(DebugLocation::DebugLink { name: String::from_utf8_lossy(name).into_owned(), crc32 });
    }
    if let BinaryId::BuildId(id) = &binary_id {
        debug_locations.push(DebugLocation::BuildId { id: id.clone() });
    }

    let endian = match endian {
        Endianness::Little => Endian::Little,
        Endianness::Big => Endian::Big,
    };
    Ok(ElfImage { source, arch, endian, address_size, binary_id, segments, sections, symbols, debug_locations })
}

fn map_arch(machine: elf::Machine, address_size: u8) -> Arch {
    match machine {
        elf::EM_X86_64 => Arch::X86_64,
        elf::EM_AARCH64 => Arch::Aarch64,
        elf::EM_ARM => Arch::Arm,
        elf::EM_RISCV if address_size == 4 => Arch::Riscv32,
        other => Arch::Other(u32::from(other.0)),
    }
}

fn classify_section(name: &str, sh_type: elf::SectionType, flags: elf::SectionFlags) -> SectionKind {
    if name.starts_with(".debug") || name.starts_with(".zdebug") {
        return SectionKind::Debug;
    }
    if matches!(name, ".eh_frame" | ".eh_frame_hdr" | ".gcc_except_table") || name.starts_with(".ARM.ex") {
        return SectionKind::Unwind;
    }
    match sh_type {
        elf::SHT_SYMTAB | elf::SHT_DYNSYM | elf::SHT_STRTAB | elf::SHT_SYMTAB_SHNDX => return SectionKind::Symtab,
        elf::SHT_REL | elf::SHT_RELA | elf::SHT_RELR => return SectionKind::Relocs,
        _ => {}
    }
    if !flags.contains(elf::SHF_ALLOC) {
        return SectionKind::Metadata;
    }
    if flags.contains(elf::SHF_TLS) {
        SectionKind::Tls
    } else if flags.contains(elf::SHF_EXECINSTR) {
        SectionKind::Code
    } else if sh_type == elf::SHT_NOBITS {
        SectionKind::ZeroInit
    } else if flags.contains(elf::SHF_WRITE) {
        SectionKind::Data
    } else {
        SectionKind::ReadOnlyData
    }
}

impl Image for ElfImage {
    fn format_id(&self) -> &'static str {
        "elf"
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
        let data = self.source.bytes()?;
        let file = object::File::parse(data).map_err(malformed)?;
        let section = file.section_by_index(object::SectionIndex(id.0 as usize)).map_err(malformed)?;
        section.uncompressed_data().map_err(malformed)
    }

    fn debug_locations(&self) -> Vec<DebugLocation> {
        self.debug_locations.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe() {
        assert_eq!(Elf.probe(b"\x7fELF\x02\x01\x01"), ProbeResult::Yes);
        assert_eq!(Elf.probe(b"MZ\x90\x00"), ProbeResult::No);
        assert_eq!(Elf.probe(b""), ProbeResult::No);
    }

    #[test]
    fn truncated_input_is_an_error_not_a_panic() {
        let source = Arc::new(libstratum_core::host::InMemorySource(b"\x7fELF\x02\x01\x01\x00".to_vec()));
        assert!(Elf.open(source, &OpenOptions::default()).is_err());
    }
}
