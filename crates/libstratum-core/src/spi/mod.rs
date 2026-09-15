//! Plugin SPI. These traits are the only contract plugins see (docs/02 §4, docs/11 §4.2).
//! Breaking them is a major version bump of `libstratum-core`.

mod artifact;
#[cfg(feature = "conformance")]
pub mod conformance;
mod debug;
mod demangle;
mod format;
mod lang;
mod map;

pub use artifact::{ArtifactFacts, ArtifactProvider, FactKind};
pub use debug::{DebugInfoBackend, DebugReader};
pub use demangle::{Demangler, NameStyle};
pub use format::{BinaryFormat, Image, OpenOptions, ProbeResult};
pub use lang::LanguageSupport;
pub use map::MapFileParser;
