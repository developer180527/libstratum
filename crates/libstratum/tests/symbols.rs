//! Symbol lens on every fixture binary: invariants, plus sizes and addresses cross-checked against
//! the linker's own map (ld-prime for Mach-O, MSVC/lld-link /MAP for PE).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use libstratum::model::{Evidence, SymbolFilter, SymbolKind};

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

/// ld-prime `-map`: `0xADDRESS\t0xSIZE\t[ n] name` under `# Symbols:`.
fn ld_prime_sizes(map: &Path) -> HashMap<String, (u64, u64)> {
    let text = std::fs::read_to_string(map).unwrap_or_default();
    let mut out = HashMap::new();
    let mut in_symbols = false;
    for line in text.lines() {
        if line.starts_with("# Symbols:") {
            in_symbols = true;
            continue;
        }
        if line.starts_with("# Dead Stripped") {
            break;
        }
        let fields: Vec<&str> = line.splitn(3, '\t').collect();
        if in_symbols && fields.len() == 3 && fields[0].starts_with("0x") {
            let name = fields[2].split_once("] ").map_or(fields[2], |(_, n)| n).to_owned();
            let parse = |s: &str| u64::from_str_radix(s.trim_start_matches("0x"), 16).unwrap_or(0);
            out.insert(name, (parse(fields[0]), parse(fields[1])));
        }
    }
    out
}

/// MSVC / lld-link `/MAP` publics: ` 0001:00000000       main       0000000140001000 f   padding.obj`.
fn msvc_map_addresses(map: &Path) -> HashMap<String, u64> {
    let text = std::fs::read_to_string(map).unwrap_or_default();
    let mut out = HashMap::new();
    for line in text.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() >= 3
            && fields[0].len() == 13
            && fields[0].as_bytes()[4] == b':'
            && fields[2].len() == 16
            && let Ok(address) = u64::from_str_radix(fields[2], 16)
        {
            out.insert(fields[1].to_owned(), address);
        }
    }
    out
}

#[test]
fn symbol_tables_are_consistent_and_match_linker_maps() {
    let engine = libstratum::default_engine();
    let (mut checked, mut map_sizes, mut map_addresses) = (0, 0, 0);
    for (toolchain, dir, binary) in binaries() {
        let label = binary.strip_prefix(fixtures()).unwrap().display().to_string();
        let session = libstratum::open_path(&engine, &binary).unwrap();
        let symbols = session.symbols(&SymbolFilter::default());
        assert!(!symbols.is_empty(), "{label}: no symbols");
        let sections = session.image().sections();

        for pair in symbols.windows(2) {
            assert!(pair[0].address <= pair[1].address, "{label}: symbols not in address order");
        }
        for s in &symbols {
            // Sized symbols lie inside their section's VM range (section symbols are excluded).
            if let (Some(size), Some(key)) = (s.size, &s.section) {
                let section = sections.iter().find(|x| x.name == key.name && x.segment == key.segment).unwrap();
                if let Some(vm) = section.extent.vm {
                    assert!(
                        s.address >= vm.start && s.address + size <= vm.end(),
                        "{label}: {} [{:#x}+{size}] outside {} [{:#x}, {:#x})",
                        s.raw_name,
                        s.address,
                        key.name,
                        vm.start,
                        vm.end()
                    );
                }
            }
            assert_eq!(s.size.is_some(), s.size_evidence.is_some(), "{label}: {} size without evidence", s.raw_name);
            // Aliases are symmetric.
            for alias in &s.aliases {
                let other = symbols.iter().find(|o| &o.raw_name == alias && o.address == s.address).unwrap();
                assert!(other.aliases.contains(&s.raw_name), "{label}: asymmetric alias {} / {alias}", s.raw_name);
            }
        }

        // `main` exists with a size that doesn't come from the heuristic.
        let main = session.by_symbol("main");
        let main =
            main.matches.iter().find(|m| m.kind == SymbolKind::Function).unwrap_or_else(|| panic!("{label}: main"));
        if toolchain == "apple-clang-lto" {
            // ThinLTO objects are bitcode and no LTO object was kept: no DWARF, no nlist sizes. The
            // heuristic is the honest answer until a link map is attached (slice 3).
            assert!(
                matches!(main.size_evidence, Some(Evidence::Heuristic { .. })),
                "{label}: {:?}",
                main.size_evidence
            );
        } else {
            assert!(
                matches!(main.size_evidence, Some(Evidence::SymbolTable | Evidence::DebugInfo { .. })),
                "{label}: main size evidence {:?}",
                main.size_evidence
            );
        }

        let stem = binary.file_stem().unwrap().to_string_lossy().into_owned();
        let map = dir.join(format!("{stem}.map"));
        if toolchain.starts_with("apple-clang") {
            // ld-prime sizes are atom sizes: the object plus the alignment padding up to the next atom
            // (`_g_outer` is 16 bytes, its atom 24 because the next symbol is 32-byte aligned). So a
            // debug-info size equals the atom size, or is smaller and the atom ends exactly at the next
            // symbol in the map.
            let linker = ld_prime_sizes(&map);
            let mut starts: Vec<u64> = linker.values().map(|&(a, _)| a).collect();
            starts.sort_unstable();
            for s in symbols.iter().filter(|s| matches!(s.size_evidence, Some(Evidence::DebugInfo { .. }))) {
                if let Some(&(address, atom)) = linker.get(&s.raw_name) {
                    let size = s.size.unwrap();
                    assert_eq!(s.address, address, "{label}: {} address vs ld-prime map", s.raw_name);
                    let next = starts.iter().find(|&&a| a > address).copied();
                    assert!(
                        size == atom || (size < atom && next == Some(address + atom)),
                        "{label}: {} size {size} vs ld-prime atom {atom} (next symbol {next:x?})",
                        s.raw_name
                    );
                    map_sizes += 1;
                }
            }
        }
        if matches!(toolchain.as_str(), "msvc" | "clang-cl") {
            // Addresses from PDB publics must equal the linker map's Rva+Base.
            let linker = msvc_map_addresses(&map);
            for s in &symbols {
                if let Some(&address) = linker.get(&s.raw_name) {
                    assert_eq!(s.address, address, "{label}: {} vs /MAP", s.raw_name);
                    map_addresses += 1;
                }
            }
        }
        checked += 1;
    }
    assert!(checked >= 381, "every fixture binary checked, got {checked}");
    assert!(map_sizes > 100, "ld-prime size cross-checks: {map_sizes}");
    assert!(map_addresses > 1000, "/MAP address cross-checks: {map_addresses}");
    eprintln!("checked {checked} binaries; {map_sizes} sizes vs ld-prime; {map_addresses} addresses vs /MAP");
}

#[test]
fn data_symbols_are_sized_from_debug_info() {
    let engine = libstratum::default_engine();
    for (binary, name) in [
        ("msvc/x64/O2/padding/padding.exe", "g_packet"),
        ("clang-cl/arm64/O2/padding/padding.exe", "g_packet"),
        ("apple-clang/arm64/O2/padding/padding", "g_packet"),
        ("linux-gcc/x86_64/O2/padding/padding", "g_packet"),
    ] {
        let session = libstratum::open_path(&engine, fixtures().join(binary)).unwrap();
        let result = session.by_symbol(name);
        let symbol = result.matches.first().unwrap_or_else(|| panic!("{binary}: {name}"));
        assert_eq!(symbol.size, Some(24), "{binary}");
        assert!(
            !matches!(symbol.size_evidence, Some(Evidence::Heuristic { .. })),
            "{binary}: {:?}",
            symbol.size_evidence
        );
    }
}
