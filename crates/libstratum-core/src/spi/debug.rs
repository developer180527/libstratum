use std::fmt::Debug;

use crate::error::SpiError;
use crate::host::DebugOpenContext;
use crate::ir::{
    AddrRange, BinaryId, DebugLocation, DiscardEvidence, FunctionInfo, InlineTree, LineTable, RawLayout, Symbol,
    UnitId, UnitInfo, VariableInfo,
};
use crate::spi::Image;

/// Turns one debug-info location into the neutral debug IR (ADR-0018).
/// Implementations: DWARF (gimli), PDB. Parser types never cross this boundary.
pub trait DebugInfoBackend: Send + Sync + Debug + 'static {
    /// Stable id: "dwarf", "pdb".
    fn id(&self) -> &'static str;

    fn accepts(&self, location: &DebugLocation) -> bool;

    fn open(
        &self,
        location: &DebugLocation,
        image: &dyn Image,
        context: &DebugOpenContext<'_>,
    ) -> Result<Box<dyn DebugReader>, SpiError>;
}

/// Lazy access to debug facts. The core memoizes results per session.
pub trait DebugReader: Send + Sync + Debug {
    /// Identity recorded in the debug info, checked against the image (ADR-0011).
    fn binary_id(&self) -> BinaryId;

    fn units(&self) -> Result<Vec<UnitInfo>, SpiError>;

    fn unit_for_address(&self, address: u64) -> Result<Option<UnitId>, SpiError>;

    // Layout lens
    /// Complete (non-declaration) aggregate definitions whose fully qualified name satisfies `matches`.
    /// Name normalization across toolchains is the core's job; backends report names as recorded.
    fn find_types(&self, matches: &dyn Fn(&str) -> bool) -> Result<Vec<RawLayout>, SpiError>;

    // Correlation lens
    fn functions(&self, unit: UnitId) -> Result<Vec<FunctionInfo>, SpiError>;
    fn inline_trees(&self, unit: UnitId) -> Result<Vec<InlineTree>, SpiError>;
    fn line_table(&self, unit: UnitId) -> Result<LineTable, SpiError>;
    fn discarded_functions(&self, unit: UnitId) -> Result<Vec<DiscardEvidence>, SpiError>;

    /// Statically allocated variables with final addresses and type sizes (sizes data symbols).
    fn variables(&self) -> Result<Vec<VariableInfo>, SpiError> {
        Err(SpiError::Unimplemented("variables"))
    }

    /// Symbols for formats whose image has none (PE). `None` if not applicable.
    fn symbols(&self) -> Result<Option<Vec<Symbol>>, SpiError> {
        Ok(None)
    }

    /// Address ranges that this reader covers, for diagnostics.
    fn covered_ranges(&self) -> Result<Vec<AddrRange>, SpiError> {
        Ok(Vec::new())
    }
}
