//! DWARF debug-info backend (ELF, Mach-O, and PE built with MinGW).
//! `gimli` types stay inside this crate (ADR-0018).

use libstratum_core::SpiError;
use libstratum_core::host::DebugOpenContext;
use libstratum_core::ir::DebugLocation;
use libstratum_core::spi::{DebugInfoBackend, DebugReader, Image};

#[derive(Debug, Clone, Copy, Default)]
pub struct Dwarf;

impl DebugInfoBackend for Dwarf {
    fn id(&self) -> &'static str {
        "dwarf"
    }

    fn accepts(&self, location: &DebugLocation) -> bool {
        !matches!(location, DebugLocation::Pdb { .. })
    }

    fn open(
        &self,
        _location: &DebugLocation,
        _image: &dyn Image,
        _context: &DebugOpenContext<'_>,
    ) -> Result<Box<dyn DebugReader>, SpiError> {
        // M2: unit index and raw type layouts. M4: lines, inline trees, tombstones.
        Err(SpiError::Unimplemented("DWARF reader (M2)"))
    }
}
