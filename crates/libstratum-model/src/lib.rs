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
