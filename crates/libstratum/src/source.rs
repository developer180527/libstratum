//! File-backed byte sources.

use std::path::Path;

use libstratum_core::SpiError;
use libstratum_core::host::ByteSource;

/// The whole file read into memory. The default: immune to the file changing on disk.
#[derive(Debug)]
pub struct FileSource {
    bytes: Vec<u8>,
}

impl FileSource {
    pub fn read(path: impl AsRef<Path>) -> Result<Self, SpiError> {
        let path = path.as_ref();
        std::fs::read(path).map(|bytes| Self { bytes }).map_err(|e| SpiError::Io(format!("{}: {e}", path.display())))
    }
}

impl ByteSource for FileSource {
    fn bytes(&self) -> Result<&[u8], SpiError> {
        Ok(&self.bytes)
    }
}

/// A read-only memory map of a file (feature `mmap`, ADR-0021).
#[cfg(feature = "mmap")]
#[derive(Debug)]
pub struct MmapSource {
    map: memmap2::Mmap,
}

#[cfg(feature = "mmap")]
impl MmapSource {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, SpiError> {
        let path = path.as_ref();
        let file = std::fs::File::open(path).map_err(|e| SpiError::Io(format!("{}: {e}", path.display())))?;
        // SAFETY: the mapping is read-only and never outlives `self`. The remaining hazard is external:
        // if another process truncates or rewrites the file while it is mapped, reads may fault (SIGBUS).
        // That's why this source is opt-in behind the `mmap` feature (ADR-0021).
        #[allow(unsafe_code)]
        let map = unsafe { memmap2::Mmap::map(&file) }.map_err(|e| SpiError::Io(format!("{}: {e}", path.display())))?;
        Ok(Self { map })
    }
}

#[cfg(feature = "mmap")]
impl ByteSource for MmapSource {
    fn bytes(&self) -> Result<&[u8], SpiError> {
        Ok(&self.map)
    }
}
