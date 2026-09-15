//! Format-neutral intermediate representation (docs/02 §5, docs/11 §4).
//!
//! Container plugins produce the image half; debug-info backends produce the
//! debug half. Correlation and lenses consume only these types. No parser
//! types (`object`, `gimli`, PDB readers) may appear here.

use std::path::PathBuf;

pub use libstratum_model::{Arch, BinaryId, SourceLoc};

/// Lexically normalizes a recorded source path so the same file compares equal across toolchains
/// and hosts: `\` becomes `/`, `.` segments are dropped and `dir/..` pairs collapse. Absolute
/// prefixes (`/`, `C:`) are kept; nothing touches the filesystem.
pub fn normalize_source_path(path: &str) -> String {
    let unified = path.replace('\\', "/");
    let absolute = unified.starts_with('/');
    let mut parts: Vec<&str> = Vec::new();
    for part in unified.split('/') {
        match part {
            "" | "." => {}
            ".." if parts.last().is_some_and(|p| *p != ".." && !p.ends_with(':')) => {
                parts.pop();
            }
            _ => parts.push(part),
        }
    }
    let joined = parts.join("/");
    if absolute { format!("/{joined}") } else { joined }
}

// ---------------------------------------------------------------------------
// Image
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Endian {
    Little,
    Big,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AddrRange {
    pub start: u64,
    pub size: u64,
}

impl AddrRange {
    pub fn end(&self) -> u64 {
        self.start.saturating_add(self.size)
    }

    pub fn contains(&self, addr: u64) -> bool {
        addr >= self.start && addr < self.end()
    }
}

/// Where something lives in each address space (ADR-0010, ADR-0016).
/// `vm`: run address. `file`: offset on disk. `load`: load address (LMA; flash on MCUs).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Extent {
    pub vm: Option<AddrRange>,
    pub file: Option<AddrRange>,
    pub load: Option<AddrRange>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SectionId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SymbolId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum SectionKind {
    Code,
    ReadOnlyData,
    Data,
    ZeroInit,
    Tls,
    Unwind,
    Debug,
    Symtab,
    Relocs,
    Metadata,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    /// `None` for ELF program headers (unnamed).
    pub name: Option<String>,
    pub extent: Extent,
    /// Format-specific flags, preserved raw.
    pub flags: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    pub id: SectionId,
    /// Mach-O segment name (`__TEXT`); `None` for ELF and PE.
    pub segment: Option<String>,
    pub name: String,
    pub kind: SectionKind,
    pub extent: Extent,
    pub flags: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum SymbolKind {
    Function,
    Object,
    Tls,
    Section,
    Label,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Binding {
    Global,
    Local,
    Weak,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Symbol {
    pub id: SymbolId,
    /// Raw (mangled) name. Demangling happens lazily via `Demangler`.
    pub raw_name: String,
    /// Normalized address: Thumb bit cleared (docs/10 §2.1).
    pub address: u64,
    /// `None` when the format carries no size (Mach-O nlist).
    pub size: Option<u64>,
    pub kind: SymbolKind,
    pub binding: Binding,
    pub section: Option<SectionId>,
    /// Arm: symbol value had bit 0 set.
    pub is_thumb: bool,
}

/// A named memory region such as FLASH or RAM (ADR-0016).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryRegion {
    pub name: String,
    pub origin: u64,
    pub length: u64,
    /// Raw attribute string, e.g. `rx`, `rwx`.
    pub attrs: String,
}

// ---------------------------------------------------------------------------
// Debug info locations (produced by container plugins)
// ---------------------------------------------------------------------------

/// Where debug info for an image may live. A container plugin lists candidates
/// in priority order; a debug-info backend accepts the ones it understands.
/// Fallback locations are only consulted when no primary location could be read.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum DebugLocation {
    /// Debug sections inside the image itself (ELF `.debug_*`, PE with DWARF).
    Embedded,
    /// Mach-O `.dSYM` bundle for the image's `LC_UUID`. The host locator finds the bundle
    /// (next to the binary, explicit path, or a symbol store) and must check the UUID.
    Dsym { uuid: [u8; 16] },
    /// Mach-O debug-map object (stabs `N_OSO`), possibly `lib.a(member.o)`.
    MachOObject { path: PathBuf, mtime: u64 },
    /// Split DWARF unit (`-gsplit-dwarf`).
    SplitDwo { name: String, comp_dir: Option<PathBuf>, dwo_id: u64 },
    /// DWARF package file.
    Dwp { path: PathBuf },
    /// ELF `.gnu_debuglink`.
    DebugLink { name: String, crc32: u32 },
    /// ELF build-id directory lookup.
    BuildId { id: Vec<u8> },
    /// PE CodeView `RSDS` record.
    Pdb { path: PathBuf, guid: [u8; 16], age: u32 },
}

impl DebugLocation {
    /// Alternatives to the primary debug info rather than supplements to it: Mach-O debug-map
    /// objects stand in for a missing dSYM, debuglink/build-id files for stripped debug sections.
    pub fn is_fallback(&self) -> bool {
        matches!(self, Self::MachOObject { .. } | Self::DebugLink { .. } | Self::BuildId { .. })
    }
}

// ---------------------------------------------------------------------------
// Debug IR (produced by debug-info backends; ADR-0018)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct UnitId(pub u32);

/// Opaque, global reference into a backend's data (DWARF DIE offset, PDB type
/// index or symbol offset). Global so references can cross units (GCC LTO).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DebugRef {
    pub unit: Option<UnitId>,
    pub key: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum SourceLanguage {
    C,
    Cpp,
    Rust,
    Swift,
    ObjC,
    ObjCpp,
    Asm,
    Other(u32),
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Producer {
    pub raw: String,
}

/// A compile unit (DWARF) or module (PDB DBI).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnitInfo {
    pub id: UnitId,
    pub name: String,
    pub comp_dir: Option<String>,
    pub language: SourceLanguage,
    pub producer: Option<Producer>,
    pub ranges: Vec<AddrRange>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AggregateKind {
    Struct,
    Class,
    Union,
}

/// Raw layout facts as recorded by the compiler. Derived facts (holes,
/// padding, cache lines, reorder suggestions) are computed by the core once
/// for every backend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawLayout {
    pub name: String,
    pub kind: AggregateKind,
    pub byte_size: u64,
    /// Explicit alignment if the debug info records one.
    pub alignment: Option<u64>,
    pub decl: Option<SourceLoc>,
    pub is_declaration: bool,
    /// The type or one of its bases inherits virtually: virtual base subobjects occupy bytes that
    /// no direct member describes, so tail padding can't be derived from the members.
    pub has_virtual_bases: bool,
    pub entries: Vec<RawLayoutEntry>,
}

/// Offsets and sizes are in bits so bitfields are exact. `align_bytes` is the member type's
/// alignment when the backend can determine it (needed for reorder suggestions).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum RawLayoutEntry {
    Field {
        name: Option<String>,
        type_name: String,
        offset_bits: u64,
        size_bits: u64,
        align_bytes: Option<u64>,
    },
    Bitfield {
        name: Option<String>,
        type_name: String,
        offset_bits: u64,
        width_bits: u32,
    },
    /// `offset_bits` is `None` for virtual bases whose position depends on the most-derived type.
    Base {
        type_name: String,
        offset_bits: Option<u64>,
        size_bits: u64,
        is_virtual: bool,
    },
    VtablePtr {
        offset_bits: u64,
        size_bits: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionInfo {
    pub reference: DebugRef,
    pub name: String,
    pub linkage_name: Option<String>,
    pub ranges: Vec<AddrRange>,
    pub decl: Option<SourceLoc>,
}

/// Inlining tree for one concrete function (DWARF inlined subroutines, PDB `S_INLINESITE`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InlineNode {
    pub callee: DebugRef,
    pub call_site: Option<SourceLoc>,
    pub ranges: Vec<AddrRange>,
    pub children: Vec<InlineNode>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InlineTree {
    pub function: DebugRef,
    pub inlined: Vec<InlineNode>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineRow {
    pub address: u64,
    /// Index into [`LineTable::files`].
    pub file: u32,
    pub line: u32,
    pub column: Option<u32>,
    pub is_stmt: bool,
    pub end_sequence: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LineTable {
    pub files: Vec<String>,
    pub rows: Vec<LineRow>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum DiscardReason {
    /// ELF: relocation resolved to a tombstone value.
    Tombstone { value: u64 },
    /// Present in object debug info but absent from the linked image.
    NotInImage,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscardEvidence {
    pub function: DebugRef,
    pub name: String,
    pub reason: DiscardReason,
}

/// Parsed linker map (optional enrichment, ADR-0006).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LinkMap {
    pub input_files: Vec<String>,
    pub placed: Vec<MapSymbol>,
    pub discarded: Vec<MapSymbol>,
    pub regions: Vec<MemoryRegion>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MapSymbol {
    pub name: String,
    pub address: Option<u64>,
    pub size: u64,
    pub input_file: Option<u32>,
}

#[cfg(test)]
mod tests {
    use super::normalize_source_path;

    #[test]
    fn source_paths_normalize_across_toolchains() {
        assert_eq!(normalize_source_path("./layout/padding.cpp"), "layout/padding.cpp");
        assert_eq!(
            normalize_source_path("layout/odr_conflict/../odr_conflict/config.h"),
            "layout/odr_conflict/config.h"
        );
        assert_eq!(normalize_source_path(r"D:\a\repo\src\.\x.cpp"), "D:/a/repo/src/x.cpp");
        assert_eq!(normalize_source_path("/usr/include/../include/stdio.h"), "/usr/include/stdio.h");
        assert_eq!(normalize_source_path("../shared/x.h"), "../shared/x.h");
    }
}
