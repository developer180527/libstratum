# 08 — Decision log

Architecture Decision Records. **Status values:** Accepted · Proposed (needs review) · Superseded.
To change an accepted decision, add a new ADR that supersedes it. Don't edit history.

---

## ADR-0001: Rust, built on gimli and object
**Status:** Accepted
**Context:** The engine needs zero-copy DWARF parsing, multi-format binary parsing, memory safety on untrusted-ish files, easy embedding (C ABI), and good parallelism.
**Decision:** Implement in Rust. Use `gimli` for DWARF, `object` for container formats, `cpp_demangle` for Itanium names. Evaluate `addr2line` for inline frames.
**Consequences:** Mature, dual-licensed (MIT/Apache-2.0) dependencies that are used in production by rustc backtraces and Sentry's symbolic. `object` targets trusted inputs, so we add limits and fuzzing ourselves. C++ hosts need the future FFI crate.

## ADR-0002: Struct layout comes from DWARF, not libclang
**Status:** Accepted; amended by ADR-0018 ("DWARF" now means "the build's debug info: DWARF or PDB")
**Context:** Options: (a) `clang -fdump-record-layouts` text scraping, (b) libclang/libtooling AST, (c) DWARF from the build.
**Decision:** (c) DWARF.
**Consequences:**
- ➕ Reflects what the compiler actually emitted for the real flags and target; compiler-agnostic; shares the parser with correlation; no libclang dependency (large, version-coupled).
- ➖ Needs a build with `-g`; unused types aren't available; on non-Darwin Clang some types exist only as declarations.
- A frontend-based `LayoutProvider` may be added later as an optional plugin for instant, build-free hovers. The query and result types already allow it (`Evidence` distinguishes the source).

## ADR-0003: Formats, debug sources, map parsers and languages are plugins behind SPI traits
**Status:** Accepted; `DebugSource` part superseded by ADR-0018
**Decision:** `BinaryFormat`/`Image`, `DebugSource`, `MapFileParser`, `LanguageSupport` traits in `libstratum-core::spi`. One crate per plugin, registered statically via `Engine::builder()`. Core crates must not depend on plugins.
**Consequences:** Adding a format touches only its crate. The IR must be rich enough (two address spaces, raw names, identity kinds). Missing concepts require an ADR.

## ADR-0004: Library first, CLI second, other frontends later
**Status:** Superseded by ADR-0017
**Decision:** Phase 1 ships only the library; Phase 2 the CLI. FFI, server, LSP, MCP, GUI and IDE come later.
**Consequences:** The public API is designed for embedding from day one (no global state, `Send + Sync`, cancellation, progress, no panics, serializable request/response types). The CLI is the first consumer and proves the API's ergonomics.

## ADR-0005: Clang first; GCC later without redesign
**Status:** Superseded by ADR-0019
**Decision:** Fixtures and quirk handling target Apple Clang (Mach-O) and LLVM Clang + lld (ELF). Designs that GCC will need are built in now: global `DieRef` (cross-CU abstract origins), both bitfield encodings, cross-CU declaration resolution, and the documented limit that fold detection is incomplete.
**Consequences:** One toolchain family covers both formats. GCC is a matrix addition, not an architecture change.

## ADR-0006: Linker maps are optional enrichment
**Status:** Accepted
**Context:** Map formats differ across GNU ld, lld and ld-prime, and have no stability guarantee. But on Mach-O the map is the only dSYM-independent source for dead-stripped symbols and exact symbol sizes (✅ verified).
**Decision:** No query *requires* a map. Maps improve precision, and that's recorded in `Evidence::LinkMap`. Parsers are version-sniffed plugins.

## ADR-0007: Queries return lists and structured provenance; evidence and diagnostics on every result
**Status:** Accepted
**Decision:** See [03](03-data-model.md). There is no single-answer API. Empty results carry reasons. `Folded` cost is never attributed to one origin. Elimination is split into `DiscardedByLinker` / `OptimizedOut` / `NoCode`.
**Consequences:** Frontends must handle multiplicity, which is the point. The API is slightly more verbose, but it's honest.

## ADR-0008: Lazy per-CU parsing before incremental ingest
**Status:** Accepted
**Context:** The original concept proposed incremental re-ingest for AI-agent loops. Incremental DWARF processing is complex and breaks under LTO, where there are no per-object boundaries.
**Decision:** Index cheaply at open, correlate lazily per query scope, memoize per session, and cache indexes by content hash. Revisit incremental ingest only if benchmarks show open/index is the bottleneck.

