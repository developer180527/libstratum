//! End-to-end: the default engine opens one fixture per container format through the public API.

use std::path::{Path, PathBuf};

use libstratum::model::{Arch, BinaryId, ContainerFormat, Severity};

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
        // Debug info is located and read (embedded DWARF, dSYM, PDB) without warnings.
        assert!(session.has_debug_info(), "{rel}: {:?}", info.diagnostics);
        assert!(info.diagnostics.iter().all(|d| d.severity != Severity::Warning), "{rel}: {:?}", info.diagnostics);
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

/// Inputs that crashed the fuzzer (fuzz/regressions) must now open or fail cleanly. The engine turns
/// panics inside `open` into `OpenError::Internal`, so that variant counts as a regression too.
#[test]
fn fuzz_regressions_do_not_panic() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fuzz/regressions");
    let engine = libstratum::default_engine();
    let mut count = 0;
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let bytes = std::fs::read(&path).unwrap();
        match engine.open(libstratum::Input::Bytes(bytes), &libstratum::OpenOptions::default()) {
            Ok(session) => {
                let image = session.image();
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    for section in image.sections() {
                        let _ = image.section_data(section.id);
                    }
                    let _ = session.info();
                }));
                assert!(result.is_ok(), "{}: panic after open", path.display());
            }
            Err(libstratum::OpenError::Internal(message)) => panic!("{}: {message}", path.display()),
            Err(_) => {}
        }
        count += 1;
    }
    assert!(count > 0);
}
