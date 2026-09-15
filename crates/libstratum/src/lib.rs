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
use std::sync::Arc;

mod locator;
mod source;

pub use libstratum_core::spi::OpenOptions;
pub use libstratum_core::{
    CancelToken, Engine, EngineBuilder, Input, OpenError, QueryError, Session, SpiError, host, ir, spi,
};
pub use libstratum_model as model;
pub use locator::FsLocator;
pub use source::FileSource;
#[cfg(feature = "mmap")]
pub use source::MmapSource;

/// Builder pre-populated with every plugin enabled by Cargo features and a filesystem locator
/// that looks next to the binary. Replace the host services to add search paths or symbol stores.
pub fn default_builder() -> EngineBuilder {
    let mut builder =
        Engine::builder().host(host::HostServices { locator: Arc::new(FsLocator::default()), cache: None });
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

/// Reads a binary from disk into memory and opens it. The path is kept so companion files
/// (dSYM, `.o`, PDB, `.dwo`) can be located next to it.
pub fn open_path(engine: &Engine, path: impl AsRef<Path>) -> Result<Session, OpenError> {
    let path = path.as_ref();
    let source = FileSource::read(path).map_err(OpenError::Read)?;
    engine.open(Input::File { source: Arc::new(source), path: path.to_path_buf() }, &OpenOptions::default())
}

/// Like [`open_path`], but memory-maps the file (feature `mmap`; see ADR-0021 for the hazard).
#[cfg(feature = "mmap")]
pub fn open_path_mmap(engine: &Engine, path: impl AsRef<Path>) -> Result<Session, OpenError> {
    let path = path.as_ref();
    let source = MmapSource::open(path).map_err(OpenError::Read)?;
    engine.open(Input::File { source: Arc::new(source), path: path.to_path_buf() }, &OpenOptions::default())
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
