//! Conformance checks every `BinaryFormat` plugin must pass on its fixtures (docs/06 §3).
//! Enabled with the `conformance` feature; plugins use it from their integration tests.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;

use crate::host::InMemorySource;
use crate::ir::SymbolKind;
use crate::spi::{BinaryFormat, Image, OpenOptions, ProbeResult};

/// Opens `bytes` with `format` and checks structural invariants. Returns the image on success
/// so callers can add fixture-specific assertions. Panics with a descriptive message on violation.
pub fn check_image(format: &dyn BinaryFormat, bytes: &[u8], label: &str) -> Box<dyn Image> {
    let header = &bytes[..bytes.len().min(4096)];
    assert_ne!(format.probe(header), ProbeResult::No, "{label}: probe rejected its own fixture");

    let source = Arc::new(InMemorySource(bytes.to_vec()));
    let image = format.open(source, &OpenOptions::default()).unwrap_or_else(|e| panic!("{label}: open failed: {e}"));
    let file_len = bytes.len() as u64;

    let mut previous = None;
    for section in image.sections() {
        assert!(previous < Some(section.id), "{label}: section ids must be unique and increasing");
        previous = Some(section.id);
        if let Some(file) = section.extent.file {
            assert!(file.end() <= file_len, "{label}: section {} file extent past end of file", section.name);
        }
        if section.extent.load.is_some() {
            assert!(
                section.extent.vm.is_some() && section.extent.file.is_some(),
                "{label}: section {} has a load extent without vm and file extents",
                section.name
            );
        }
        if let Some(file) = section.extent.file
            && section.extent.vm.is_some()
        {
            let data = image
                .section_data(section.id)
                .unwrap_or_else(|e| panic!("{label}: section_data({}) failed: {e}", section.name));
            assert_eq!(data.len() as u64, file.size, "{label}: section {} data length", section.name);
        }
    }
    for segment in image.segments() {
        if let Some(file) = segment.extent.file {
            assert!(file.end() <= file_len, "{label}: segment file extent past end of file");
        }
    }

    for symbol in image.symbols() {
        let Some(section_id) = symbol.section else { continue };
        let section = image
            .sections()
            .iter()
            .find(|s| s.id == section_id)
            .unwrap_or_else(|| panic!("{label}: symbol {} refers to missing section", symbol.raw_name));
        // TLS symbol values are offsets into the TLS template, not addresses.
        if symbol.kind == SymbolKind::Tls || symbol.kind == SymbolKind::Section {
            continue;
        }
        if let Some(vm) = section.extent.vm {
            // `<=` end: linker-defined end markers (e.g. `_etext`) sit one past the last byte.
            assert!(
                symbol.address >= vm.start && symbol.address <= vm.end(),
                "{label}: symbol {} at {:#x} outside section {} [{:#x}, {:#x})",
                symbol.raw_name,
                symbol.address,
                section.name,
                vm.start,
                vm.end()
            );
        }
        assert!(!(symbol.is_thumb && symbol.address & 1 == 1), "{label}: Thumb bit not cleared on {}", symbol.raw_name);
    }

    image
}

/// Truncated and corrupted inputs must produce errors, never panics.
pub fn check_robustness(format: &dyn BinaryFormat, bytes: &[u8], label: &str) {
    let mut cuts: Vec<usize> = vec![0, 1, 4, 16, 64, 128, 512, 4096, bytes.len() / 2, bytes.len().saturating_sub(1)];
    cuts.retain(|&c| c < bytes.len());
    for cut in cuts {
        let truncated = bytes[..cut].to_vec();
        let result = catch_unwind(AssertUnwindSafe(|| {
            let _ = format.probe(&truncated);
            let _ = format.open(Arc::new(InMemorySource(truncated.clone())), &OpenOptions::default()).map(|image| {
                for section in image.sections() {
                    let _ = image.section_data(section.id);
                }
            });
        }));
        assert!(result.is_ok(), "{label}: panic on input truncated to {cut} bytes");
    }
}
