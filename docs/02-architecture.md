# 02 — Architecture

> **Amended by [11 §4](11-platforms.md#4-architecture-change-debug-info-neutral-ir) and ADR-0017/0018.** The debug-info layer is now neutral (`DebugInfoBackend` → `DebugIr`, with DWARF and PDB backends); §4.2 `DebugSource` below is superseded; frontends in §10 are deferred. The crate map in 11 §4.4 replaces §2.

This document defines the structure that is hardest to change later: layers, crate boundaries, dependency rules, extension points, and the public API contract.

## 1. Layered view

```
                       ┌─────────────────────────────────────────────┐
  FRONTENDS            │ libstratum-cli (Phase 2)                       │
  (thin clients)       │ later: libstratum-ffi · libstratum-server         │
                       │        (LSP / MCP / JSON-RPC) · GUI · IDE   │
                       └──────────────────────┬──────────────────────┘
                                              │ public API only
                       ┌──────────────────────▼──────────────────────┐
  FACADE               │ libstratum — Engine builder, default plugins│
                       │             behind Cargo features           │
                       └──────────────────────┬──────────────────────┘
                                              │
                       ┌──────────────────────▼──────────────────────┐
  PUBLIC TYPES         │ libstratum-model — serde types, schema_version │
                       └──────────────────────▲──────────────────────┘
                                              │ produces
                       ┌──────────────────────┴──────────────────────┐
  ENGINE               │ libstratum-core                                │
                       │   spi/       plugin traits + normalized IR  │
                       │   dwarf/     DWARF reader over gimli        │
                       │   correlate/ graph construction             │
                       │   query/     query execution               │
                       │   cache/     snapshot encode/decode        │
                       │   diag/      diagnostics                    │
                       └──────▲──────────────▲──────────────▲────────┘
                              │ implements   │ implements   │ implements
  PLUGINS        ┌────────────┴──┐ ┌─────────┴─────┐ ┌──────┴──────────┐
                 │ format-elf    │ │ format-macho  │ │ lang-cpp        │
                 │ (+ map: gnu,  │ │ (+ map: ld64) │ │ (demangle,      │
                 │  lld)         │ │               │ │  type naming)   │
                 └───────────────┘ └───────────────┘ └─────────────────┘
```

## 2. Crates

| Crate | Kind | Responsibility | Depends on |
|---|---|---|---|
| `libstratum-model` | lib | Public, serializable result types; `SCHEMA_VERSION`. **No logic.** | `serde` |
| `libstratum-core` | lib | SPI traits, normalized IR, DWARF layer, correlation, queries, cache, diagnostics | `libstratum-model`, `gimli`, `object` (read-only helpers) |
| `libstratum-format-elf` | lib | ELF `BinaryFormat` + debug-info locators (embedded, `.dwo`/`.dwp`, debuglink, build ID) + GNU ld and lld map parsers | `libstratum-core`, `object` |
| `libstratum-format-macho` | lib | Mach-O `BinaryFormat` (thin and universal binaries) + locators (dSYM, debug map/OSO, archives) + ld64/ld-prime map parser | `libstratum-core`, `object` |
| `libstratum-lang-cpp` | lib | `LanguageSupport` for C and C++: Itanium demangling, name normalization, CU filtering | `libstratum-core`, `cpp_demangle` |
| `libstratum` | lib (facade) | What embedders depend on: `Engine::builder()` with default plugins behind features `elf`, `macho`, `cpp` | all of the above |
| `libstratum-cli` | bin | Phase 2 CLI | `stratum` |

Future crates (not built now, but planned for): `libstratum-ffi` (C ABI + generated header), `libstratum-server` (JSON-RPC transport, then LSP and MCP adapters), `libstratum-format-pe`, `libstratum-debug-pdb`, `libstratum-lang-rust`.

### Dependency rules (enforced in CI, e.g. via `cargo deny` or a workspace check)

1. `libstratum-model` depends on nothing internal.
2. `libstratum-core` **must not** depend on any plugin crate.
3. Plugins depend on `libstratum-core` and never on each other.
4. Frontends depend only on `libstratum` (the facade) and `libstratum-model`. They never touch `libstratum-core` internals.
5. **No `gimli::*` or `object::*` types in any `pub` signature of `libstratum` or `libstratum-model`.** Parser upgrades must not be breaking changes for embedders.
6. Only frontends may print, read environment variables, or pick file locations.

## 3. The processing pipeline

```
open(path | bytes)
  │
  ├─ 1. PROBE      each registered BinaryFormat::probe(header) → pick one (or error: UnknownFormat)
  ├─ 2. OPEN       BinaryFormat::open → Box<dyn Image>             (arch, identity, segments, sections, symbols)
  ├─ 3. LOCATE     Image::debug_sources(locator) → Vec<DebugSource> (identity-checked)
  ├─ 4. INDEX      lazy: per-CU index (offset, name, language, address ranges). No DIE walking yet.
  │                ── Session is usable here: summaries and symbol queries work without DWARF ──
  ├─ 5. CORRELATE  on demand, per query scope:
  │                  types      → layout table
  │                  functions  → inline trees + line tables → range index
  │                  symbols    → ICF groups, instantiation groups
  │                  maps       → linker-discarded / dead-stripped set
  └─ 6. QUERY      read-only over the graph → libstratum-model results + diagnostics
```

**Laziness is a core requirement, not an optimization.** A hover on one struct must not parse all of DWARF. Step 4 is cheap (it only reads unit headers and top-level attributes). Step 5 runs per compile unit and per query family, and results are memoized in the session. This also answers the AI-agent latency concern better than incremental re-ingest (see [ADR-0008](08-decisions.md#adr-0008-lazy-per-cu-parsing-before-incremental-ingest)).

## 4. Extension points (SPI)

All traits live in `libstratum_core::spi`. They are the **only** contract a plugin sees. They are versioned with the core crate; breaking them is a major version bump.

### 4.1 Binary formats

```rust
pub trait BinaryFormat: Send + Sync + 'static {
    /// Stable id: "elf", "macho".
    fn id(&self) -> &'static str;
    /// Cheap magic-number check on the first bytes. Must not allocate or fail.
    fn probe(&self, header: &[u8]) -> ProbeResult;          // No | Yes | YesContainer (e.g. universal)
    /// Parse headers, sections, symbols. Select arch slice via `opts` for containers.
    fn open(&self, src: Arc<dyn ByteSource>, opts: &OpenOptions) -> Result<Box<dyn Image>, SpiError>;
}

pub trait Image: Send + Sync {
    fn format_id(&self) -> &'static str;
    fn arch(&self) -> Arch;                                   // X86_64 | Aarch64 | Other(u32)
    fn endian(&self) -> Endian;
    fn identity(&self) -> ImageIdentity;                      // BuildId(bytes) | Uuid([u8;16]) | None, + content hash
    fn segments(&self) -> &[ir::Segment];
    fn sections(&self) -> &[ir::Section];                     // normalized kind + raw names + vm/file extents
    fn symbols(&self) -> &[ir::Symbol];                       // raw (mangled) names, address, size, binding, section
    fn section_data(&self, id: ir::SectionId) -> Result<Cow<'_, [u8]>, SpiError>; // decompressed
    /// Every place debug info for this image may live, in priority order.
    fn debug_sources(&self, loc: &dyn FileLocator) -> Result<Vec<Box<dyn DebugSource>>, SpiError>;
}
```

### 4.2 Debug info sources

Debug info is a **separate layer** from the container format. Most formats carry DWARF, but where it lives and which address space it uses differ:

```rust
pub trait DebugSource: Send + Sync {
    fn kind(&self) -> DebugSourceKind;            // Embedded | Dsym | MachODebugMapObject | SplitDwo | Dwp | DebugLink
    fn identity(&self) -> ImageIdentity;          // must match the Image, else DebugInfoMismatch
    fn encoding(&self) -> DebugEncoding;          // Dwarf (today); Pdb later
    fn dwarf_section(&self, id: gimli::SectionId) -> Result<Option<Cow<'_, [u8]>>, SpiError>; // crate-private visibility of gimli is OK inside SPI
    /// Addresses in this source → final image addresses. `None` = already final (dSYM, ELF).
    fn address_map(&self) -> Option<&dyn AddressMap>;
}
```

- **ELF:** one `Embedded` source; plus `SplitDwo`/`Dwp` for `-gsplit-dwarf`; plus `DebugLink` for separate debug files.
- **Mach-O with dSYM:** one `Dsym` source, addresses already final.
- **Mach-O without dSYM:** one `MachODebugMapObject` source *per OSO entry*, each with an `AddressMap` built from the debug map's `FUN`/`STSYM` stabs. See [04 §Mach-O](04-formats-and-toolchains.md#3-mach-o).

> The `gimli::SectionId` appearance is deliberate and allowed: the SPI is an *internal-facing* contract for plugins, which already depend on the DWARF model. Rule 5 applies to the embedder-facing API only.

### 4.3 Linker maps

```rust
pub trait MapFileParser: Send + Sync + 'static {
    fn id(&self) -> &'static str;                    // "gnu-ld", "lld-elf", "ld64"
    fn sniff(&self, head: &str) -> bool;
    fn parse(&self, text: &str) -> Result<ir::LinkMap, SpiError>;  // input files, placed symbols, discarded/dead-stripped, ICF info if present
}
```

Maps are **optional enrichment** ([ADR-0006](08-decisions.md#adr-0006-linker-maps-are-optional-enrichment)). No query may *require* a map; some answers get more precise with one (e.g., linker-discarded functions on Mach-O, where `dsymutil` removes their DWARF, which we verified).

### 4.4 Language support

```rust
pub trait LanguageSupport: Send + Sync + 'static {
    fn id(&self) -> &'static str;                        // "c-cpp"
    fn claims(&self, dw_lang: gimli::DwLang) -> bool;    // DW_LANG_C*, DW_LANG_C_plus_plus*
    fn demangle(&self, raw: &str, style: NameStyle) -> Option<String>;  // Full | Short (no template args)
    fn qualified_type_name(&self, die: &ir::TypeNameParts) -> String;
    fn instantiation_key(&self, raw: &str) -> Option<String>;          // groups add3<int>, add3<long long>
}
```

Compile units no plugin claims are **indexed but skipped** during correlation, with a diagnostic (`UnsupportedLanguage { cu, lang }`). They still count toward size summaries through symbols and sections.

### 4.5 Artifact providers (suite)

Non-binary artifacts (optimization remarks, time traces, stack sizes, profiles, SVD) plug in through `ArtifactProvider` and join on shared keys only. Full design in [09-suite.md §4](09-suite.md#4-architecture-changes); decisions in ADR-0013 and ADR-0014.

### 4.6 Host services (injected by the embedder)

```rust
pub trait ByteSource: Send + Sync { fn len(&self) -> u64; fn bytes(&self) -> Result<&[u8]>; }  // mmap, in-memory buffer, VFS
pub trait FileLocator: Send + Sync {                                                           // how to find dSYM/.o/.dwo/debuglink
    fn open(&self, request: &LocateRequest) -> Result<Option<Arc<dyn ByteSource>>>;
}
pub trait CacheStore: Send + Sync {                                                            // where snapshots live (or nowhere)
    fn get(&self, key: &CacheKey) -> Option<Arc<dyn ByteSource>>;
    fn put(&self, key: &CacheKey, bytes: &[u8]) -> Result<()>;
}
```

The facade provides defaults: `MmapSource`, `FsLocator` (dSYM next to binary, OSO paths, `.dwo` via `DW_AT_dwo_name` + comp dir, debuglink dirs, build-ID dirs), and `NoCache`/`DirCache`. An IDE can swap in its virtual file system. A GUI can load from memory.

### 4.7 Registration

```rust
let engine = libstratum::Engine::builder()
    .format(libstratum_format_elf::Elf::default())
    .format(libstratum_format_macho::MachO::default())
    .map_parser(libstratum_format_elf::maps::Lld)
    .map_parser(libstratum_format_macho::maps::Ld64)
    .language(libstratum_lang_cpp::CCpp::default())
    .locator(libstratum::FsLocator::default())
    .cache(libstratum::DirCache::new(cache_dir))
    .build()?;

// or, with default features:
let engine = libstratum::Engine::with_defaults()?;
```

Registration is explicit and static. No dynamic library loading and no global registry, so embedders pay only for what they enable, and tests can build engines with fake formats.

**Adding a new format checklist** (e.g. PE with MinGW DWARF): new crate → implement `BinaryFormat` + `Image` + its `DebugSource`s → add fixtures → register. No change to `correlate`, `query`, `model` or frontends. If a change there *is* needed, the IR is missing a concept, and that deserves its own ADR.

## 5. Normalized IR (internal)

The IR is the format-free representation that `correlate` consumes. Key rules:

- **Two address spaces.** Every extent carries `vm: Option<Range>` and `file: Option<Range>` (`.bss` has no file range, `.debug_*` has no VM range). Borrowed from Bloaty.
- **Section kind is normalized; names are kept raw.** `SectionKind = Code | ReadOnlyData | Data | ZeroInit | Tls | Unwind | Debug | Symtab | Relocs | Metadata | Other`. Raw names are kept as `{segment: Option<"__TEXT">, name: "__text"}` or `{segment: None, name: ".text"}`.
- **Symbols keep raw names.** Demangling is a language-plugin concern, done lazily.
- **Stable IDs.** `SectionId`, `SymbolId`, `CuId`, `DieRef` (source + unit + offset) are opaque indices valid for the session's lifetime. Public results carry *stable, content-derived keys* (see [03](03-data-model.md#identity)).

## 6. Public API (embedder-facing)

```rust
impl Engine {
    pub fn open(&self, input: Input) -> Result<Session, OpenError>;            // Input::Path | Input::Bytes | Input::Source
}

impl Session {                                                                  // Send + Sync; cheap to clone (Arc)
    pub fn info(&self) -> model::BinaryInfo;                                    // format, arch, identity, debug sources found
    pub fn diagnostics(&self) -> Vec<model::Diagnostic>;

    pub fn struct_layout(&self, q: &model::TypeQuery, ctx: &QueryCtx) -> Result<model::LayoutResult, QueryError>;
    pub fn find_types(&self, pattern: &model::NamePattern, ctx: &QueryCtx) -> Result<Vec<model::TypeRef>, QueryError>;
    pub fn by_symbol(&self, q: &model::SymbolQuery, ctx: &QueryCtx) -> Result<model::SymbolResult, QueryError>;
    pub fn by_address(&self, addr: u64, ctx: &QueryCtx) -> Result<model::AddressResult, QueryError>;
    pub fn by_source_line(&self, loc: &model::SourceQuery, ctx: &QueryCtx) -> Result<model::SourceResult, QueryError>;
    pub fn section_summary(&self, opts: &model::SummaryOptions, ctx: &QueryCtx) -> Result<model::Summary, QueryError>;

    pub fn snapshot(&self, ctx: &QueryCtx) -> Result<model::Snapshot, QueryError>;  // for diffs and CI
}

pub fn diff(base: &model::Snapshot, head: &model::Snapshot, opts: &model::DiffOptions) -> model::Diff;

pub struct QueryCtx { pub cancel: CancelToken, pub progress: Option<Arc<dyn ProgressSink>>, pub limits: Limits }
```

**API contract**
- Every result type includes `diagnostics: Vec<Diagnostic>` and, where relevant, `evidence`.
- **Queries are read-only and idempotent.** Only `open` does heavy I/O (plus lazy reads of debug sources).
- **Cancellation** is checked between CUs and inside long DIE walks. A cancelled query returns `QueryError::Cancelled` and leaves the session valid.
- **No panics escape the public API.** Malformed input produces `Err` or diagnostics. Internal invariant violations are caught and reported as `InternalError` with context. This is a prerequisite for the future C ABI.
- **Limits** (max DIE depth, max results, max bytes decompressed) protect hosts from pathological inputs.

### Errors vs diagnostics

| Situation | Mechanism |
|---|---|
| Can't read the file, unknown format, corrupt headers | `Err(OpenError)` |
| Debug info missing, identity mismatch, OSO object not found, unsupported language CU, DWARF unit parse failure | `Diagnostic` (session still usable, answers degrade explicitly) |
| Type not found, address outside image | `Ok(result)` with an empty `matches` + reason. **Not** an error |
| Cancelled, limit exceeded | `Err(QueryError)` |

## 7. Concurrency and memory

- `Session` is `Send + Sync`. Memoized correlation tables use `OnceLock`/sharded maps, so concurrent hovers on different types don't block each other.
- Per-CU work parallelizes with `rayon` behind a feature flag (`parallel`, on by default). Embedders with their own thread pool can disable it.
- **Zero-copy:** `ByteSource` is memory-mapped by default and gimli readers borrow from it. Decompressed sections are cached per session.
- **Memory budget:** the lazy design means the resident cost grows with *what was asked*, not with binary size. We measure this in benchmarks (see [06](06-testing.md#4-performance-benchmarks)).

## 8. Cache and snapshots

Two separate concepts:

| | Cache | Snapshot |
|---|---|---|
| Purpose | Speed up re-opening an unchanged binary | Persist results for diffs and CI; share across machines |
| Contents | Internal indexes (CU index, symbol tables, computed layouts) | Public `libstratum-model` data (layouts, symbols, sections, sizes) |
| Format stability | **None.** Keyed by stratum version; invalidated freely | **Versioned** (`schema_version`); readable across minor versions |
| Key | `(stratum build hash, format id, image identity, content hash of image + each debug source)` | n/a, it's a file the user owns |

- The content hash uses a fast hash (e.g. `xxh3`/BLAKE3) over bytes. mtime is only a fast pre-check, never the sole key.
- The cache is **opt-in** through `CacheStore`, so the library never writes to disk on its own.

## 9. Configuration

`EngineOptions` / `OpenOptions` carry only semantic settings:
- architecture slice for universal binaries
- extra search paths (via the locator)
- cache-line size (default 64; configurable per query)
- pointer to a linker map file
- demangling style
- `strict_identity` (default `true`)

No config files are read by the library. That's a frontend concern.

## 10. Future frontends

How each planned frontend attaches *without core changes*:

| Frontend | Attachment | Needs from core (already designed in) |
|---|---|---|
| CLI (Phase 2) | Links `stratum` | Public API, serde models |
| GUI app (Rust, e.g. Tauri or egui) | Links `stratum` | `Send + Sync` sessions, progress, cancellation |
| GUI app / custom IDE (C++, Swift, …) | `libstratum-ffi` C ABI: `stratum_engine_new`, `stratum_open`, `stratum_query_json(session, request_json) → response_json`, `stratum_cancel`, `stratum_free` | Serializable requests and responses, no panics, opaque handles |
| VS Code, LSP-capable editors | `libstratum-server` process (JSON-RPC over stdio or socket) + LSP adapter (hover, code lens) | Same request/response JSON as the FFI; cancellation maps to `$/cancelRequest` |
| AI agents | MCP adapter in `libstratum-server`: tools `struct_layout`, `by_symbol`, `by_address`, `by_source_line`, `section_summary`, `diff` | JSON Schemas generated from `libstratum-model` (e.g. `schemars`) |

**Design consequence we adopt now:** every query has a serializable **request** type in `libstratum-model` (`model::Request::StructLayout { .. }`) alongside the typed Rust methods. The CLI, FFI, server and MCP then all share one dispatcher: `Session::execute(Request) -> Response`.

## 11. What is intentionally NOT in the architecture

- **A plugin ABI for dynamically loaded `.so`/`.dylib` plugins.** Rust has no stable ABI, and static registration is enough. Could be added through the C ABI later.
- **Source parsing** (libclang/tree-sitter). Could be added later as a separate `LayoutProvider`/`SourceIndex` plugin, never required by the core.
- **Any compiler invocation by the library.** Building fixtures and binaries is the user's or CI's job.
