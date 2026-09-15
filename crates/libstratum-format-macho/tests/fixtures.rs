//! Mach-O plugin against the committed fixture corpus (fixtures/bin/apple-clang).

use std::path::{Path, PathBuf};

use libstratum_core::ir::{Arch, BinaryId, DebugLocation, SectionKind};
use libstratum_core::spi::conformance::{check_image, check_robustness};
use libstratum_format_macho::MachO;

fn fixture_dir(arch: &str, opt: &str, fixture: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/bin/apple-clang").join(arch).join(opt).join(fixture)
}

fn binary(arch: &str, opt: &str, fixture: &str) -> (Vec<u8>, String) {
    let path = fixture_dir(arch, opt, fixture).join(fixture);
    let label = format!("apple-clang/{arch}/{opt}/{fixture}");
    (std::fs::read(&path).unwrap_or_else(|e| panic!("{label}: {e}")), label)
}

const FIXTURES: &[&str] =
    &["padding", "bitfields", "inheritance", "anonymous", "packed", "templates", "c_types", "odr_conflict"];

#[test]
fn every_macho_fixture_conforms() {
    let mut count = 0;
    for (arch_dir, arch) in [("arm64", Arch::Aarch64), ("x86_64", Arch::X86_64)] {
        for opt in ["O0", "O2"] {
            for fixture in FIXTURES {
                let (bytes, label) = binary(arch_dir, opt, fixture);
                let image = check_image(&MachO, &bytes, &label);
                assert_eq!(image.arch(), arch, "{label}: arch");
                assert_eq!(image.address_size(), 8, "{label}: address size");
                let BinaryId::Uuid(uuid) = image.binary_id() else { panic!("{label}: LC_UUID missing") };
                let locations = image.debug_locations();
                assert!(locations.contains(&DebugLocation::Dsym { uuid }), "{label}: dSYM location");
                let objects: Vec<_> = locations
                    .iter()
                    .filter_map(|l| match l {
                        DebugLocation::MachOObject { path, .. } => Some(path.clone()),
                        _ => None,
                    })
                    .collect();
                let expected: Vec<PathBuf> = if *fixture == "odr_conflict" {
                    vec!["a.o".into(), "b.o".into()]
                } else {
                    vec![format!("{fixture}.o").into()]
                };
                assert_eq!(objects, expected, "{label}: debug map objects (relative OSO paths)");
                assert!(
                    image
                        .symbols()
                        .iter()
                        .any(|s| s.raw_name == "_main" && s.kind == libstratum_core::ir::SymbolKind::Function),
                    "{label}: _main"
                );
                assert!(
                    image.sections().iter().any(|s| s.segment.as_deref() == Some("__TEXT")
                        && s.name == "__text"
                        && s.kind == SectionKind::Code),
                    "{label}: __TEXT,__text"
                );
                count += 1;
            }
        }
    }
    assert_eq!(count, 32);
}

/// The dSYM companion is itself a Mach-O (MH_DSYM) with DWARF in a `__DWARF` segment.
#[test]
fn dsym_companion_has_embedded_dwarf_and_same_uuid() {
    let dir = fixture_dir("arm64", "O2", "padding");
    let binary = std::fs::read(dir.join("padding")).unwrap();
    let dsym = std::fs::read(dir.join("padding.dSYM/Contents/Resources/DWARF/padding")).unwrap();
    let image = check_image(&MachO, &binary, "padding");
    let companion = check_image(&MachO, &dsym, "padding.dSYM");
    assert_eq!(image.binary_id(), companion.binary_id(), "dSYM UUID must match the binary");
    assert!(companion.debug_locations().contains(&DebugLocation::Embedded));
    assert!(companion.sections().iter().any(|s| s.segment.as_deref() == Some("__DWARF") && s.name == "__debug_info"));
}

#[test]
fn robustness_on_truncated_fixtures() {
    let (bytes, label) = binary("arm64", "O2", "inheritance");
    check_robustness(&MachO, &bytes, &label);
}

/// A synthetic universal binary made of the two thin fixture slices selects the requested arch.
#[test]
fn universal_binary_slice_selection() {
    use libstratum_core::host::InMemorySource;
    use libstratum_core::spi::{BinaryFormat, OpenOptions};
    use std::sync::Arc;

    let (arm, _) = binary("arm64", "O0", "padding");
    let (x86, _) = binary("x86_64", "O0", "padding");
    let align = |n: usize| n.div_ceil(0x4000) * 0x4000;
    let arm_off = 0x4000;
    let x86_off = align(arm_off + arm.len());
    let mut fat = vec![0u8; x86_off + x86.len()];
    fat[0..4].copy_from_slice(&0xcafe_babe_u32.to_be_bytes());
    fat[4..8].copy_from_slice(&2u32.to_be_bytes());
    for (i, (cpu, sub, off, len)) in
        [(0x0100_000c_u32, 0u32, arm_off, arm.len()), (0x0100_0007, 3, x86_off, x86.len())].into_iter().enumerate()
    {
        let e = 8 + i * 20;
        for (j, v) in [cpu, sub, off as u32, len as u32, 14].into_iter().enumerate() {
            fat[e + j * 4..e + j * 4 + 4].copy_from_slice(&v.to_be_bytes());
        }
    }
    fat[arm_off..arm_off + arm.len()].copy_from_slice(&arm);
    fat[x86_off..].copy_from_slice(&x86);

    assert_eq!(MachO.probe(&fat), libstratum_core::spi::ProbeResult::YesContainer);
    for arch in [Arch::Aarch64, Arch::X86_64] {
        let image =
            MachO.open(Arc::new(InMemorySource(fat.clone())), &OpenOptions { arch: Some(arch) }).expect("slice opens");
        assert_eq!(image.arch(), arch);
        // Section data must be read relative to the slice, not the file start.
        let text = image.sections().iter().find(|s| s.name == "__text").unwrap();
        let thin = if arch == Arch::Aarch64 { &arm } else { &x86 };
        let file = text.extent.file.unwrap();
        assert_eq!(
            &image.section_data(text.id).unwrap()[..],
            &thin[file.start as usize..(file.start + file.size) as usize]
        );
    }
    let err = MachO.open(Arc::new(InMemorySource(fat)), &OpenOptions { arch: Some(Arch::Riscv32) }).unwrap_err();
    assert!(err.to_string().contains("available"), "{err}");
}
