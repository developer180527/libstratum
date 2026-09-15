# 09 — The stratum suite: one engine, many lenses

## 1. Thesis

Compilers and linkers already compute a large amount of ground truth about a program: types, layouts, placement, inlining decisions, vectorization failures, stack frames, build cost. Most of it is discarded or scattered across incompatible artifacts.

**The compiler did the computing. The unsolved part is extracting, normalizing, and *joining* those artifacts on shared keys (source location, symbol, type, address), and doing it honestly.** stratum's correlation graph is that join layer, so a suite of low-level analyses built on one engine is a natural extension, not scope creep, *if* it is built as lenses over one model instead of separate tools.

"The hard part is done by compilers" is only half true. What remains hard:

1. **Extraction and normalization.** Each artifact has its own format; several are unstable across compiler versions (e.g. LLVM optimization remarks).
2. **Joining.** The value appears only when facts from different artifacts are linked ("not inlined here" + "this is its code range" + "it is hot").
3. **Honesty.** Every source has gaps (missing flags, stripped binaries, LTO). Saying "unknown, rebuild with X" is work.
4. **Flags.** Much data exists only when the build used specific flags. The suite must tell the user or agent what's missing.

## 2. Artifacts the suite can consume

| Artifact | How it's produced | What it knows | Stability |
|---|---|---|---|
| DWARF | `-g` | Types, layouts, inline trees, line tables, variables | Standardized (v4/v5) |
| Symbol tables, sections, program headers | always | Placement, sizes, load vs run addresses | Standardized per format |
| Linker maps | `-Wl,-Map` / `-Wl,-map` | Input files, discards, memory regions (GNU ld) | Unstable, per linker |
| **LLVM optimization remarks** | `-fsave-optimization-record` (YAML, yaml-strtab, or bitstream with `RMRK` magic) | Why inlining, vectorization or unrolling happened or didn't | Semi-stable; changes across LLVM versions 📚 |
| Build time traces | `-ftime-trace` (Clang) | Per-header, per-template, per-pass compile time | JSON, Clang-specific |
| Stack size data | `-fstack-usage` (GCC `.su`), `-fstack-size-section` (Clang, ELF `.stack_sizes`), Arm Compiler `--callgraph-info=stack` | Per-function frame size | Toolchain-specific 📚 |
| Compilation database | `compile_commands.json` | Exact flags per TU | Stable JSON |
| Runtime profiles | `perf`, Instruments, SWO/ETM traces (embedded) | Hot code, cache misses, contention | Tool-specific |
| Hardware descriptions (embedded) | CMSIS-SVD, devicetree | Peripheral register offsets and sizes, memory map | Stable XML/DTS |

## 3. Lenses

A **lens** is a family of queries in the shared `Request`/`Response` model, backed by one or more providers. Every frontend (CLI, MCP, editor, GUI) gets every lens automatically.

| Lens | Answers | Providers | Existing tool | stratum's edge | Priority |
|---|---|---|---|---|---|
| **Layout** | sizeof, padding, reorder, cache lines | DWARF | pahole, VS Memory Layout | Cross-platform, embeddable, agent-ready | v1 |
| **Correlation** | line ↔ code, inline chains, address → source | DWARF, symbols, maps | addr2line, atos | Bidirectional, honest provenance | v1 |
| **Optimization report** | Why wasn't `f` inlined into `g`? Why didn't this loop vectorize? | Opt remarks + DWARF | `opt-viewer.py`, `llvm-opt-report` | Joined with real code ranges and sizes; biggest cut in hallucination for the effort | 2 |
| **Codegen** | Annotated disassembly of one function (source lines, inline frames) | Binary + DWARF + disassembler plugin | Compiler Explorer (single TU, not the linked binary) | Works on the real linked, LTO'd binary | 3 |
| **Layout/ABI diff** | Did this change alter layout or ABI? | Snapshots | libabigail/abidiff (Linux) | Mach-O too; agent before/after checks | 4 |
| **Size & regressions** | What grew and why | Symbols, maps, snapshots | Bloaty, SuperSize | Integrated with the other lenses, not unique | 5 |
| **Stack** | Frame sizes, worst-case call chains | Stack size data + call graph (disassembly) | puncover, WorstCaseStack, StackAnalyzer | Cross-toolchain, joined with source | 5 (embedded: earlier; see [10](10-embedded.md)) |
| **Build cost** | Which headers and templates cost compile time *and* binary size | Time traces + symbols | ClangBuildAnalyzer | The time×size join | 6 |
| **Hot-path layout** | False sharing, hot fields split across cache lines | Profiles + DWARF layout | `perf c2c` (Linux) | The join is the whole value; hardest | 7 |

