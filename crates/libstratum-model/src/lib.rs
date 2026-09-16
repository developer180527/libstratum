//! Public, serializable types for libstratum.
//!
//! This crate is the contract every frontend serializes (docs/03-data-model.md).
//! It contains no logic. Evolution is additive: enums are `#[non_exhaustive]`,
//! and breaking changes bump [`SCHEMA_VERSION`].

use serde::{Deserialize, Serialize};

/// Version of the serialized model. Bumped only on breaking changes.
pub const SCHEMA_VERSION: u32 = 1;

/// A size measured in every address space (ADR-0010, ADR-0016).
///
/// `vm`: bytes occupied at run time. `file`: bytes on disk.
/// `load`: initialized bytes stored at the load address (flash on MCUs); zero-initialized
/// memory such as `.bss` has a `vm` size but no `load` size.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Size {
    pub vm: u64,
    pub file: u64,
    pub load: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum Arch {
    X86_64,
    Aarch64,
    /// 32-bit Arm; Cortex-M is Thumb-only.
    Arm,
    Riscv32,
    Other(u32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum ContainerFormat {
    Elf,
    MachO,
    Pe,
    Other,
}

/// How a binary is matched with its debug info (ADR-0011).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum BinaryId {
    /// ELF `NT_GNU_BUILD_ID`.
    BuildId(Vec<u8>),
    /// Mach-O `LC_UUID`.
    Uuid([u8; 16]),
    /// PE CodeView `RSDS` record: GUID + age.
    PdbGuidAge {
        guid: [u8; 16],
        age: u32,
    },
    None,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ImageIdentity {
    pub format: ContainerFormat,
    pub arch: Arch,
    pub id: BinaryId,
    /// Hex content hash of the image bytes.
    pub content_hash: String,
}

/// Stable section key: `(segment, name)`, e.g. `("__TEXT", "__text")` or `(None, ".text")`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SectionKey {
    pub segment: Option<String>,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SourceLoc {
    pub file: String,
    pub line: u32,
    pub column: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Severity {
    Info,
    Warning,
    Error,
}

/// Stable, machine-readable diagnostic codes (docs/03-data-model.md#diagnostics).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum DiagCode {
    NoDebugInfo,
    DebugInfoMismatch,
    OsoObjectMissing,
    OsoObjectStale,
    DwoMissing,
    PdbMissing,
    PdbFastlinkPartial,
    UnsupportedLanguage,
    DebugInfoParseError,
    DeclarationOnlyType,
    OdrConflict,
    AddressMapGap,
    LtoObject,
    MapParseError,
    MapImageMismatch,
    Unimplemented,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diagnostic {
    pub severity: Severity,
    pub code: DiagCode,
    pub message: String,
    pub subject: Option<String>,
}

/// Where a fact came from, so consumers can judge trust (docs/03-data-model.md#evidence).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum Evidence {
    DebugInfo { backend: String, unit: u64, key: Option<u64> },
    LineTable { backend: String, unit: u64 },
    SymbolTable,
    LinkMap { parser: String },
    DebugMap,
    Computed { rule: String },
    Heuristic { rule: String },
}

/// Basic facts about an opened binary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BinaryInfo {
    pub identity: ImageIdentity,
    pub sections: Vec<SectionInfo>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SectionInfo {
    pub key: SectionKey,
    pub vm_address: Option<u64>,
    pub load_address: Option<u64>,
    pub size: Size,
}

/// Availability of each lens for a session (ADR-0014).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capabilities {
    pub lenses: Vec<LensCapability>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LensCapability {
    pub lens: Lens,
    pub status: Availability,
    pub missing: Vec<MissingInput>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum Lens {
    Layout,
    Symbols,
    Correlation,
    MemoryRegions,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Availability {
    Available,
    Partial,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum MissingInput {
    RebuildWith(String),
    KeepObjectFiles,
    ProvideDebugFile,
    ProvideMapFile,
    NotYetImplemented,
}

// ---------------------------------------------------------------------------------------------
// Layout lens (docs/03-data-model.md#layout)
// ---------------------------------------------------------------------------------------------

/// Result of a `struct_layout` query. Several matches mean the name is ambiguous or the program
/// contains conflicting definitions (an ODR violation, reported as a diagnostic).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LayoutResult {
    pub query: String,
    pub matches: Vec<TypeLayout>,
    pub not_found_reason: Option<String>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AggregateKind {
    Struct,
    Class,
    Union,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TypeLayout {
    pub name: String,
    pub kind: AggregateKind,
    pub size_bytes: u64,
    /// Explicit alignment from debug info, else computed from members when possible.
    pub alignment_bytes: Option<u64>,
    pub decl: Option<SourceLoc>,
    pub members: Vec<LayoutMember>,
    pub holes: Vec<Hole>,
    pub tail_padding_bits: u64,
    /// Sum of holes and tail padding, in bits.
    pub padding_bits: u64,
    /// Members are not placed at multiples of their alignment (`#pragma pack` or similar).
    pub packed: bool,
    pub cacheline: CachelineView,
    pub suggestion: Option<ReorderSuggestion>,
    /// Limits of what could be derived, stated instead of guessed.
    pub notes: Vec<String>,
    pub evidence: Vec<Evidence>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MemberKind {
    Field,
    Bitfield,
    Base,
    VirtualBase,
    VtablePtr,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LayoutMember {
    pub kind: MemberKind,
    pub name: Option<String>,
    pub type_name: Option<String>,
    /// `None` for virtual bases, whose position depends on the most-derived type.
    pub offset_bits: Option<u64>,
    pub size_bits: u64,
    pub align_bytes: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hole {
    pub offset_bits: u64,
    pub size_bits: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CachelineView {
    pub cacheline_bytes: u64,
    /// Offsets (bytes) of cache-line boundaries inside the type.
    pub boundaries: Vec<u64>,
    /// Members whose bytes span a boundary.
    pub straddling: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReorderSuggestion {
    pub order: Vec<String>,
    pub new_size_bytes: u64,
    pub saved_bytes: u64,
    pub caveats: Vec<String>,
}

// ---------------------------------------------------------------------------------------------
// Symbols (docs/12-m3-symbols-sizes.md §2)
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum SymbolKind {
    Function,
    Object,
    Tls,
    Label,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Binding {
    Global,
    Local,
    Weak,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SymbolEntry {
    /// Name as recorded (mangled). PDB procedures without a public symbol carry their undecorated name.
    pub raw_name: String,
    pub demangled: Option<String>,
    pub kind: SymbolKind,
    pub binding: Binding,
    pub section: Option<SectionKey>,
    /// Absolute address; Thumb bit cleared.
    pub address: u64,
    pub size: Option<u64>,
    /// Where the size came from: symbol table, debug info, link map, or a labeled heuristic.
    pub size_evidence: Option<Evidence>,
    /// Other symbols with the identical (section, address, size).
    pub aliases: Vec<String>,
    /// The alias group contains distinct functions (identical code folding).
    pub folded: bool,
    /// Compile unit or PDB module that defined the symbol, when known.
    pub unit: Option<String>,
}

/// Which symbols to list. Empty filter = all symbols.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SymbolFilter {
    /// Exact match against the raw, demangled, or language-normalized demangled name.
    pub name: Option<String>,
    pub kind: Option<SymbolKind>,
    /// Section name, e.g. `.text` or `__text`.
    pub section: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SymbolResult {
    pub query: String,
    pub matches: Vec<SymbolEntry>,
    pub diagnostics: Vec<Diagnostic>,
}