## ADR-0009: Separate cache (unstable, internal) from snapshots (versioned, public)
**Status:** Accepted
**Decision:** See [02 §8](02-architecture.md#8-cache-and-snapshots). The library never writes to disk unless the host supplies a `CacheStore`.

## ADR-0010: Every size is reported in both VM and file space
**Status:** Accepted
**Decision:** Borrowed from Bloaty. `Size { vm, file }` everywhere. Summaries are exhaustive, with explicit `unattributed`.

## ADR-0011: Strict binary/debug-info identity matching by default
**Status:** Accepted
**Decision:** Build ID (ELF) or `LC_UUID` (Mach-O) must match. Mismatch → diagnostic, and the source is refused unless `strict_identity=false`. Mach-O OSO objects are additionally checked by mtime (`oso-object-stale`).

## ADR-0012: Serializable Request/Response dispatcher alongside typed methods
**Status:** Accepted
**Decision:** `Session::execute(model::Request) -> model::Response` exists from M5. The CLI, FFI, JSON-RPC server and MCP all route through it. JSON Schemas are generated from the model types.

## ADR-0013: Artifact providers and lenses
**Status:** Accepted
**Context:** Beyond binaries and DWARF, toolchains emit other ground-truth artifacts (LLVM optimization remarks, time traces, stack size data, profiles, SVD hardware descriptions). A suite of analyses over them is planned ([09](09-suite.md)).
**Decision:** Add an `ArtifactProvider` plugin kind. Providers emit facts keyed **only** by shared join keys (`SourceLoc`, `SymbolKey`, `TypeKey`, `Address`) and never depend on each other. User-facing analyses are **lenses**: query families in the shared `Request`/`Response` model. Heavy dependencies (disassembler) live in optional plugins.
**Consequences:** New analyses don't fork the engine or the frontends. Join-key normalization becomes the core's most critical contract. Each provider needs a version-pinned fixture corpus.

## ADR-0014: Capabilities query
**Status:** Accepted
**Decision:** `Session::capabilities()` reports, per lens, `Available | Partial | Unavailable` together with actionable missing inputs (`RebuildWith(flag)`, `KeepObjects`, `ProvideMapFile`, `ProvideSvd`, …).
**Consequences:** Agents and UIs can check before querying and tell users exactly how to fix gaps, instead of receiving empty answers.

## ADR-0015: Embedded targets in v1 scope
**Status:** Accepted (option b, via ADR-0019)
**Context:** Embedded firmware is the highest-value domain ([10](10-embedded.md)). It's almost entirely 32-bit ELF (Arm Thumb, RV32), often built with GCC.
**Options:** (a) keep v1 at x86_64 + aarch64, embedded later; (b) add `arm` (Thumb, ARMv6-M/v7-M/v8-M) and `riscv32` ELF fixtures to v1, and bring GCC (arm-none-eabi) forward to M3–M4.
**Leaning:** (b) if the first users are firmware developers. The engine cost is small (Thumb normalization, 32-bit fixtures); most of the cost is fixture-matrix growth.

## ADR-0016: Three address spaces and memory regions
**Status:** Accepted
**Context:** On MCUs, `.data` occupies flash (LMA) and RAM (VMA). `Size { vm, file }` ([ADR-0010](#adr-0010-every-size-is-reported-in-both-vm-and-file-space)) can't express flash vs RAM cost. ELF `PT_LOAD` has `p_paddr` (load address) and `p_vaddr` (run address).
**Decision (proposed):** Extend IR extents and `Size` with a `load` space. Add `MemoryRegion { name, origin, length, attrs }`, sourced from the map's `Memory Configuration` → linker script `MEMORY` → user config → heuristic. Add a `region` summary dimension and utilization.
**Consequences:** Additive now; it would be a breaking model change later. Hosted targets simply report `load == vm`.

## ADR-0017: Backend-only scope
**Status:** Accepted; sequence refined by ADR-0020
**Context:** The goal is a low-level observability platform that answers precise questions about code and binaries. UI, CLI, IDE and editor integrations distract from making the backend correct across platforms.
**Decision:** Build only the backend: library crates, plugins, fixtures, goldens, the agent evaluation harness, and `examples/`. No CLI, FFI, server, LSP, MCP, GUI or IDE work until the backend's v1 exit criteria are met. The serializable `Request`/`Response` model stays as part of the backend API.
**Consequences:** Supersedes ADR-0004. Frontend concerns (env vars, config files, output formatting, HTTP) stay out of the core, as rule 6 already required.

## ADR-0018: Debug-info-neutral IR (DWARF and PDB backends)
**Status:** Accepted
**Context:** Windows is Tier 1, and Windows debug info is PDB/CodeView, not DWARF. The earlier design centered correlation on DWARF and let `gimli` types into the SPI.
**Decision:** Introduce `DebugInfoBackend`/`DebugReader` producing a neutral `DebugIr` (units, types with raw layouts, functions, inline trees, line tables, discard evidence, optional symbols). `libstratum-debug-dwarf` (gimli) and `libstratum-debug-pdb` (ms-pdb or pdb2) are backends. Correlation and lenses consume only `DebugIr`. Parser types never appear in SPI or public signatures. Demangling becomes a per-scheme `Demangler` (Itanium, MSVC), separate from `LanguageSupport`. See [11 §4](11-platforms.md#4-architecture-change-debug-info-neutral-ir).
**Consequences:** Supersedes the `DebugSource` design in ADR-0003. The first milestones must implement the layout lens on **both** backends to prove the abstraction before correlation code piles up on it.

## ADR-0019: v1 platform matrix
**Status:** Accepted
**Decision:** Tier 1 (release-blocking): Windows PE+PDB (MSVC, clang-cl; x64, arm64) · Linux ELF+DWARF (Clang, GCC; x86_64, aarch64) · macOS Mach-O+DWARF (Apple Clang; arm64, x86_64) · Embedded ELF+DWARF (arm-none-eabi-gcc, LLVM/ATfE; Cortex-M thumbv6m/v7em/v8m, rv32imac). Tier 2: Android, iOS, MinGW (PE+DWARF), armclang, IAR, ARM64EC/ARM64X. Consoles via private plugins. See [11 §1](11-platforms.md#1-the-platform-matrix).
**Consequences:** Supersedes ADR-0005 (GCC is Tier 1 now). The fixture matrix grows substantially and needs Windows CI runners for MSVC. Accepts ADR-0015 (b) and ADR-0016.

## ADR-0020: Delivery sequence: library → real-code validation → CLI
**Status:** Accepted
**Context:** "Backend" and "library" are the same thing: the Rust crates. The open question was what comes after it.
**Decision:**
1. **Library (backend v0.1)** per the roadmap (M0–M5).
2. **Real-code validation:** run the library on the user's C++ game engine (macOS and Windows builds) and at least one firmware project. Dogfooding **starts at M2** (layout), not after M5, so real-world surprises reach the IR while it's still cheap to change.
3. **CLI (`libstratum-cli`)** as the first frontend, for humans and LLMs alike: JSON output by default when not attached to a terminal, pretty output otherwise, stable exit codes, schema-versioned output. Agents can use it via their shell with no extra integration.
4. Later: MCP, FFI, server/LSP, GUI, engine IDE.
**Consequences:** Refines ADR-0017: frontends stay out until backend v0.1 has survived real code; the CLI is explicitly next.

---

## Open questions

| # | Question | Needed by | Leaning |
|---|---|---|---|
| Q1 | ~~Reuse `addr2line`?~~ **Resolved (M0 S5):** it has no line→address or function/inline-tree iteration, so inline trees and line tables are built on `gimli` directly; `addr2line` becomes a dev-dependency test oracle | — | Resolved |
| Q2 | ~~MSRV~~: **1.88**, tracking gimli (set in the scaffold) | — | Resolved |
| Q3 | ~~License~~: **MIT** (decided 2026-09-15). All planned dependencies (gimli, object, ms-pdb/pdb2, cpp_demangle, msvc-demangler) are MIT-compatible | — | Resolved |
| Q4 | Crate names (checked 2026-09-15): `stratum` and `libstratum-core` are **taken** (Stratum V2 Bitcoin mining protocol, actively published); many `nexus-stratum-*` UI crates exist. **Free:** `libstratum`, `libstratum-core`, `libstratum-model`, `libstratum-cli`, `libstratum-model`, `libstratum-cli`, `libstratum-format-elf`, `libstratum-debug-pdb`. Homebrew `stratum` formula: free | — | **Resolved:** `libstratum-*` prefix for every crate, CLI binary `stratum` |
| Q5 | ~~Facade name~~: **`libstratum`**; all crates use the `libstratum-*` prefix; the future CLI binary is named `stratum` (decided 2026-09-15) | — | Resolved |
| Q6 | Path normalization: case-insensitive matching on macOS (APFS default)? | M4 | Match case-insensitively when the image is Mach-O, and flag it |
| Q7 | Symbol sizing on Mach-O without a map: trust DWARF `high_pc` over next-symbol distance? | M3 | Yes, with evidence recorded |
| Q8 | How far to go on virtual base offsets (need most-derived type context)? | M2 | Report "context-dependent"; no computation in v1 |
| Q9 | Should `-O0` reference-build comparison (`--reference`) be in v1? | M4 | Yes: it's the only way to claim `OptimizedOut` credibly |
| Q10 | Snapshot file format: JSON (+zstd) or a binary format? | M5 | JSON+zstd for v1; readable and diffable |
| Q11 | ~~First users~~: resolved by ADR-0019 (all major platforms + embedded); dogfood on the game engine and one firmware project | — | Resolved |
| Q12 | ~~MCP in Phase 2~~: deferred by ADR-0017 | — | Deferred |
| Q14 | PDB reader: `ms-pdb` (Microsoft, pure Rust) vs `pdb2` (fork of Sentry's `pdb`)? Criteria: inline-site and C13 coverage, API stability, speed on multi-GB PDBs | M0 spike | `ms-pdb` |
| Q15 | Cross-building clang-cl fixtures on macOS/Linux (e.g. `xwin`): licensing and reproducibility OK? | M0 | Verify; MSVC fixtures on Windows CI regardless |
| Q13 | Disassembler for the codegen and stack lenses: Capstone bindings (C dependency) or pure-Rust (`yaxpeax`)? | Lens 3 | Evaluate Thumb, RISC-V and arm64 coverage first |