Every lens individually has a competitor. **The suite's differentiator is one query model and one set of identities across all of them.** An agent can ask "is `Packet` hot, badly laid out, and did the reorder change codegen?" and get consistent, evidence-backed answers.

## 4. Architecture changes

These generalize [02-architecture.md](02-architecture.md). Recorded as [ADR-0013](08-decisions.md#adr-0013-artifact-providers-and-lenses) and [ADR-0014](08-decisions.md#adr-0014-capabilities-query).

### 4.1 `ArtifactProvider`: a second plugin kind

Binaries are one kind of input. Other artifacts are attached to a session through providers:

```rust
pub trait ArtifactProvider: Send + Sync + 'static {
    fn id(&self) -> &'static str;                         // "llvm-opt-remarks", "clang-time-trace", "gcc-stack-usage", "cmsis-svd"
    fn sniff(&self, name: &str, head: &[u8]) -> bool;
    /// Parse into provider-owned facts, keyed ONLY by shared join keys.
    fn load(&self, src: Arc<dyn ByteSource>, ctx: &ProviderCtx) -> Result<Box<dyn ArtifactFacts>, SpiError>;
}

pub trait ArtifactFacts: Send + Sync {
    fn kind(&self) -> FactKind;                           // OptRemarks | TimeTrace | StackSizes | Profile | HardwareMap
    fn join_keys(&self) -> JoinKeyKinds;                  // which of SourceLoc / Symbol / Type / Address it uses
    fn identity_hint(&self) -> Option<ImageIdentity>;     // for mismatch detection when available
}
```

Session attachment:

```rust
let session = engine.open(Input::path("fw.elf"))?;
session.attach(Input::path("build/remarks/"))?;          // provider auto-selected by sniffing
session.attach(Input::path("STM32F407.svd"))?;
```

### 4.2 Join keys are the contract

Providers never reference each other. They only emit facts keyed by:

| Key | Normalization owner |
|---|---|
| `SourceLoc` | core (path normalization, prefix maps, comp dir) |
| `SymbolKey` | core + `LanguageSupport` (raw ↔ demangled) |
| `TypeKey` | DWARF layer |
| `Address` | Image (plus address translation, Thumb bit, LMA/VMA; see [10](10-embedded.md)) |

A lens implementation joins facts through these keys. Adding a provider never requires changing another provider.

### 4.3 Capabilities query

```rust
pub fn capabilities(&self) -> model::Capabilities;
// → per lens: Available | Partial { missing } | Unavailable { missing }
// → missing: [ RebuildWith("-fsave-optimization-record"), KeepObjects, ProvideMapFile, ProvideSvd, … ]
```

This is essential for agents: they ask first, then either query or tell the user exactly which flags to add.

### 4.4 Optional heavy dependencies stay optional

Disassembly (Codegen, Stack call graphs) needs a disassembler (e.g. Capstone bindings, or `yaxpeax` in pure Rust). It lives in a `libstratum-disasm` plugin behind a feature flag, so the core stays lean.

## 5. Guardrails against scope creep

1. **One lens at a time.** Each ships with fixtures, goldens and an entry in the agent evaluation before the next one starts.
2. **Every provider has a version-pinned fixture corpus** (e.g. opt remarks from LLVM N and N−1).
3. **Don't compete on a competitor's home turf.** Size reporting doesn't need to beat Bloaty; it needs to join cleanly with the other lenses.
4. **The suite shares the engine, not a monolith binary.** Embedders enable only the lenses and providers they need (Cargo features).

## 6. The agent evaluation (proves the suite's value)

A benchmark of 30–50 questions with verified answers, run by an agent (a) with shell only and (b) with stratum via MCP. Measure accuracy, tool calls, and tokens.

Example questions:
- `sizeof(Packet)` on aarch64-apple-darwin and thumbv7em-none-eabihf
- Is `crc16` inlined into `rx_isr` at `-O2`? If not, why?
- Why did `.text` grow 4 KB between these two builds?
- Which hot struct has fields split across a cache line?
- Does the `USART_TypeDef` struct match the SVD register offsets?

Stored under `eval/` with the fixture binaries it needs. Regressions fail nightly CI.

## Sources

- LLVM remarks: https://llvm.org/docs/Remarks.html · https://llvm.org/docs/CommandGuide/llvm-opt-report.html · https://github.com/llvm-mirror/llvm/blob/master/tools/opt-viewer/opt-viewer.py
- Stack analysis: https://blog.japaric.io/stack-analysis/ · https://interrupt.memfault.com/blog/measuring-stack-usage · https://github.com/PeterMcKinnis/WorstCaseStack/blob/master/README.md
- Zephyr footprint reports (DWARF-based): https://github.com/zephyrproject-rtos/zephyr/blob/main/scripts/footprint/size_report
