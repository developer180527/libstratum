//! ELF container plugin: Linux, Android, and bare-metal firmware (Cortex-M, RV32).

use std::sync::Arc;

use libstratum_core::SpiError;
use libstratum_core::host::ByteSource;
use libstratum_core::spi::{BinaryFormat, Image, OpenOptions, ProbeResult};

const ELF_MAGIC: &[u8; 4] = b"\x7fELF";

#[derive(Debug, Clone, Copy, Default)]
pub struct Elf;

impl BinaryFormat for Elf {
    fn id(&self) -> &'static str {
        "elf"
    }

    fn probe(&self, header: &[u8]) -> ProbeResult {
        if header.starts_with(ELF_MAGIC) { ProbeResult::Yes } else { ProbeResult::No }
    }

    fn open(&self, _source: Arc<dyn ByteSource>, _options: &OpenOptions) -> Result<Box<dyn Image>, SpiError> {
        // M1: sections, segments (PT_LOAD p_vaddr/p_paddr), symbols with Thumb
        // normalization, build id, debug locations (embedded, .dwo, debuglink).
        Err(SpiError::Unimplemented("ELF image (M1)"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe() {
        assert_eq!(Elf.probe(b"\x7fELF\x02\x01\x01"), ProbeResult::Yes);
        assert_eq!(Elf.probe(b"MZ\x90\x00"), ProbeResult::No);
        assert_eq!(Elf.probe(b""), ProbeResult::No);
    }
}
