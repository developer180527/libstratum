//! Layout lens on every fixture — DWARF (ELF, Mach-O) and PDB (PE) — with the ABI facts that must
//! hold on all of them. This is the M2 abstraction proof: one query, one model, every platform.

use std::path::{Path, PathBuf};

use libstratum::model::{DiagCode, MemberKind};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/bin")
}

/// (toolchain, arches, pointer bytes, binary file suffix)
const TOOLCHAINS: &[(&str, &[&str], u64, &str)] = &[
    ("apple-clang", &["arm64", "x86_64"], 8, ""),
    ("linux-clang", &["x86_64", "aarch64"], 8, ""),
    ("linux-clang-types5", &["x86_64"], 8, ""),
    ("linux-clang-types4", &["x86_64"], 8, ""),
    ("linux-gcc", &["x86_64", "aarch64"], 8, ""),
    ("arm-none-eabi-gcc", &["thumbv6m", "thumbv7em", "thumbv8m"], 4, ""),
    ("arm-llvm", &["thumbv6m", "thumbv7em", "thumbv8m"], 4, ""),
    ("riscv-gcc", &["rv32imac"], 4, ""),
    ("msvc", &["x64", "arm64"], 8, ".exe"),
    ("clang-cl", &["x64", "arm64"], 8, ".exe"),
];

fn offsets(layout: &libstratum::model::TypeLayout) -> Vec<u64> {
    layout.members.iter().filter_map(|m| m.offset_bits).map(|b| b / 8).collect()
}

