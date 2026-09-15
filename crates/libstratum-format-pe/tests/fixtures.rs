//! PE plugin against the committed fixture corpus (fixtures/bin/msvc, fixtures/bin/clang-cl).

use std::path::{Path, PathBuf};

use libstratum_core::ir::{Arch, BinaryId, DebugLocation, SectionKind};
use libstratum_core::spi::conformance::{check_image, check_robustness};
use libstratum_format_pe::Pe;

fn fixture_dir(toolchain: &str, arch: &str, opt: &str, fixture: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/bin").join(toolchain).join(arch).join(opt).join(fixture)
}

const FIXTURES: &[&str] =
    &["padding", "bitfields", "inheritance", "anonymous", "packed", "templates", "c_types", "odr_conflict"];

/// GUID and age as recorded in the PDB info stream, read independently with pdb2.
fn pdb_identity(path: &Path) -> ([u8; 16], u32) {
    let mut pdb = pdb2::PDB::open(std::fs::File::open(path).unwrap()).unwrap();
    let info = pdb.pdb_information().unwrap();
    // The executable's RSDS age matches the DBI stream age; the PDB info stream age can be higher.
    let dbi_age = pdb.debug_information().unwrap().age().unwrap_or(info.age);
    (info.guid.to_bytes_le(), dbi_age)
}

#[test]
fn every_pe_fixture_conforms() {
    let mut count = 0;
    for toolchain in ["msvc", "clang-cl"] {
        for (arch_dir, arch) in [("x64", Arch::X86_64), ("arm64", Arch::Aarch64)] {
            for opt in ["O0", "O2"] {
                for fixture in FIXTURES {
                    let dir = fixture_dir(toolchain, arch_dir, opt, fixture);
                    let label = format!("{toolchain}/{arch_dir}/{opt}/{fixture}");
                    let bytes = std::fs::read(dir.join(format!("{fixture}.exe"))).unwrap();
                    let image = check_image(&Pe, &bytes, &label);

                    assert_eq!(image.arch(), arch, "{label}: arch");
                    assert_eq!(image.address_size(), 8, "{label}: address size");
                    assert_eq!(image.image_base(), 0x1_4000_0000, "{label}: default x64/arm64 exe base");
                    assert!(image.symbols().is_empty(), "{label}: linked PE symbols come from the PDB");

                    let BinaryId::PdbGuidAge { guid, age } = image.binary_id() else { panic!("{label}: no RSDS") };
                    let (pdb_guid, pdb_age) = pdb_identity(&dir.join(format!("{fixture}.pdb")));
                    assert_eq!(guid, pdb_guid, "{label}: RSDS GUID must match the PDB");
                    assert_eq!(age, pdb_age, "{label}: RSDS age must match the PDB");

                    let pdb_path = image.debug_locations().into_iter().find_map(|l| match l {
                        DebugLocation::Pdb { path, .. } => Some(path),
                        _ => None,
                    });
                    let pdb_path = pdb_path.unwrap_or_else(|| panic!("{label}: no PDB location"));
                    assert!(
                        pdb_path.to_string_lossy().ends_with(&format!("{fixture}.pdb")),
                        "{label}: RSDS path {pdb_path:?}"
                    );

                    let kind = |name: &str| image.sections().iter().find(|s| s.name == name).map(|s| s.kind);
                    assert_eq!(kind(".text"), Some(SectionKind::Code), "{label}: .text");
                    assert_eq!(kind(".rdata"), Some(SectionKind::ReadOnlyData), "{label}: .rdata");
                    assert_eq!(kind(".pdata"), Some(SectionKind::Unwind), "{label}: .pdata");
                    count += 1;
                }
            }
        }
    }
    assert_eq!(count, 64);
}

#[test]
fn robustness_on_truncated_fixtures() {
    let dir = fixture_dir("msvc", "x64", "O2", "inheritance");
    let bytes = std::fs::read(dir.join("inheritance.exe")).unwrap();
    check_robustness(&Pe, &bytes, "msvc/x64/O2/inheritance");
}
