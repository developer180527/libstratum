//! End-to-end: the default engine opens one fixture per container format through the public API.

use std::path::{Path, PathBuf};

use libstratum::model::{Arch, BinaryId, ContainerFormat, DiagCode};

fn fixture(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/bin").join(rel)
}

#[test]
fn opens_elf_macho_and_pe_fixtures() {
    let engine = libstratum::default_engine();
    let cases = [
        ("linux-gcc/x86_64/O2/padding/padding", ContainerFormat::Elf, Arch::X86_64),
        ("arm-none-eabi-gcc/thumbv7em/O2/padding/padding", ContainerFormat::Elf, Arch::Arm),
        ("apple-clang/arm64/O2/padding/padding", ContainerFormat::MachO, Arch::Aarch64),
        ("msvc/arm64/O2/padding/padding.exe", ContainerFormat::Pe, Arch::Aarch64),
    ];
    for (rel, format, arch) in cases {
        let session = libstratum::open_path(&engine, fixture(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"));
        let info = session.info();
        assert_eq!(info.identity.format, format, "{rel}");
        assert_eq!(info.identity.arch, arch, "{rel}");
        assert!(!matches!(info.identity.id, BinaryId::None), "{rel}: identity");
        assert!(info.sections.iter().any(|s| s.size.vm > 0), "{rel}: sections");
        // Debug backends arrive in M2: locations are found, reading them is reported as unimplemented.
        assert!(info.diagnostics.iter().any(|d| d.code == DiagCode::Unimplemented), "{rel}: {:?}", info.diagnostics);
    }
}

#[test]
fn embedded_sections_report_flash_and_ram_addresses() {
    let engine = libstratum::default_engine();
    let session = libstratum::open_path(&engine, fixture("arm-none-eabi-gcc/thumbv7em/O0/c_types/c_types")).unwrap();
    let info = session.info();
    let text = info.sections.iter().find(|s| s.key.name == ".text").unwrap();
    assert_eq!(text.load_address, text.vm_address, "code runs where it is stored");
    let bss = info.sections.iter().find(|s| s.key.name == ".bss").unwrap();
    assert_eq!(bss.vm_address, Some(0x2000_0000));
    assert_eq!(bss.load_address, None, ".bss occupies RAM only");
    assert_eq!(bss.size.load, 0);
}