#[test]
fn fixture_types_have_expected_layouts_on_every_toolchain() {
    let engine = libstratum::default_engine();
    let mut checked = 0;
    for (toolchain, arches, ptr, suffix) in TOOLCHAINS {
        for arch in *arches {
            for opt in ["O0", "O2"] {
                let open = |fixture: &str| {
                    let path = fixtures()
                        .join(toolchain)
                        .join(arch)
                        .join(opt)
                        .join(fixture)
                        .join(format!("{fixture}{suffix}"));
                    libstratum::open_path(&engine, &path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
                };
                let label = |fixture: &str, ty: &str| format!("{toolchain}/{arch}/{opt}/{fixture} {ty}");
                let one = |session: &libstratum::Session, fixture: &str, ty: &str| {
                    let result = session.struct_layout(ty).unwrap();
                    assert_eq!(
                        result.matches.len(),
                        1,
                        "{}: {:?} {:?}",
                        label(fixture, ty),
                        result.not_found_reason,
                        session.diagnostics()
                    );
                    result.matches.into_iter().next().unwrap()
                };

                let s = open("padding");
                let packet = one(&s, "padding", "Packet");
                assert_eq!(
                    (packet.size_bytes, offsets(&packet)),
                    (24, vec![0, 8, 16, 20]),
                    "{}",
                    label("padding", "Packet")
                );
                assert_eq!(packet.suggestion.as_ref().map(|s| s.new_size_bytes), Some(16));
                assert_eq!(one(&s, "padding", "PacketReordered").size_bytes, 16);
                assert!(one(&s, "padding", "PacketReordered").suggestion.is_none());
                let straddle = one(&s, "padding", "CacheStraddle");
                assert_eq!((straddle.size_bytes, straddle.cacheline.boundaries.clone()), (80, vec![64]));
                assert_eq!(one(&s, "padding", "Empty").size_bytes, 1);

                let s = open("bitfields");
                let reg = one(&s, "bitfields", "RegisterBits");
                assert_eq!(reg.size_bytes, 4);
                let bits: Vec<(u64, u64)> = reg.members.iter().map(|m| (m.offset_bits.unwrap(), m.size_bits)).collect();
                assert_eq!(bits, vec![(0, 1), (1, 7), (8, 8), (16, 16)], "{}", label("bitfields", "RegisterBits"));
                let flags = one(&s, "bitfields", "Flags");
                let c = flags.members.iter().find(|m| m.name.as_deref() == Some("c")).unwrap();
                assert_eq!(
                    c.offset_bits,
                    Some(32),
                    "zero-width bitfield starts a new unit: {}",
                    label("bitfields", "Flags")
                );

                let s = open("inheritance");
                let poly = one(&s, "inheritance", "Polymorphic");
                assert_eq!(poly.members[0].kind, MemberKind::VtablePtr);
                assert_eq!(poly.members[0].size_bits, ptr * 8);
                assert_eq!(poly.size_bytes, if *ptr == 8 { 24 } else { 16 });
                let ebo = one(&s, "inheritance", "WithEmptyBase");
                assert_eq!((ebo.size_bytes, ebo.padding_bits), (4, 0), "empty base optimization");
                let diamond = one(&s, "inheritance", "Diamond");
                assert!(
                    !diamond.notes.is_empty(),
                    "{}: virtual bases are reported as a limitation",
                    label("inheritance", "Diamond")
                );
                assert_eq!(diamond.members.iter().filter(|m| m.kind == MemberKind::Base).count(), 2);

                let s = open("anonymous");
                assert_eq!(one(&s, "anonymous", "Variant").size_bytes, 8);
                assert_eq!(one(&s, "anonymous", "Aligned").size_bytes, 32, "alignas(16)");
                assert_eq!(one(&s, "anonymous", "Holder").size_bytes, *ptr, "[[no_unique_address]]");
                let inner = one(&s, "anonymous", "Inner");
                assert_eq!(inner.name, "engine::Outer::Inner");

                let s = open("packed");
                let wire = one(&s, "packed", "WireHeader");
                assert_eq!((wire.size_bytes, offsets(&wire), wire.packed), (7, vec![0, 1, 5], true));
                assert!(wire.suggestion.is_none());
                let pack2 = one(&s, "packed", "Pack2");
                assert_eq!((pack2.size_bytes, offsets(&pack2), pack2.packed), (8, vec![0, 2, 6], true));

                let s = open("templates");
                assert_eq!(one(&s, "templates", "Pair<char,double>").size_bytes, 16);
                assert_eq!(one(&s, "templates", "Pair<char, double>").size_bytes, 16, "spelling-insensitive");
                assert_eq!(offsets(&one(&s, "templates", "Pair<double,char>")), vec![0, 8]);
                assert_eq!(one(&s, "templates", "SmallArray<short,3>").size_bytes, 8);

                let s = open("c_types");
                assert_eq!(one(&s, "c_types", "Point").size_bytes, 12);
                assert_eq!(one(&s, "c_types", "Number").size_bytes, 8);
                let message = one(&s, "c_types", "Message");
                assert_eq!(message.members.last().unwrap().size_bits, 0, "flexible array member");

                let s = open("odr_conflict");
                let config = s.struct_layout("Config").unwrap();
                let mut sizes: Vec<u64> = config.matches.iter().map(|m| m.size_bytes).collect();
                sizes.sort();
                if toolchain.starts_with("linux-clang-types") {
                    // Type-unit signatures hash the type's name, so the linker deduplicates the two
                    // conflicting definitions: only one survives in the binary (M2 finding).
                    assert_eq!(sizes.len(), 1, "{}", label("odr_conflict", "Config"));
                } else {
                    assert_eq!(sizes, vec![16, 32], "{}", label("odr_conflict", "Config"));
                    assert!(config.diagnostics.iter().any(|d| d.code == DiagCode::OdrConflict));
                }

                // Declared but never defined: reported as a declaration, not as "no such type".
                // Windows builds of `opaque` come from the fixtures workflow (pending until it runs).
                let opaque_dir = fixtures().join(toolchain).join(arch).join(opt).join("opaque");
                if opaque_dir.is_dir() {
                    let s = open("opaque");
                    let opaque = s.struct_layout("Opaque").unwrap();
                    assert!(opaque.matches.is_empty(), "{}", label("opaque", "Opaque"));
                    assert!(
                        opaque.not_found_reason.as_deref().is_some_and(|r| r.contains("declarations")),
                        "{}: {:?}",
                        label("opaque", "Opaque"),
                        opaque.not_found_reason
                    );
                    assert_eq!(one(&s, "opaque", "Defined").size_bytes, 4);
                } else {
                    assert!(
                        matches!(*toolchain, "msvc" | "clang-cl"),
                        "{}: opaque fixture missing",
                        label("opaque", "")
                    );
                }

                checked += 1;
            }
        }
    }
    assert_eq!(checked, 38);
}

#[test]
fn unknown_type_explains_itself() {
    let session =
        libstratum::open_path(&libstratum::default_engine(), fixtures().join("linux-gcc/x86_64/O2/padding/padding"))
            .unwrap();
    let result = session.struct_layout("DoesNotExist").unwrap();
    assert!(result.matches.is_empty());
    assert!(result.not_found_reason.unwrap().contains("optimized-out"));
}

/// Without a dSYM, Mach-O debug info comes from the debug-map objects; with one, objects are not
/// read as well (no duplicate definitions).
#[test]
fn macho_debug_map_objects_are_a_fallback_for_a_missing_dsym() {
    let engine = libstratum::default_engine();
    let original = fixtures().join("apple-clang/arm64/O0/odr_conflict");

    let with_dsym = libstratum::open_path(&engine, original.join("odr_conflict")).unwrap();
    assert_eq!(with_dsym.struct_layout("Config").unwrap().matches.len(), 2, "dSYM only, not dSYM + objects");

    let scratch = std::env::temp_dir().join(format!("libstratum-oso-{}", std::process::id()));
    std::fs::create_dir_all(&scratch).unwrap();
    for file in ["odr_conflict", "a.o", "b.o"] {
        std::fs::copy(original.join(file), scratch.join(file)).unwrap();
    }
    let without_dsym = libstratum::open_path(&engine, scratch.join("odr_conflict")).unwrap();
    let config = without_dsym.struct_layout("Config").unwrap();
    std::fs::remove_dir_all(&scratch).unwrap();
    let mut sizes: Vec<u64> = config.matches.iter().map(|m| m.size_bytes).collect();
    sizes.sort();
    assert_eq!(sizes, vec![16, 32], "{:?}", without_dsym.diagnostics());
    assert!(
        without_dsym.diagnostics().iter().any(|d| d.code == DiagCode::NoDebugInfo && d.message.contains("not found")),
        "the missing dSYM is reported: {:?}",
        without_dsym.diagnostics()
    );
}
