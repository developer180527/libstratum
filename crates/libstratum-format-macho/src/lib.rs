//! Mach-O container plugin: macOS and iOS, thin and universal binaries.

use std::sync::Arc;

use libstratum_core::SpiError;
use libstratum_core::host::ByteSource;
use libstratum_core::spi::{BinaryFormat, Image, OpenOptions, ProbeResult};

const MH_MAGIC: u32 = 0xfeed_face;
const MH_MAGIC_64: u32 = 0xfeed_facf;
const FAT_MAGIC: u32 = 0xcafe_babe;
const FAT_MAGIC_64: u32 = 0xcafe_babf;
/// Java class files share `0xcafebabe`; their "count" field is a version >= 45.
const MAX_FAT_ARCHS: u32 = 45;

#[derive(Debug, Clone, Copy, Default)]
pub struct MachO;

impl BinaryFormat for MachO {
    fn id(&self) -> &'static str {
        "macho"
    }

    fn probe(&self, header: &[u8]) -> ProbeResult {
        let Some(magic) = header.get(..4) else { return ProbeResult::No };
        let magic: [u8; 4] = magic.try_into().expect("slice of length 4");
        let (le, be) = (u32::from_le_bytes(magic), u32::from_be_bytes(magic));

        if matches!(le, MH_MAGIC | MH_MAGIC_64) || matches!(be, MH_MAGIC | MH_MAGIC_64) {
            return ProbeResult::Yes;
        }
        if matches!(be, FAT_MAGIC | FAT_MAGIC_64) {
            let count = header.get(4..8).map(|b| u32::from_be_bytes(b.try_into().expect("slice of length 4")));
            if count.is_some_and(|n| n > 0 && n < MAX_FAT_ARCHS) {
                return ProbeResult::YesContainer;
            }
        }
        ProbeResult::No
    }

    fn open(&self, _source: Arc<dyn ByteSource>, _options: &OpenOptions) -> Result<Box<dyn Image>, SpiError> {
        // M1: slice selection, load commands, LC_UUID, nlist symbols, debug map
        // (N_OSO objects) and dSYM locations.
        Err(SpiError::Unimplemented("Mach-O image (M1)"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_thin_64_little_endian() {
        assert_eq!(MachO.probe(&[0xcf, 0xfa, 0xed, 0xfe, 0, 0, 0, 0]), ProbeResult::Yes);
    }

    #[test]
    fn probe_universal_but_not_java_class() {
        assert_eq!(MachO.probe(&[0xca, 0xfe, 0xba, 0xbe, 0, 0, 0, 2]), ProbeResult::YesContainer);
        // Java class file: 0xcafebabe, minor 0, major 65.
        assert_eq!(MachO.probe(&[0xca, 0xfe, 0xba, 0xbe, 0, 0, 0, 65]), ProbeResult::No);
    }

    #[test]
    fn probe_rejects_other_formats() {
        assert_eq!(MachO.probe(b"\x7fELF"), ProbeResult::No);
        assert_eq!(MachO.probe(b"MZ"), ProbeResult::No);
    }
}
