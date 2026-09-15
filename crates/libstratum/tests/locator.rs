//! FsLocator against real fixtures and synthetic directory layouts.

use std::path::{Path, PathBuf};

use libstratum::host::{FileLocator, LocateRequest};
use libstratum::ir::DebugLocation;
use libstratum::model::BinaryId;
use libstratum::{FsLocator, Input, OpenOptions};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/bin")
}

/// A scratch directory under the system temp dir, removed on drop.
struct Scratch(PathBuf);

impl Scratch {
    fn new(tag: &str) -> Self {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let dir = std::env::temp_dir().join(format!("libstratum-{tag}-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        Scratch(dir)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn locate(locator: &FsLocator, image: Option<&Path>, location: DebugLocation) -> Option<Vec<u8>> {
    let request = LocateRequest { image_path: image.map(Path::to_path_buf), location };
    locator.locate(&request).unwrap().map(|source| source.bytes().unwrap().to_vec())
}

fn identity_of(bytes: Vec<u8>) -> BinaryId {
    let session = libstratum::default_engine().open(Input::Bytes(bytes), &OpenOptions::default()).unwrap();
    session.image().binary_id()
}

#[test]
fn dsym_next_to_macho_binary() {
    let binary = fixtures().join("apple-clang/arm64/O2/padding/padding");
    let session = libstratum::open_path(&libstratum::default_engine(), &binary).unwrap();
    let BinaryId::Uuid(uuid) = session.image().binary_id() else { panic!("no uuid") };
    let dsym = locate(&FsLocator::default(), Some(&binary), DebugLocation::Dsym { uuid }).expect("dSYM found");
    assert_eq!(identity_of(dsym), BinaryId::Uuid(uuid), "dSYM identity");
}

#[test]
fn relative_debug_map_object_next_to_binary() {
    let binary = fixtures().join("apple-clang/x86_64/O0/odr_conflict/odr_conflict");
    let found =
        locate(&FsLocator::default(), Some(&binary), DebugLocation::MachOObject { path: "b.o".into(), mtime: 0 })
            .expect("object found");
    assert_eq!(found, std::fs::read(binary.with_file_name("b.o")).unwrap());
}

#[test]
fn windows_pdb_path_resolves_next_to_exe_on_any_host() {
    let exe = fixtures().join("msvc/x64/O2/padding/padding.exe");
    let session = libstratum::open_path(&libstratum::default_engine(), &exe).unwrap();
    let location = session
        .image()
        .debug_locations()
        .into_iter()
        .find(|l| matches!(l, DebugLocation::Pdb { .. }))
        .expect("RSDS location");
    let pdb = locate(&FsLocator::default(), Some(&exe), location).expect("PDB found next to exe");
    assert!(pdb.starts_with(b"Microsoft C/C++ MSF 7.00"), "PDB magic");
}

#[test]
fn symbol_store_layout() {
    let exe = fixtures().join("clang-cl/arm64/O2/packed/packed.exe");
    let session = libstratum::open_path(&libstratum::default_engine(), &exe).unwrap();
    let Some(DebugLocation::Pdb { path, guid, age }) =
        session.image().debug_locations().into_iter().find(|l| matches!(l, DebugLocation::Pdb { .. }))
    else {
        panic!("RSDS location")
    };

    // The exe alone in a directory: not found without a store.
    let scratch = Scratch::new("store");
    let lonely = scratch.0.join("bin/packed.exe");
    std::fs::create_dir_all(lonely.parent().unwrap()).unwrap();
    std::fs::copy(&exe, &lonely).unwrap();
    let location = DebugLocation::Pdb { path, guid, age };
    assert!(locate(&FsLocator::default(), Some(&lonely), location.clone()).is_none());

    // Populate `<store>/packed.pdb/<GUID><AGE>/packed.pdb` with the key computed independently.
    let d1 = u32::from_le_bytes(guid[0..4].try_into().unwrap());
    let d2 = u16::from_le_bytes(guid[4..6].try_into().unwrap());
    let d3 = u16::from_le_bytes(guid[6..8].try_into().unwrap());
    let d4: String = guid[8..].iter().map(|b| format!("{b:02X}")).collect();
    let key = format!("{d1:08X}{d2:04X}{d3:04X}{d4}{age:X}");
    let store = scratch.0.join("symbols");
    let slot = store.join("packed.pdb").join(key);
    std::fs::create_dir_all(&slot).unwrap();
    std::fs::copy(exe.with_extension("pdb"), slot.join("packed.pdb")).unwrap();

    let found = locate(&FsLocator::default().symbol_store(&store), Some(&lonely), location).expect("found in store");
    assert_eq!(found, std::fs::read(exe.with_extension("pdb")).unwrap());
}

#[test]
fn build_id_tree_under_debug_root() {
    let binary = fixtures().join("linux-clang/x86_64/O2/padding/padding");
    let session = libstratum::open_path(&libstratum::default_engine(), &binary).unwrap();
    let BinaryId::BuildId(id) = session.image().binary_id() else { panic!("no build id") };

    let scratch = Scratch::new("buildid");
    let hex: String = id.iter().map(|b| format!("{b:02x}")).collect();
    let target = scratch.0.join(".build-id").join(&hex[..2]).join(format!("{}.debug", &hex[2..]));
    std::fs::create_dir_all(target.parent().unwrap()).unwrap();
    std::fs::copy(&binary, &target).unwrap();

    let location = DebugLocation::BuildId { id };
    assert!(locate(&FsLocator::default(), Some(&binary), location.clone()).is_none(), "no roots configured");
    assert!(locate(&FsLocator::default().debug_root(&scratch.0), Some(&binary), location).is_some());
}

#[test]
fn debuglink_requires_matching_crc() {
    let scratch = Scratch::new("debuglink");
    let image = scratch.0.join("app");
    std::fs::write(&image, b"not really an image").unwrap();
    std::fs::write(scratch.0.join("app.debug"), b"123456789").unwrap();

    let link = |crc32| DebugLocation::DebugLink { name: "app.debug".into(), crc32 };
    assert!(locate(&FsLocator::default(), Some(&image), link(0xCBF4_3926)).is_some(), "matching CRC");
    assert!(locate(&FsLocator::default(), Some(&image), link(0xDEAD_BEEF)).is_none(), "stale debug file");
}

#[test]
fn archive_member_objects() {
    let scratch = Scratch::new("archive");
    let image = scratch.0.join("app");
    std::fs::write(&image, b"image").unwrap();
    // Minimal System V `ar` archive with one member.
    let payload = b"object bytes";
    let mut ar = b"!<arch>\n".to_vec();
    ar.extend(format!("{:<16}{:<12}{:<6}{:<6}{:<8}{:<10}`\n", "member.o/", 0, 0, 0, 644, payload.len()).as_bytes());
    ar.extend_from_slice(payload);
    std::fs::write(scratch.0.join("libdemo.a"), &ar).unwrap();

    let found = locate(
        &FsLocator::default(),
        Some(&image),
        DebugLocation::MachOObject { path: "libdemo.a(member.o)".into(), mtime: 0 },
    );
    assert_eq!(found.as_deref(), Some(&payload[..]));
}

#[test]
fn prefix_map_rewrites_build_machine_paths() {
    let dir = fixtures().join("apple-clang/arm64/O0/padding");
    let recorded = PathBuf::from("/build-machine/checkout/objects/padding.o");
    let location = DebugLocation::MachOObject { path: recorded, mtime: 0 };
    let locator = FsLocator::default().prefix_map("/build-machine/checkout/objects", &dir);
    let unrelated_image = std::env::temp_dir().join("elsewhere/app");
    assert!(locate(&FsLocator::default(), Some(&unrelated_image), location.clone()).is_none());
    assert!(locate(&locator, Some(&unrelated_image), location).is_some());
}

#[cfg(feature = "mmap")]
#[test]
fn memory_mapped_open_matches_read() {
    let engine = libstratum::default_engine();
    let binary = fixtures().join("linux-gcc/aarch64/O2/templates/templates");
    let mapped = libstratum::open_path_mmap(&engine, &binary).unwrap();
    let read = libstratum::open_path(&engine, &binary).unwrap();
    assert_eq!(mapped.info(), read.info());
}
