# 07 — Roadmap (backend)

> Rewritten for [ADR-0017](08-decisions.md#adr-0017-backend-only-scope) (backend only), [ADR-0018](08-decisions.md#adr-0018-debug-info-neutral-ir-dwarf-and-pdb-backends) (DWARF + PDB), and [ADR-0019](08-decisions.md#adr-0019-v1-platform-matrix) (Windows, Linux, macOS, embedded).

**Strategy: prove the abstractions early, then go wide.** The riskiest architectural bet is that one neutral IR serves PE+PDB, ELF+DWARF and Mach-O+DWARF equally well. So the first real feature (layout) is built on **all Tier 1 platforms at once**, before any correlation logic exists that could quietly assume DWARF.

## Phase 0: Design (current)
- [x] Prior art, toolchain research, local verification (macOS)
- [x] Architecture, data model, algorithms, testing, suite, embedded, platforms
- [x] Resolve Q1–Q5 and Q14 (Q15 moot: Windows fixtures come from GitHub Actions) ([08](08-decisions.md#open-questions))
- [ ] Freeze doc-level shapes of the SPI (`BinaryFormat`, `Image`, `DebugInfoBackend`, `DebugReader`, `ArtifactProvider`) and the neutral IR

## M0: Foundations and verification spikes

**Progress (2026-09-15):** workspace scaffold ✅ · CI workflow ✅ · fixture driver (`fixtures/build/build.py`, all 8 toolchains defined) ✅ · Docker image and fixtures workflow ✅ · `layout/` fixtures built for apple-clang ✅ · spikes S1–S6 ✅ · Linux/embedded fixtures (176 builds on the Debian server) and S8–S10 ✅ · MSVC/clang-cl fixtures (64 builds via GitHub Actions) ✅ · PDB reader choice S7/Q14 ✅ (`pdb2`). **M0 complete.** Report: [spikes/m0-report.md](spikes/m0-report.md).

- Cargo workspace with all backend crates (empty, compiling); dependency-rule check in CI
- **CI runners:** macOS (Apple Clang), Linux (Docker: Clang, GCC, arm-none-eabi, RISC-V toolchains), **Windows (MSVC, clang-cl)**
- Fixture build system per platform, pinned toolchains, committed outputs
- First fixtures: `layout/` on every Tier 1 platform × toolchain × arch
- **Spikes** (results recorded in 04/10/11; 📚/❓ become ✅):
  - PDB reader choice (Q14): TPI layouts, `S_INLINESITE`, C13 lines on a real MSVC `/O2 /DEBUG /OPT:ICF` build
  - `/OPT:ICF` visibility in PDB publics and procs
  - lld ICF aliases and tombstones; GCC `-fipa-icf`; GCC LTO early debug
  - ld-prime `-deduplicate`; Mach-O LTO object in the debug map
  - Thumb bit and mapping symbols on arm-none-eabi ELF; `.data` LMA/VMA from `PT_LOAD`
  - `addr2line` reuse vs our own inline resolution over `DebugIr` (Q1)

**Exit:** CI green on three host OSes; fixtures committed; spike report merged.

## M1: Containers and IR (no debug info)

**Progress (2026-09-16):** ELF, Mach-O (thin + universal) and PE plugins parse sections, segments, symbols, identity and
debug locations ✅ · shared conformance suite (`libstratum-core` feature `conformance`) ✅ · all 272 fixtures pass it,
plus Thumb, flash/RAM, dSYM-UUID, universal-slice and RSDS-vs-PDB identity tests ✅ · public-API end-to-end test ✅.
Also done: `FsLocator` (dSYM by UUID next to the binary, debug-map objects incl. `lib.a(member.o)`, PDB via recorded
path / next to the image / SymSrv-layout stores, `.dwo`/`.dwp`, debuglink with CRC check, build-id trees, prefix maps;
Windows paths resolve on any host) ✅ · `Input::File` + `DebugOpenContext` so backends locate companions ✅ ·
`FileSource` default and opt-in `MmapSource` (ADR-0021) ✅ · `fuzz/` with an `open_any` target seeded from fixtures ✅.
**M1 complete.** Moved: content hash → M5 (cache); ARM64X view selection → Tier 2.

Findings while implementing (encoded in the plugins, checked by tests):
- GNU ld attaches linker-script symbols defined outside any output section (e.g. `__stack_top`) to an arbitrary
  section; labels whose address lies outside their section are treated as absolute.
- Mach-O `__mh_execute_header` claims section 1 but points at the Mach header; same rule.
- Linked PE images have no COFF symbols; symbols come from the PDB backend. IR addresses are absolute VAs, so
  `Image::image_base()` was added for RVA-based debug formats (default 0).
- `DebugLocation::Dsym` carries the image UUID instead of a path: the image doesn't know where it lives, so the host
  locator finds the bundle and checks the UUID.
- The executable's RSDS age matches the PDB's DBI-stream age (verified against pdb2 on all 64 Windows fixtures).
- Fuzzing (`fuzz/open_any`, seeded with one fixture per format) found two ELF bugs within minutes: an arithmetic
  overflow in the load-address computation for hostile section headers, and a **decompression bomb** (an 8.8 KB file
  declaring a gigabyte-sized compressed debug section; 4.7 GB resident). Fixed with checked arithmetic and a 1100:1
  compression-ratio cap; both inputs live in `fuzz/regressions/` and run in the normal test suite. A following
  5-minute run: 5.18 M executions, no findings.
- `libstratum-core`: SPI traits, neutral IR (three address spaces, memory regions, hybrid-image-ready), diagnostics, `Engine`/`Session`, host services, `capabilities()`
- `libstratum-format-pe`: PE32+, RSDS debug directory, ARM64X detection
- `libstratum-format-elf`: hosted + bare metal; Thumb normalization, mapping symbols, `PT_LOAD` LMA/VMA
- `libstratum-format-macho`: thin + universal, `LC_UUID`, debug map
- `libstratum-arch` facts; plugin conformance suite; fuzz targets

**Exit:** every Tier 1 fixture opens with correct identity, sections, and VM/file/load extents.

## M2: Debug-info backends + layout lens (the abstraction proof)

**Progress (2026-09-16):** DWARF backend (ELF embedded, dSYM, debug-map objects as fallback, debuglink/build-id) ✅ ·
PDB backend via `pdb2` (TPI classes/unions, bitfields, bases, virtual bases, vfptr; GUID/age identity) ✅ · core layout lens
(holes, tail padding, packing, cache lines, reorder suggestions, ODR conflicts, deterministic order) ✅ · cross-toolchain
C/C++ name normalization ✅ · `Session::struct_layout` + `examples/layout.rs` ✅ · **abstraction proof: one test asserts the
same ABI facts on all 272 fixtures (DWARF + PDB, 8 toolchains)** ✅ · 272 committed goldens (`fixtures/golden`) ✅.
Remaining: automated cross-checks against `llvm-dwarfdump`/`pahole`/`llvm-pdbutil`, type units (`DW_AT_signature`),
declaration-only hints, source-path normalization in `decl`, regrouping PDB-flattened anonymous unions.

Findings:
- GCC spells integer types out in DWARF names (`SmallArray<short int, 3>`), MSVC uses `__int64`-style names and
  `> >`: normalization canonicalizes keyword runs and punctuation spacing.
- The same source has different layouts per ABI, and the lens shows it: `Flags` is 16 bytes on Itanium ABIs but 32 on
  MSVC (bitfields with different underlying types start new storage units there).
- Types with virtual bases hide bytes from their member lists (virtual base subobjects everywhere; MSVC's implicit vbptr),
  so holes and padding aren't computed for them; a note says so instead of reporting phantom padding.
- Base-class member sizes are full object sizes, but derived members can reuse a base's tail padding (Itanium `Diamond`:
  `d` at 28 inside `VRight`'s 16 bytes); overlap is expected, not an error.
- PDB flattens anonymous unions into the enclosing struct's member list.
- `libstratum-debug-dwarf`: unit index, type index, raw layouts; ELF embedded, dSYM, OSO objects
- `libstratum-debug-pdb`: PDB locator (RSDS path, search paths, symbol-store layout), GUID/age check, TPI raw layouts, module list
- `libstratum-lang-cpp`, `libstratum-demangle` (Itanium + MSVC), cross-toolchain type-name normalization
- Core layout lens: holes, bitfields, bases, vptr, cache lines, reorder suggestions with per-ABI alignment tables (Win64, SysV x86_64, AAPCS64, Apple arm64, AAPCS32, RV32)

**Exit:** the same C++ fixture yields equivalent layout goldens on MSVC/PDB, Clang/ELF, GCC/ELF, Apple Clang/Mach-O and Cortex-M, and cross-checks agree (`llvm-pdbutil`, `/d1reportSingleClassLayout`, `dwarfdump`, `pahole`). **If the IR leaks PDB- or DWARF-specific concepts here, stop and fix the IR before M3.**

## M3: Symbols, sections, sizes, memory regions
- Symbol sources: ELF symtab, Mach-O nlist + map sizes, PDB publics/procs (PE)
- Summaries across VM/file/load, grouped `region > section > unit > symbol`, with utilization
- ICF groups (lld, `/OPT:ICF`, ld-prime), instantiation groups
- Map parsers: GNU ld (incl. `Memory Configuration`), lld, ld-prime, MSVC `/MAP`; linker-script `MEMORY` parser

**Exit:** summary invariants hold everywhere; embedded flash/RAM numbers match `arm-none-eabi-size` and the map.

## M4: Correlation lens
- DWARF: line tables, inline trees, split DWARF, OSO address translation, tombstones
- PDB: C13 lines, `S_INLINESITE` binary annotations, `DEBUG_S_INLINEELINES`
- Core: `by_address`, `by_source_line`, provenance merging, elimination classification (with reference builds)
- Embedded fault decoding (PC/LR + Thumb + inline chains)

**Exit:** inline and elimination goldens on every Tier 1 platform; cross-checks against `llvm-symbolizer`, `atos`, `addr2line -i`.

## M5: Snapshots, diff, cache, performance
- `Snapshot`, `diff` (layout and size, cross-toolchain names)
- `CacheStore`, content-hash keys (image + PDB/dSYM/.dwo)
- Benchmarks: multi-GB PDB (e.g. the game engine or Chromium), LLVM debug build, firmware images
- `Request`/`Response` dispatcher, generated JSON Schemas, `cargo-semver-checks` baseline

**Exit: backend v0.1.** All Tier 1 platforms; lenses: layout, symbols/size/regions, correlation, diff.

## Real-code validation (starts at M2, gates the CLI) — [ADR-0020](08-decisions.md#adr-0020-delivery-sequence-library--real-code-validation--cli)
- **Game engine** (`~/Developer/engine`, C++/CMake): macOS (Apple Clang, Mach-O) and Windows (MSVC, PE/PDB) builds; debug, release and shipping configurations
  - M2: layouts of engine types (ECS components, math types, render structs) cross-checked by hand
  - M3: size/section/symbol summaries of the engine binaries; ICF in the MSVC release build
  - M4: address → source on real crash addresses and profiler samples
  - M5: open/query performance on the full engine PDB and dSYM
- **Firmware project** (Cortex-M, arm-none-eabi-gcc): flash/RAM regions, Thumb, fault decoding
- Every surprise becomes a fixture and a golden; IR changes go through an ADR

**Exit:** no open correctness issues on either codebase; benchmarks within targets.

## CLI (`libstratum-cli`): the first frontend
- For humans **and** LLMs: JSON by default when stdout isn't a TTY, pretty tables otherwise; `schema_version` in all output; stable exit codes
- Commands map 1:1 to the `Request` model: `info`, `capabilities`, `layout`, `types`, `symbol`, `addr`, `line`, `summary`, `regions`, `snapshot`, `diff`
- Frontend-only concerns live here: config files, env vars (`_NT_SYMBOL_PATH`, `DEBUGINFOD_URLS`), symbol-server downloads, cache directory
- CI helper: `stratum check --budget budgets.toml`

**Exit:** CLI 0.1; the agent evaluation runs through the CLI as well as the Rust API.

## After backend v0.1 (order to be decided)

| Item | Notes |
|---|---|
| Lenses | Optimization report (LLVM remarks, MSVC `/Qvec-report`) → Codegen (disassembler plugin) → ABI diff → Stack (`.su`, `.stack_sizes`, call graph) → Build cost (`-ftime-trace`, C++ Build Insights) → Hot-path layout |
| Embedded extras | SVD register check, ISR audit, worst-case stack |
| Runtime providers | Engine telemetry (allocations, profiler zones) joined by symbol, type and address |
| Tier 2 platforms | Android, iOS, MinGW, armclang, IAR, ARM64EC/ARM64X |
| Further frontends | MCP, FFI, server/LSP, GUI, game-engine observability IDE: after the CLI |
| Agent evaluation | Continuous from M2 onward, through the Rust API |
