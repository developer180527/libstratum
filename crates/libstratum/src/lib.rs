//! libstratum: a low-level observability library for native binaries.
//!
//! This facade is what embedders depend on. It wires the default plugin set
//! (behind Cargo features) and adds convenience I/O. See `docs/` for the design.
//!
//! ```no_run
//! let engine = libstratum::default_engine();
//! let session = libstratum::open_path(&engine, "build/game.exe").unwrap();
//! println!("{:?}", session.info().identity);
//! ```

use std::path::Path;

pub use libstratum_core::spi::OpenOptions;
pub use libstratum_core::{CancelToken, Engine, EngineBuilder, Input, OpenError, QueryError, Session, host, ir, spi};
pub use libstratum_model as model;

/// Builder pre-populated with every plugin enabled by Cargo features.
pub fn default_builder() -> EngineBuilder {
    #[allow(unused_mut)]
    let mut builder = Engine::builder();
    #[cfg(feature = "elf")]
    {
        builder = builder.format(libstratum_format_elf::Elf);
    }
    #[cfg(feature = "macho")]
    {
        builder = builder.format(libstratum_format_macho::MachO);
    }
    #[cfg(feature = "pe")]
    {
        builder = builder.format(libstratum_format_pe::Pe);
    }
    #[cfg(feature = "dwarf")]
    {
        builder = builder.debug_backend(libstratum_debug_dwarf::Dwarf);
    }
    #[cfg(feature = "pdb")]
    {
        builder = builder.debug_backend(libstratum_debug_pdb::Pdb);
    }
    #[cfg(feature = "cpp")]
    {
        builder = builder
            .language(libstratum_lang_cpp::CCpp)
            .demangler(libstratum_demangle::Itanium)
            .demangler(libstratum_demangle::Msvc);
    }
    builder
}

/// Engine with the default plugin set.
pub fn default_engine() -> Engine {
    default_builder().build()
}

/// Reads a binary from disk and opens it.
pub fn open_path(engine: &Engine, path: impl AsRef<Path>) -> Result<Session, OpenError> {
    // TODO(M1): memory-map instead of reading, and install a filesystem locator
    // for dSYM / .o / .dwo / PDB companions relative to `path`.
    let bytes = std::fs::read(path).map_err(|e| OpenError::Read(libstratum_core::SpiError::Io(e.to_string())))?;
    engine.open(Input::Bytes(bytes), &OpenOptions::default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_engine_registers_all_formats() {
        let ids = default_engine().format_ids();
        assert_eq!(ids, ["elf", "macho", "pe"]);
    }

    #[test]
    fn truncated_elf_is_a_format_error() {
        let engine = default_engine();
        let err = engine.open(Input::Bytes(b"\x7fELF\x02\x01\x01\x00".to_vec()), &OpenOptions::default()).unwrap_err();
        assert!(matches!(err, OpenError::Format { format: "elf", .. }), "{err:?}");
    }
}
