//! PDB (CodeView) debug-info backend for PE images built by MSVC or clang-cl.
//! Pure Rust; no dependency on the Windows-only DIA SDK (docs/11 §2.2).

use libstratum_core::SpiError;
use libstratum_core::host::DebugOpenContext;
use libstratum_core::ir::DebugLocation;
use libstratum_core::spi::{DebugInfoBackend, DebugReader, Image};

#[derive(Debug, Clone, Copy, Default)]
pub struct Pdb;

impl DebugInfoBackend for Pdb {
    fn id(&self) -> &'static str {
        "pdb"
    }

    fn accepts(&self, location: &DebugLocation) -> bool {
        matches!(location, DebugLocation::Pdb { .. })
    }

    fn open(
        &self,
        _location: &DebugLocation,
        _image: &dyn Image,
        _context: &DebugOpenContext<'_>,
    ) -> Result<Box<dyn DebugReader>, SpiError> {
        // M2: locate PDB (RSDS path, host locator, symbol-store layout), check
        // GUID/age, TPI raw layouts, DBI modules. M4: C13 lines, S_INLINESITE.
        Err(SpiError::Unimplemented("PDB reader (M2)"))
    }
}
