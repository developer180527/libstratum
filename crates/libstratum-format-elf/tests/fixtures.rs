//! ELF plugin against the committed fixture corpus (fixtures/bin).

use std::path::{Path, PathBuf};

use libstratum_core::ir::{Arch, BinaryId, DebugLocation, SectionKind};
use libstratum_core::spi::conformance::{check_image, check_robustness};
use libstratum_format_elf::Elf;

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/bin")
}

fn binary(toolchain: &str, arch: &str, opt: &str, fixture: &str) -> (Vec<u8>, String) {
    let path = fixtures().join(toolchain).join(arch).join(opt).join(fixture).join(fixture);
    let label = format!("{toolchain}/{arch}/{opt}/{fixture}");
    (std::fs::read(&path).unwrap_or_else(|e| panic!("{label}: {e}")), label)
}

/// (arch directory, expected arch, address size)
type ArchCase = (&'static str, Arch, u8);

const ELF_TOOLCHAINS: &[(&str, &[ArchCase])] = &[
    ("linux-clang", &[("x86_64", Arch::X86_64, 8), ("aarch64", Arch::Aarch64, 8)]),
    ("linux-gcc", &[("x86_64", Arch::X86_64, 8), ("aarch64", Arch::Aarch64, 8)]),
    ("arm-none-eabi-gcc", &[("thumbv6m", Arch::Arm, 4), ("thumbv7em", Arch::Arm, 4), ("thumbv8m", Arch::Arm, 4)]),
    ("arm-llvm", &[("thumbv6m", Arch::Arm, 4), ("thumbv7em", Arch::Arm, 4), ("thumbv8m", Arch::Arm, 4)]),
    ("riscv-gcc", &[("rv32imac", Arch::Riscv32, 4)]),
];
const FIXTURES: &[&str] =
    &["padding", "bitfields", "inheritance", "anonymous", "packed", "templates", "c_types", "odr_conflict"];

#[test]
fn every_elf_fixture_conforms() {
    let mut count = 0;
    for (toolchain, arches) in ELF_TOOLCHAINS {
        for (arch_dir, arch, address_size) in *arches {
            for opt in ["O0", "O2"] {
                for fixture in FIXTURES {
                    let (bytes, label) = binary(toolchain, arch_dir, opt, fixture);
                    let image = check_image(&Elf, &bytes, &label);
                    assert_eq!(image.arch(), *arch, "{label}: arch");
                    assert_eq!(image.address_size(), *address_size, "{label}: address size");
                    assert!(
                        matches!(image.binary_id(), BinaryId::BuildId(ref id) if id.len() == 20),
                        "{label}: build id"
                    );
                    assert!(image.debug_locations().contains(&DebugLocation::Embedded), "{label}: embedded DWARF");
                    assert!(image.sections().iter().any(|s| s.kind == SectionKind::Debug), "{label}: debug sections");
                    assert!(image.symbols().iter().any(|s| s.raw_name == "main"), "{label}: main symbol");
                    count += 1;
                }
            }
        }
    }
    assert_eq!(count, 176);
}

#[test]
fn robustness_on_truncated_fixtures() {
    for (toolchain, arch, fixture) in [("linux-gcc", "x86_64", "inheritance"), ("arm-llvm", "thumbv6m", "c_types")] {
        let (bytes, label) = binary(toolchain, arch, "O2", fixture);
        check_robustness(&Elf, &bytes, &label);
    }
}

/// Thumb functions: symbol addresses are even and flagged; mapping symbols are filtered (M0 S3, S9).
#[test]
fn thumb_symbols_are_normalized() {
    let (bytes, label) = binary("arm-none-eabi-gcc", "thumbv7em", "O2", "padding");
    let image = check_image(&Elf, &bytes, &label);
    let main = image.symbols().iter().find(|s| s.raw_name == "main").unwrap();
    assert!(main.is_thumb);
    assert_eq!(main.address & 1, 0);
    assert!(image.symbols().iter().all(|s| !s.raw_name.starts_with('$')), "mapping symbols leaked");
}

/// Flash vs RAM: .isr_vector/.text in flash; .bss only in RAM with no load extent (ADR-0016, M0 S9).
#[test]
fn embedded_load_and_run_addresses() {
    let (bytes, label) = binary("arm-none-eabi-gcc", "thumbv7em", "O0", "c_types");
    let image = check_image(&Elf, &bytes, &label);
    let section = |name: &str| image.sections().iter().find(|s| s.name == name).unwrap_or_else(|| panic!("{name}"));

    let vectors = section(".isr_vector");
    assert_eq!(vectors.extent.vm.unwrap().start, 0x0800_0000);
    assert_eq!(vectors.extent.load.unwrap().start, 0x0800_0000);
    assert_eq!(vectors.kind, SectionKind::ReadOnlyData);

    assert_eq!(section(".text").kind, SectionKind::Code);

    let bss = section(".bss");
    assert_eq!(bss.kind, SectionKind::ZeroInit);
    assert_eq!(bss.extent.vm.unwrap().start, 0x2000_0000);
    assert!(bss.extent.file.is_none() && bss.extent.load.is_none());

    let rw_segment = image.segments().iter().find(|s| s.extent.vm.is_some_and(|vm| vm.start == 0x2000_0000)).unwrap();
    assert!(rw_segment.extent.load.is_none(), "zero-initialized RAM segment stores nothing in flash");
}
