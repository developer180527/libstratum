//! Golden layouts: the full `struct_layout` output for every fixture type in every fixture binary is
//! committed under `fixtures/golden/`, so any change in library output is a reviewable diff.
//! Regenerate with `STRATUM_BLESS=1 cargo test -p libstratum --test layout_golden`.

use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures")
}

const TYPES: &[(&str, &[&str])] = &[
    ("padding", &["Packet", "PacketReordered", "CacheStraddle", "Empty"]),
    ("bitfields", &["Flags", "RegisterBits"]),
    ("inheritance", &["WithEmptyBase", "Multiple", "Polymorphic", "Derived", "Diamond", "VLeft"]),
    ("anonymous", &["Variant", "engine::Outer", "engine::Outer::Inner", "Aligned", "Holder"]),
    ("packed", &["WireHeader", "Pack2"]),
    ("templates", &["Pair<char,double>", "Pair<double,char>", "SmallArray<short,3>"]),
    ("c_types", &["Point", "Number", "Message"]),
    ("odr_conflict", &["Config"]),
    ("opaque", &["Opaque", "Defined"]),
];

#[test]
fn layouts_match_goldens() {
    let bless = std::env::var_os("STRATUM_BLESS").is_some();
    let engine = libstratum::default_engine();
    let mut mismatches = Vec::new();
    let mut compared = 0;

    let mut binaries: Vec<PathBuf> = Vec::new();
    for toolchain in std::fs::read_dir(root().join("bin")).unwrap().flatten().filter(|e| e.path().is_dir()) {
        for arch in std::fs::read_dir(toolchain.path()).unwrap().flatten() {
            for opt in std::fs::read_dir(arch.path()).unwrap().flatten() {
                for fixture in std::fs::read_dir(opt.path()).unwrap().flatten() {
                    binaries.push(fixture.path());
                }
            }
        }
    }
    binaries.sort();

    for dir in binaries {
        let fixture = dir.file_name().unwrap().to_string_lossy().into_owned();
        let Some((_, types)) = TYPES.iter().find(|(f, _)| *f == fixture) else { continue };
        let binary =
            [dir.join(&fixture), dir.join(format!("{fixture}.exe"))].into_iter().find(|p| p.is_file()).unwrap();
        let session = libstratum::open_path(&engine, &binary).unwrap();

        let mut document = serde_json::Map::new();
        for ty in *types {
            let result = session.struct_layout(ty).unwrap();
            document.insert((*ty).to_owned(), serde_json::to_value(&result).unwrap());
        }
        let actual = serde_json::to_string_pretty(&document).unwrap() + "\n";

        let relative = dir.strip_prefix(root().join("bin")).unwrap();
        let golden = root().join("golden").join(relative).join("layout.json");
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
    // 334 today; 342 once the Windows `opaque` builds from the fixtures workflow are committed.
    assert!(compared >= 334, "expected every fixture binary to be compared, got {compared}");
    assert!(
        mismatches.is_empty(),
        "golden mismatches (rerun with STRATUM_BLESS=1 and review the diff):\n{}",
        mismatches.join("\n")
    );
}
