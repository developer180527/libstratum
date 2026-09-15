# 07 — Roadmap (backend)

> Rewritten for [ADR-0017](08-decisions.md#adr-0017-backend-only-scope) (backend only), [ADR-0018](08-decisions.md#adr-0018-debug-info-neutral-ir-dwarf-and-pdb-backends) (DWARF + PDB), and [ADR-0019](08-decisions.md#adr-0019-v1-platform-matrix) (Windows, Linux, macOS, embedded).

**Strategy: prove the abstractions early, then go wide.** The riskiest architectural bet is that one neutral IR serves PE+PDB, ELF+DWARF and Mach-O+DWARF equally well. So the first real feature (layout) is built on **all Tier 1 platforms at once**, before any correlation logic exists that could quietly assume DWARF.

## Phase 0: Design (current)
- [x] Prior art, toolchain research, local verification (macOS)
- [x] Architecture, data model, algorithms, testing, suite, embedded, platforms
- [x] Resolve Q1–Q5 (Q14, Q15 pending M0 Windows work) ([08](08-decisions.md#open-questions))
- [ ] Freeze doc-level shapes of the SPI (`BinaryFormat`, `Image`, `DebugInfoBackend`, `DebugReader`, `ArtifactProvider`) and the neutral IR

## M0: Foundations and verification spikes

**Progress (2026-09-15):** workspace scaffold ✅ · CI workflow ✅ · fixture driver (`fixtures/build/build.py`, all 8 toolchains defined) ✅ · Docker image and fixtures workflow ✅ · `layout/` fixtures built for apple-clang ✅ · spikes S1–S6 ✅ · Linux/embedded fixtures (176 builds on the Debian server) and S8–S10 ✅ · MSVC/clang-cl fixtures (64 builds via GitHub Actions) ✅ · PDB reader choice S7/Q14 ⏳. Report: [spikes/m0-report.md](spikes/m0-report.md).

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
- `libstratum-core`: SPI traits, neutral IR (three address spaces, memory regions, hybrid-image-ready), diagnostics, `Engine`/`Session`, host services, `capabilities()`
- `libstratum-format-pe`: PE32+, RSDS debug directory, ARM64X detection
- `libstratum-format-elf`: hosted + bare metal; Thumb normalization, mapping symbols, `PT_LOAD` LMA/VMA
- `libstratum-format-macho`: thin + universal, `LC_UUID`, debug map
- `libstratum-arch` facts; plugin conformance suite; fuzz targets

**Exit:** every Tier 1 fixture opens with correct identity, sections, and VM/file/load extents.

## M2: Debug-info backends + layout lens (the abstraction proof)
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
