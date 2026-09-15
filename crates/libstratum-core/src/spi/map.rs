use std::fmt::Debug;

use crate::error::SpiError;
use crate::ir::LinkMap;

/// Parses one linker's map file format (optional enrichment, ADR-0006).
pub trait MapFileParser: Send + Sync + Debug + 'static {
    /// Stable id: "gnu-ld", "lld-elf", "ld64", "msvc-link".
    fn id(&self) -> &'static str;

    fn sniff(&self, head: &str) -> bool;

    fn parse(&self, text: &str) -> Result<LinkMap, SpiError>;
}
