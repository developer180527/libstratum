//! Services injected by the embedder (docs/02-architecture.md §4.6).
//! The core never chooses file locations, reads env vars, or opens network connections.

use std::fmt::Debug;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::error::SpiError;
use crate::ir::DebugLocation;
use crate::spi::{BinaryFormat, Image, OpenOptions, ProbeResult};

/// A readable blob: memory map, in-memory buffer, or virtual file.
pub trait ByteSource: Send + Sync + Debug {
    fn bytes(&self) -> Result<&[u8], SpiError>;
}

/// Bytes held in memory.
#[derive(Debug, Clone)]
pub struct InMemorySource(pub Vec<u8>);

impl ByteSource for InMemorySource {
    fn bytes(&self) -> Result<&[u8], SpiError> {
        Ok(&self.0)
    }
}

/// A request to find a companion file (dSYM, `.o`, `.dwo`, PDB, …).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocateRequest {
    /// Path of the primary binary, if it came from a file.
    pub image_path: Option<PathBuf>,
    pub location: DebugLocation,
}

/// Resolves companion files. Hosts can back this with a filesystem, a VFS, or a symbol store.
pub trait FileLocator: Send + Sync + Debug {
    fn locate(&self, request: &LocateRequest) -> Result<Option<Arc<dyn ByteSource>>, SpiError>;
}

/// A locator that finds nothing. Useful for in-memory analysis and tests.
#[derive(Debug, Clone, Copy, Default)]
pub struct NullLocator;

impl FileLocator for NullLocator {
    fn locate(&self, _request: &LocateRequest) -> Result<Option<Arc<dyn ByteSource>>, SpiError> {
        Ok(None)
    }
}

/// Opaque cache key derived from engine build, format, identity and content hashes.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CacheKey(pub String);

/// Optional persistent cache (ADR-0009). Absent means the library never writes to disk.
pub trait CacheStore: Send + Sync + Debug {
    fn get(&self, key: &CacheKey) -> Option<Arc<dyn ByteSource>>;
    fn put(&self, key: &CacheKey, bytes: &[u8]) -> Result<(), SpiError>;
}

#[derive(Debug, Clone)]
pub struct HostServices {
    pub locator: Arc<dyn FileLocator>,
    pub cache: Option<Arc<dyn CacheStore>>,
}

impl Default for HostServices {
    fn default() -> Self {
        Self { locator: Arc::new(NullLocator), cache: None }
    }
}

/// Context handed to debug-info backends when they open a location.
#[derive(Debug, Clone, Copy)]
pub struct DebugOpenContext<'a> {
    pub host: &'a HostServices,
    /// Path of the primary binary when it was opened from a file. Companion files
    /// (dSYM, `.o`, `.dwo`, PDB) are usually found relative to it.
    pub image_path: Option<&'a Path>,
    /// The engine's registered container formats, so backends can open companion files
    /// (a dSYM is a Mach-O, a `.dwo` an ELF) without depending on format plugins.
    pub formats: &'a [Arc<dyn BinaryFormat>],
}

impl DebugOpenContext<'_> {
    /// Asks the host locator for the bytes of `location`.
    pub fn locate(&self, location: &DebugLocation) -> Result<Option<Arc<dyn ByteSource>>, SpiError> {
        self.host
            .locator
            .locate(&LocateRequest { image_path: self.image_path.map(Path::to_path_buf), location: location.clone() })
    }

    /// Opens companion bytes with the first registered format that recognizes them.
    pub fn open_image(&self, source: Arc<dyn ByteSource>) -> Result<Box<dyn Image>, SpiError> {
        let bytes = source.bytes()?;
        let header = &bytes[..bytes.len().min(4096)];
        let format = self
            .formats
            .iter()
            .find(|f| f.probe(header) != ProbeResult::No)
            .ok_or_else(|| SpiError::Malformed("companion file has no recognized container format".into()))?;
        format.open(source.clone(), &OpenOptions::default())
    }
}
