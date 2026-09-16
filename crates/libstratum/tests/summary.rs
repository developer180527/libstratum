//! Summary lens on every fixture binary: each byte attributed exactly once per address space
//! (docs/12-m3-symbols-sizes.md §3), and the grouping levels agree with each other.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use libstratum::model::{DiagCode, Dimension, Size, SummaryRow};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/bin")
}

/// Every fixture build directory: (toolchain, directory, binary).
fn binaries() -> Vec<(String, PathBuf, PathBuf)> {
    let mut out = Vec::new();
    for toolchain in std::fs::read_dir(fixtures()).unwrap().flatten().filter(|e| e.path().is_dir()) {
        let name = toolchain.file_name().to_string_lossy().into_owned();
        for arch in std::fs::read_dir(toolchain.path()).unwrap().flatten().filter(|e| e.path().is_dir()) {
            for opt in std::fs::read_dir(arch.path()).unwrap().flatten().filter(|e| e.path().is_dir()) {
                for fixture in std::fs::read_dir(opt.path()).unwrap().flatten().filter(|e| e.path().is_dir()) {
                    let stem = fixture.file_name().to_string_lossy().into_owned();
                    let dir = fixture.path();
                    if let Some(binary) =
                        [dir.join(&stem), dir.join(format!("{stem}.exe"))].into_iter().find(|p| p.is_file())
                    {
                        out.push((name.clone(), dir, binary));
                    }
                }
            }
        }
    }
    out.sort();
    out
}

fn sum(rows: &[SummaryRow]) -> Size {
    rows.iter().fold(Size::default(), |a, r| Size {
        vm: a.vm + r.size.vm,
        file: a.file + r.size.file,
        load: a.load + r.size.load,
    })
}

fn plus(a: Size, b: Size) -> Size {
    Size { vm: a.vm + b.vm, file: a.file + b.file, load: a.load + b.load }
}

#[test]
fn every_byte_is_attributed_once() {
    let engine = libstratum::default_engine();
    let mut checked = 0;
    let mut named_symbol_bytes = 0u64;
    for (toolchain, _, binary) in binaries() {
        let label = binary.strip_prefix(fixtures()).unwrap().display().to_string();
        let session = libstratum::open_path(&engine, &binary).unwrap();
        let file_len = std::fs::metadata(&binary).unwrap().len();

        let sections = session.summary(&[Dimension::Section]).unwrap();
        let symbols = session.summary(&[Dimension::Section, Dimension::Symbol]).unwrap();
        for summary in [&sections, &symbols] {
            assert_eq!(plus(sum(&summary.rows), sum(&summary.unattributed)), summary.total, "{label}: invariant");
            assert_eq!(summary.total.file, file_len, "{label}: file total is the input length");
            assert!(summary.shared.vm <= summary.total.vm && summary.shared.file <= summary.total.file, "{label}");
            assert!(
                !summary.diagnostics.iter().any(|d| d.code == DiagCode::SectionOverlap),
                "{label}: {:?}",
                summary.diagnostics
            );
            for row in summary.rows.iter().chain(&summary.unattributed) {
                assert!(row.size != Size::default(), "{label}: empty row {:?}", row.path);
                assert!(!row.evidence.is_empty(), "{label}: row without evidence {:?}", row.path);
            }
        }
        assert_eq!(sections.total, symbols.total, "{label}");

        // Symbol rows plus in-section `[no symbol]` rows add up to each section row.
        let mut per_section: BTreeMap<String, Size> = BTreeMap::new();
        for row in symbols.rows.iter().chain(symbols.unattributed.iter().filter(|r| r.path.len() == 2)) {
            let entry = per_section.entry(row.path[0].clone()).or_default();
            *entry = plus(*entry, row.size);
        }
        for row in &sections.rows {
            assert_eq!(per_section.get(&row.path[0]), Some(&row.size), "{label}: section {}", row.path[0]);
        }
        named_symbol_bytes += sum(&symbols.rows).vm;

        // A section's row never exceeds its header size; it's smaller only where sections overlap
        // (`.tbss` templates yield to the section at their address).
        let image = session.image();
        for row in &sections.rows {
            let section = image
                .sections()
                .iter()
                .find(|s| s.segment.as_ref().map_or(s.name.clone(), |g| format!("{g},{}", s.name)) == row.path[0])
                .unwrap();
            if let Some(vm) = section.extent.vm {
                assert!(row.size.vm <= vm.size, "{label}: {} vm", row.path[0]);
            }
        }

        // Berkeley totals cover exactly the mapped sections.
        let b = session.berkeley_sizes();
        let mapped: u64 = image.sections().iter().filter_map(|s| s.extent.vm).map(|r| r.size).sum();
        assert_eq!(b.text + b.data + b.bss, mapped, "{label}: berkeley");
        if toolchain.starts_with("apple-clang") {
            let headers = sections.unattributed.iter().find(|r| r.path == ["[headers]"]).unwrap();
            assert_eq!(headers.size.file, image.headers_size(), "{label}: Mach-O headers");
        }
        checked += 1;
    }
    assert!(checked >= 382, "every fixture binary checked, got {checked}");
    assert!(named_symbol_bytes > 0);
}

/// Golden `summary([Section])` and Berkeley totals per fixture, so any change is a reviewable diff;
/// `fixtures/crosscheck` compares the Berkeley totals with GNU `size`.
/// Regenerate with `STRATUM_BLESS=1 cargo test -p libstratum --test summary`.
#[test]
fn summaries_match_goldens() {
    let bless = std::env::var_os("STRATUM_BLESS").is_some();
    let engine = libstratum::default_engine();
    let golden_root = fixtures().join("../golden");
    let mut mismatches = Vec::new();
    let mut compared = 0;
    for (_, dir, binary) in binaries() {
        let session = libstratum::open_path(&engine, &binary).unwrap();
        let mut document = serde_json::Map::new();
        document.insert("berkeley".into(), serde_json::to_value(session.berkeley_sizes()).unwrap());
        document
            .insert("sections".into(), serde_json::to_value(session.summary(&[Dimension::Section]).unwrap()).unwrap());
        let actual = serde_json::to_string_pretty(&document).unwrap() + "\n";

        let relative = dir.strip_prefix(fixtures()).unwrap();
        let golden = golden_root.join(relative).join("summary.json");
        if bless {
            std::fs::create_dir_all(golden.parent().unwrap()).unwrap();
            std::fs::write(&golden, &actual).unwrap();
        } else {
            match std::fs::read_to_string(&golden) {
                Ok(expected) if expected.replace("\r\n", "\n") == actual => {}
                Ok(_) => mismatches.push(format!("{} differs", relative.display())),
                Err(_) => mismatches.push(format!("{} has no golden", relative.display())),
            }
        }
        compared += 1;
    }
    assert!(compared >= 382, "expected every fixture binary to be compared, got {compared}");
    assert!(
        mismatches.is_empty(),
        "golden mismatches (rerun with STRATUM_BLESS=1 and review the diff):\n{}",
        mismatches.join("\n")
    );
}
