//! libstratum engine.
//!
//! Contains the plugin SPI ([`spi`]), the format-neutral IR ([`ir`]), host
//! services ([`host`]), and the [`Engine`]/[`Session`] API. Plugins (container
//! formats, debug-info backends, demanglers, languages) implement the SPI;
//! this crate never depends on them (docs/02-architecture.md, docs/11-platforms.md §4).

pub mod cancel;
pub mod engine;
pub mod error;
pub mod host;
pub mod ir;
pub mod layout;
pub mod spi;
mod summary;
mod symbols;

pub use cancel::CancelToken;
pub use engine::{Engine, EngineBuilder, Input, Session};
pub use error::{OpenError, QueryError, SpiError};
pub use libstratum_model as model;
