use std::borrow::Cow;
use std::fmt::Debug;
use std::sync::Arc;

use crate::error::SpiError;
use crate::host::ByteSource;
use crate::ir::{Arch, BinaryId, DebugLocation, Endian, MemoryRegion, Section, SectionId, Segment, Symbol};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeResult {
    No,
    Yes,
    /// A container of several images (Mach-O universal, PE ARM64X).
    YesContainer,
}

#[derive(Debug, Clone, Default)]
pub struct OpenOptions {
    /// Slice or view to select from a container image.
    pub arch: Option<Arch>,
}

/// Recognizes and opens one container format (ELF, Mach-O, PE, …).
pub trait BinaryFormat: Send + Sync + Debug + 'static {
    /// Stable id: "elf", "macho", "pe".
    fn id(&self) -> &'static str;

    /// Cheap magic-number check on the leading bytes. Must not panic on any input.
    fn probe(&self, header: &[u8]) -> ProbeResult;

    fn open(&self, source: Arc<dyn ByteSource>, options: &OpenOptions) -> Result<Box<dyn Image>, SpiError>;
}

/// The normalized view of one loaded binary.
pub trait Image: Send + Sync + Debug {
    fn format_id(&self) -> &'static str;
    fn arch(&self) -> Arch;
    fn endian(&self) -> Endian;
    /// Pointer size in bytes (4 or 8).
    fn address_size(&self) -> u8;
    /// Preferred load base. IR addresses are absolute (base included); PE/PDB data is
    /// relative to this base (RVA), so debug backends convert with it. 0 where not applicable.
    fn image_base(&self) -> u64 {
        0
    }
    fn binary_id(&self) -> BinaryId;
    fn segments(&self) -> &[Segment];
    fn sections(&self) -> &[Section];
    /// Container symbols. Empty for linked PE images (symbols come from the PDB).
    fn symbols(&self) -> &[Symbol];
    /// Memory regions derivable from the image alone (may be empty).
    fn memory_regions(&self) -> &[MemoryRegion] {
        &[]
    }
    /// Section contents, decompressed if needed.
    fn section_data(&self, id: SectionId) -> Result<Cow<'_, [u8]>, SpiError>;
    /// Candidate debug-info locations, in priority order.
    fn debug_locations(&self) -> Vec<DebugLocation>;
}
