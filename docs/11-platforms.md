# 11 — Platforms: Windows, Linux, Apple, embedded

**Scope decision (2026-09-15):** stratum is a **backend-only low-level observability platform**. v1 must work on the major desktop platforms, **including Windows**, and on embedded targets. All frontends (CLI, IDE, VS Code, GUI) are out of scope until the backend is proven. See [ADR-0017](08-decisions.md#adr-0017-backend-only-scope), [ADR-0018](08-decisions.md#adr-0018-debug-info-neutral-ir-dwarf-and-pdb-backends), [ADR-0019](08-decisions.md#adr-0019-v1-platform-matrix).

## 1. The platform matrix

Two independent axes: the **container format** (how the binary is packaged) and the **debug-info format** (how source ↔ binary facts are encoded). Keeping them separate is what makes Windows and embedded fit one design.

| Platform | Container | Debug info | Toolchains (v1) | Arches (v1) | Tier |
|---|---|---|---|---|---|
| **Windows** | PE/COFF | **PDB** (CodeView) | MSVC (`cl` + `link`), clang-cl + lld-link | x64, arm64 | 1 |
| **Linux** | ELF | DWARF | Clang + lld, GCC + GNU ld/gold/mold | x86_64, aarch64 | 1 |
| **macOS** | Mach-O | DWARF (dSYM or debug map) | Apple Clang + ld-prime | arm64, x86_64 | 1 |
| **Embedded (bare metal / RTOS)** | ELF | DWARF | arm-none-eabi-gcc, LLVM/Arm Toolchain for Embedded | Cortex-M (thumbv6m, thumbv7em, thumbv8m), rv32imac | 1 |
| Android | ELF | DWARF | NDK Clang | aarch64, x86_64 | 2 (nearly free: same as Linux ELF + Clang) |
| iOS / tvOS | Mach-O | DWARF | Apple Clang | arm64 | 2 (nearly free: same as macOS) |
| Windows MinGW | PE/COFF | **DWARF** | MinGW GCC / llvm-mingw | x64 | 2 (tests the format/debug split) |
| Arm Compiler 6, IAR | ELF | DWARF (vendor quirks) | armclang+armlink, IAR ILINK | Cortex-M | 2 |
| Xtensa (ESP32) | ELF | DWARF | GCC | xtensa | 3 |
| Consoles | ELF-like / PE | DWARF / PDB | Platform SDKs (under NDA) | — | Private plugins; see §6 |

**Tier 1:** fixtures and goldens in CI, a release blocker. **Tier 2:** architecture-ready, fixtures added opportunistically. **Tier 3:** plugin welcome, not planned.

## 2. Windows in depth

### 2.1 Binary: PE/COFF
- Read with the `object` crate (PE32+ and COFF objects are supported 📚). Sections: `.text`, `.rdata`, `.data`, `.bss` (often folded into `.data` with `VirtualSize > SizeOfRawData`), `.pdata`/`.xdata` (x64 and arm64 unwind), `.reloc`, `.tls`, `.idata`/`.edata`.
- **VM vs file sizes** map directly onto `VirtualSize` vs `SizeOfRawData`, so the two-space model works unchanged.
- **The PE symbol table is empty in linked images.** Symbols come from the PDB (publics and procedure records), so on Windows the debug-info backend is also the symbol source.
- **Identity:** the debug directory holds a CodeView `RSDS` record with a **GUID + age** + PDB path. The PDB's info stream has the same GUID/age. A mismatch → `debug-info-mismatch` (ADR-0011) 📚.
- **Hybrid Arm binaries:** **ARM64EC** (Arm code with x64 compatibility; COFF machine says AMD64) and **ARM64X** (ARM64 + ARM64EC in one file; COFF machine says ARM64). They're detected through the load config → CHPE metadata 📚. ARM64X is conceptually like a Mach-O universal binary: `probe` returns `YesContainer`, and the view (arm64 or arm64ec) is selected through `OpenOptions`. ARM64EC is **Tier 2**, but the IR must not assume "one machine type per image".

### 2.2 Debug info: PDB
A PDB is an MSF container (a mini filesystem of streams) holding CodeView records 📚:

| PDB stream | Contents | stratum use |
|---|---|---|
| PDB info | GUID, age, named streams | Identity check |
| **TPI** (type info) | `LF_STRUCTURE`/`LF_CLASS`/`LF_UNION` + `LF_FIELDLIST` (`LF_MEMBER` offsets, `LF_BCLASS`/`LF_VBCLASS` bases, `LF_VFUNCTAB` vptr, `LF_BITFIELD`) | **Layout lens.** Equivalent of DWARF type DIEs |
| **IPI** (id info) | Function ids, `LF_FUNC_ID`/`LF_MFUNC_ID`, build info, string ids | Inlinee identities, source file names |
| **DBI** | Module list (one per `.obj`/library member), section map, section contributions | Per-module (compile unit) attribution, object-file provenance, **section contributions ≈ the linker map** |
| Module streams | Symbols (`S_GPROC32`, `S_LPROC32`, `S_BLOCK32`, **`S_INLINESITE`** with binary annotations), C13 subsections: **`DEBUG_S_LINES`**, **`DEBUG_S_INLINEELINES`**, file checksums | **Correlation lens:** line tables and inline chains |
| Globals / publics | Global data, public symbols with RVAs | Symbol lens (the PE has no linked symtab) |

**Inline chains on Windows:** `S_INLINESITE` records nest inside procedures. Their *binary annotations* are a compact state machine encoding code offsets and line deltas, while `DEBUG_S_INLINEELINES` gives each inlinee's source file and starting line 📚. This is semantically equivalent to DWARF `DW_TAG_inlined_subroutine` + `DW_AT_call_*`, with the same shape (a nested tree) and different encoding. It fits the debug IR (§4) directly.

**Reader choice:** `ms-pdb` (Microsoft's `pdb-rs`, actively maintained, pure Rust, no Windows or DIA requirement) vs `pdb2` (maintained fork of the older `pdb` crate used by Sentry) 📚. Leaning `ms-pdb` (see Q14). **We must not depend on DIA** (the Windows-only COM API that SizeBench uses 📚): stratum has to analyze Windows binaries on Linux and macOS hosts too (CI, cross-builds, agents).

### 2.3 MSVC toolchain behaviors that affect correlation

| Behavior | Detail | Consequence |
|---|---|---|
| `/Zi` vs `/Z7` | `/Zi`: separate PDB per compile, merged by the linker. `/Z7`: CodeView embedded in each `.obj` 📚 | The final PDB from `link /DEBUG` is complete either way. `/Z7` objects are a "debug info in objects" source, like Mach-O `.o` |
| `/DEBUG:FASTLINK` | Partial PDB that references `.obj`/`.lib` debug info instead of copying it; **deprecated and removed in Visual Studio 2026** 📚 | Detect and emit a diagnostic (`pdb-fastlink-partial`). Don't implement resolution unless demanded |
| **`/OPT:ICF` and `/OPT:REF`** | On by default in release links, **but `/DEBUG` switches the defaults to NOICF/NOREF** unless re-specified 📚. Many shipping builds pass `/DEBUG /OPT:REF /OPT:ICF` | ICF is common in shipped Windows binaries. Folded functions share an RVA across multiple public/proc symbols → `Folded` provenance. "Identical inline function info gets removed from the PDB" 📚, so folds are partially invisible, and we report that |
| `/Gy` (function-level linking) | Required for per-function REF/ICF | Producer flags come from the PDB build info (`LF_BUILDINFO`) → diagnostics |
| `/MAP` | Linker map text: sections, publics with `Rva+Base` and `Lib:Object` | Optional `MapFileParser` (`msvc-link`). DBI section contributions already cover most of it |
| LTCG (`/GL` + `/LTCG`) | Whole-program optimization; heavy cross-module inlining | Same honesty rules as Clang LTO |
| Class layout | `/d1reportSingleClassLayoutX` (undocumented, compile-time text) 📚 | Cross-check tool for layout goldens only; never a data source |
| Vectorization reports | `/Qvec-report:2`, `/Qpar-report:2`: reason codes per loop 📚 | Future `ArtifactProvider` (opt-report lens, MSVC flavor) |
| Build time | C++ Build Insights (ETW), `vcperf` 📚 | Future provider (build-cost lens) |

### 2.4 Symbol lookup on Windows
`FileLocator` gains a `SymbolServerLocator`:
1. PDB path from the `RSDS` record (absolute, then next to the image)
2. `_NT_SYMBOL_PATH`-style search paths (**read by frontends, passed in**; the library never reads env vars, per rule 6)
3. Symbol store layout: `<store>/<pdbname>/<GUID><AGE>/<pdbname>`, served over HTTP as the same path 📚. **The library defines the layout; the host provides the transport** (no HTTP client inside the core).

This also serves the crash-reporting and symbol-archive use case (addresses from any build resolved by GUID/age or build ID).

### 2.5 Names
- MSVC decoration (`?update@MovementSystem@@QEAAXM@Z`) needs its own demangler: `msvc-demangler` (Rust port of `undname` behavior 📚).
- **Consequence:** demangling is a **mangling-scheme** concern (Itanium vs MSVC), not a language concern. clang-cl emits MSVC mangling for C++ code. `LanguageSupport` is split into `LanguageSupport` (C/C++ semantics: CU filtering, type-name normalization) and `Demangler` (scheme detected from the symbol prefix: `_Z`/`__Z` → Itanium, `?` → MSVC).
- PDB type names are already fully qualified with template arguments (`std::vector<int,std::allocator<int> >`). Name normalization must make MSVC and Clang spellings comparable (spacing, `>>`, `__int64` vs `long long`) for cross-platform diffs.

### 2.6 Building Windows fixtures
- **MSVC fixtures require real MSVC** → Windows CI runners (e.g. GitHub Actions `windows-2025` with Visual Studio), outputs committed like every other fixture.
- **clang-cl + lld-link fixtures can be cross-built on macOS/Linux** using a downloaded Windows SDK and CRT (e.g. the `xwin` tool) ❓ verify licensing and reproducibility in M0.
- Cross-check tools: `llvm-pdbutil dump` (any host), `dia2dump`/`cvdump` (Windows), `/d1reportSingleClassLayout` for layouts.

## 3. Other platforms: what's specific

- **Linux:** DWARF on ELF, split DWARF (`.dwo`/`.dwp`), debuglink and build-ID lookup, **debuginfod** (HTTP symbol server keyed by build ID; same host-provides-transport rule as Windows symbol servers). GCC is Tier 1 now, so GCC LTO early debug, `-fipa-icf` and legacy bitfield encoding (already designed in, [04 §5](04-formats-and-toolchains.md#5-clang-vs-gcc)) must be covered by fixtures in v1.
- **macOS:** as documented in [04 §3](04-formats-and-toolchains.md#3-mach-o) (dSYM vs debug map, ld-prime map, dead-stripped DIE loss).
- **Embedded:** as documented in [10](10-embedded.md): Thumb bit, LMA/VMA and memory regions (ADR-0016, now accepted), GCC stack usage files, SVD.

## 4. Architecture change: debug-info-neutral IR

The original design centered the core on DWARF (`libstratum-core/dwarf`, `DebugSource::dwarf_section`, `gimli::SectionId` in the SPI). **With PDB as a Tier 1 format, that has to change now.** Retrofitting it after correlation is written against DWARF would be exactly the expensive architectural debt we're trying to avoid.

### 4.1 New layering

```
 Container plugins          Debug-info backends              Core (format- and debug-format-agnostic)
 ─────────────────          ───────────────────              ─────────────────────────────────────────
 format-elf   ─┐            debug-dwarf (gimli)  ─┐
 format-macho ─┼─ Image ──► debug-pdb (ms-pdb)   ─┼─ DebugIr ──► correlate ──► query ──► model
 format-pe    ─┘            (future: others)     ─┘
```

- A container plugin says **where** debug info lives (`DebugLocation`: embedded sections, dSYM, OSO objects, `.dwo`, PDB path + GUID/age, `/Z7` objects, …).
- A **debug-info backend** turns one location into the neutral **`DebugIr`**.
- Correlation, queries and all lenses consume **only** `DebugIr`.

### 4.2 The `DebugInfoBackend` SPI

```rust
pub trait DebugInfoBackend: Send + Sync + 'static {
    fn id(&self) -> &'static str;                                   // "dwarf", "pdb"
    fn accepts(&self, loc: &DebugLocation) -> bool;
    fn open(&self, loc: &DebugLocation, img: &dyn Image, host: &HostServices)
        -> Result<Box<dyn DebugReader>, SpiError>;
}

/// Lazy, per-unit access. Every method is cheap to call repeatedly (memoized by the core).
pub trait DebugReader: Send + Sync {
    fn identity(&self) -> ImageIdentity;
    fn units(&self) -> Result<Vec<ir::UnitInfo>, SpiError>;          // CU / PDB module: name, language, producer, address ranges
    fn address_to_unit(&self, addr: u64) -> Option<ir::UnitId>;

    // types (layout lens)
    fn find_types(&self, name: &ir::NameQuery) -> Result<Vec<ir::TypeRef>, SpiError>;
    fn type_layout(&self, ty: ir::TypeRef) -> Result<ir::RawLayout, SpiError>;    // members, bases, vptr, bitfields: raw facts, no derived holes

    // functions, inlining, lines (correlation lens)
    fn functions(&self, unit: ir::UnitId) -> Result<Vec<ir::FunctionInfo>, SpiError>;
    fn inline_tree(&self, func: ir::FunctionRef) -> Result<ir::InlineTree, SpiError>;
    fn line_table(&self, unit: ir::UnitId) -> Result<ir::LineTable, SpiError>;     // normalized rows: addr, file, line, col, is_stmt
    fn discarded_functions(&self, unit: ir::UnitId) -> Result<Vec<ir::DiscardEvidence>, SpiError>; // tombstones, FASTLINK gaps, …

    // symbols (for formats whose image has none: PE)
    fn symbols(&self) -> Result<Option<Vec<ir::Symbol>>, SpiError>;
}
```

The core derives everything else (holes, cache lines, reorder suggestions, provenance merging, ICF groups, source → binary indexes) **once**, for all debug formats.

### 4.3 What had to be generalized

| DWARF-specific concept (old) | Neutral concept (new) | PDB equivalent |
|---|---|---|
| Compile unit (CU) | `Unit` | DBI module |
| DIE offset `DieRef` | Opaque `DebugRef { backend, unit, key: u64 }` (global, cross-unit capable) | TPI/IPI type index, symbol record offset |
| `DW_TAG_inlined_subroutine` + `DW_AT_call_*` | `InlineTree` nodes `{ callee, call_site, ranges }` | `S_INLINESITE` + binary annotations + `DEBUG_S_INLINEELINES` |
| Line program | `LineTable` rows | C13 `DEBUG_S_LINES` |
| `DW_AT_data_member_location` / bit offsets | `RawLayout` entries in bits | `LF_MEMBER` offset, `LF_BITFIELD` |
| `DW_LANG_*` | `SourceLanguage` enum | `S_COMPILE3` language field |
| Producer string | `Producer { compiler, version, flags? }` | `S_COMPILE3` + `LF_BUILDINFO` |
| Tombstoned `low_pc` | `DiscardEvidence` | (absent functions; ICF/REF gaps) |
| `Evidence::Dwarf` | `Evidence::DebugInfo { backend, unit, key }` | same |

`gimli` and `ms-pdb` types now appear **only** inside their backend crates. Rule 5 in [02 §2](02-architecture.md#dependency-rules-enforced-in-ci-eg-via-cargo-deny-or-a-workspace-check) is tightened: no parser types in **any** SPI or public signature.

### 4.4 Crate map (updated)

| Crate | Role |
|---|---|
| `libstratum-model` | Public types |
| `libstratum-core` | SPI, neutral IR, correlation, lenses, cache, diagnostics |
| `libstratum-format-elf` · `libstratum-format-macho` · `libstratum-format-pe` | Container plugins (+ their map parsers and locators) |
| `libstratum-debug-dwarf` · `libstratum-debug-pdb` | Debug-info backends |
| `libstratum-lang-cpp` | C/C++ semantics |
| `libstratum-demangle` | Itanium + MSVC schemes |
| `libstratum-arch` | Arch facts: Thumb bit, mapping symbols, pointer size, ABI alignment tables for reorder suggestions |
| `libstratum` | Facade, default plugin set via features |

Frontend crates (`-cli`, `-ffi`, `-server`) are removed from the plan until the backend ships ([ADR-0017](08-decisions.md#adr-0017-backend-only-scope)). The `Request`/`Response` model stays, because it's backend API surface and costs nothing to keep.

## 5. How the backend gets exercised without frontends

Backend-only doesn't mean untested or unusable:
1. **Integration tests** over the fixture corpus (goldens are `libstratum-model` JSON).
2. **`examples/`** in the facade crate: small Rust programs (`layout.rs`, `addr.rs`, `summary.rs`) that act as API documentation and quick manual checks. They are not a product CLI.
3. **Agent evaluation harness** ([09 §6](09-suite.md#6-the-agent-evaluation-proves-the-suites-value)) runs against the Rust API directly.
4. **Dogfooding on real codebases:** your game engine (Windows + macOS builds) and at least one embedded firmware project.

## 6. Consoles and other NDA platforms

Console toolchains are under NDA, so their specifics can't live in a public repository. The plugin architecture handles this: a studio can write a private `libstratum-format-<console>` or `libstratum-debug-<console>` crate against the public SPI and register it with the builder, **with no fork**. The public repo only has to keep the SPI general (hybrid images, multiple debug locations, host-provided transports), which the design already does.

## Sources

- PE/PDB matching and symbol servers: https://learn.microsoft.com/en-us/windows/win32/debug/using-symsrv · https://learn.microsoft.com/en-us/windows-hardware/drivers/debugger/advanced-symsrv-use · https://randomascii.wordpress.com/2013/03/09/symbols-the-microsoft-way/
- PDB format: https://llvm.org/docs/PDB/index.html · https://github.com/microsoft/microsoft-pdb · https://github.com/PascalBeyer/PDB-Documentation · https://devblogs.microsoft.com/cppblog/whats-inside-a-pdb-file/
- Rust PDB readers: https://github.com/microsoft/pdb-rs · https://crates.io/crates/ms-pdb · https://crates.io/crates/pdb2 · https://github.com/getsentry/pdb
- MSVC demangling: https://github.com/mstange/msvc-demangler-rust
- `/DEBUG`, FASTLINK removal: https://learn.microsoft.com/en-us/cpp/build/reference/debug-generate-debug-info?view=msvc-170
- `/Z7` `/Zi`: https://learn.microsoft.com/en-us/cpp/build/reference/z7-zi-zi-debug-information-format?view=msvc-170
- `/OPT:ICF` and `/OPT:REF` defaults: https://learn.microsoft.com/en-us/cpp/build/reference/opt-optimizations?view=msvc-170 · https://www.atmosera.com/blog/correctly-creating-native-c-release-build-pdbs/
- ARM64EC and ARM64X: https://learn.microsoft.com/en-us/windows/arm/arm64x-pe · https://lief.re/blog/2025-02-16-arm64ec-pe-support/
- SizeBench architecture (DIA + PE parser): https://github.com/microsoft/SizeBench/blob/main/docs/Solution%20Architecture.md
- MSVC reports: https://learn.microsoft.com/en-us/cpp/build/reference/qvec-report-auto-vectorizer-reporting-level?view=msvc-170 · https://github.com/microsoft/vcperf · class layout flag: https://ofekshilon.com/2010/11/07/d1reportallclasslayout-dumping-object-memory-layout/
