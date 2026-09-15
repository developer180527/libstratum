//! PE/COFF container plugin: Windows executables and DLLs (x64, arm64; ARM64X later).

use std::sync::Arc;

use libstratum_core::SpiError;
use libstratum_core::host::ByteSource;
use libstratum_core::spi::{BinaryFormat, Image, OpenOptions, ProbeResult};

const E_LFANEW_OFFSET: usize = 0x3c;
const PE_SIGNATURE: &[u8; 4] = b"PE\0\0";

#[derive(Debug, Clone, Copy, Default)]
pub struct Pe;

impl BinaryFormat for Pe {
    fn id(&self) -> &'static str {
        "pe"
    }

    fn probe(&self, header: &[u8]) -> ProbeResult {
        if !header.starts_with(b"MZ") {
            return ProbeResult::No;
        }
        let Some(lfanew) = header.get(E_LFANEW_OFFSET..E_LFANEW_OFFSET + 4) else { return ProbeResult::No };
        let lfanew = u32::from_le_bytes(lfanew.try_into().expect("slice of length 4")) as usize;
        match header.get(lfanew..lfanew.saturating_add(4)) {
            // TODO(M1): detect ARM64X via load config CHPE metadata → YesContainer.
            Some(sig) if sig == PE_SIGNATURE => ProbeResult::Yes,
            _ => ProbeResult::No,
        }
    }

    fn open(&self, _source: Arc<dyn ByteSource>, _options: &OpenOptions) -> Result<Box<dyn Image>, SpiError> {
        // M1: sections (VirtualSize vs SizeOfRawData), debug directory RSDS
        // (GUID + age + PDB path) → DebugLocation::Pdb. Symbols come from the PDB.
        Err(SpiError::Unimplemented("PE image (M1)"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pe_header(lfanew: u32) -> Vec<u8> {
        let mut h = vec![0u8; 0x100];
        h[..2].copy_from_slice(b"MZ");
        h[E_LFANEW_OFFSET..E_LFANEW_OFFSET + 4].copy_from_slice(&lfanew.to_le_bytes());
        h[lfanew as usize..lfanew as usize + 4].copy_from_slice(PE_SIGNATURE);
        h
    }

    #[test]
    fn probe_pe() {
        assert_eq!(Pe.probe(&pe_header(0x80)), ProbeResult::Yes);
    }

    #[test]
    fn probe_rejects_dos_stub_and_truncated_input() {
        let mut dos = pe_header(0x80);
        dos[0x80..0x84].copy_from_slice(b"NE\0\0");
        assert_eq!(Pe.probe(&dos), ProbeResult::No);
        assert_eq!(Pe.probe(b"MZ"), ProbeResult::No);
        let mut far = vec![0u8; 0x40];
        far[..2].copy_from_slice(b"MZ");
        far[E_LFANEW_OFFSET..E_LFANEW_OFFSET + 4].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(Pe.probe(&far), ProbeResult::No);
    }
}
