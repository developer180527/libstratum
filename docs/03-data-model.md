# 03 — Data model

`libstratum-model` is the public contract. Every frontend (CLI JSON, FFI, server, MCP) serializes these types, so they get the most care.

## Principles

1. **Lists, not options.** A query that can have several answers returns `Vec`, and the empty case carries a *reason*.
2. **Provenance is a sum type,** never a set of boolean flags.
3. **Evidence on every correlated fact.** Which source of truth said so.
4. **Two sizes.** `Size { vm: u64, file: u64 }`.
5. **Additive evolution.** New fields and enum variants may be added in minor versions. Enums are `#[non_exhaustive]` in Rust, and JSON consumers must ignore unknown fields and treat unknown variants as "other".

## Identity

Opaque session indices are never exposed. Public results use stable keys, so snapshots from two builds can be diffed:

| Entity | Stable key |
|---|---|
| Binary | `ImageIdentity { build_id \| uuid, content_hash, format, arch }` |
| Section | `(segment?, name)`, e.g. `("__TEXT","__text")` or `(null,".text")` |
| Symbol | `(raw_name, section_key)` plus an ordinal for duplicate local names |
| Type | `(qualified_name, byte_size, decl_file?, decl_line?)`. Identical definitions across CUs are merged under the ODR assumption; differing ones are **kept separate** and flagged as `OdrConflict` |
| Source location | `(normalized_path, line, column?)`. Paths are made absolute using `DW_AT_comp_dir` and optional user-supplied prefix remaps |

## Core entities

```rust
pub struct SourceLoc { pub file: PathKey, pub line: u32, pub column: Option<u32> }

pub struct CodeRange {
    pub start: u64,                 // final image VM address
    pub size: Size,                 // vm; file when applicable
    pub section: SectionKey,
    pub object_file: Option<String>,// from map file or Mach-O OSO, when known
}

pub struct SymbolInfo {
    pub raw_name: String,
    pub demangled: Option<String>,
    pub kind: SymbolKind,           // Function | Object | Tls | Section | Unknown
    pub binding: Binding,           // Global | Local | Weak
    pub range: Option<CodeRange>,
    pub aliases: Vec<String>,       // other symbols at the identical (start, size)
}
```

## Provenance: binary → source

For a `CodeRange` (or an address), *where did it come from?*

```rust
#[non_exhaustive]
pub enum Provenance {
    /// Code attributed directly to a subprogram at this location.
    Direct { function: FunctionRef, loc: SourceLoc },

    /// Code from an inlined call. Chain is innermost-first:
    /// [inlined callee @ its line, … , outermost concrete function @ call site].
    Inlined { chain: Vec<InlineFrame> },

    /// One of several out-of-line copies of the same source entity
    /// (template instantiations, static inline functions emitted per-CU, weak duplicates).
    Instantiation { function: FunctionRef, group: InstantiationGroupKey, siblings: u32 },

    /// Identical bytes shared by several unrelated symbols (linker ICF or dedup, compiler merge).
    /// Size must not be attributed to any single origin.
    Folded { candidates: Vec<FunctionRef>, shared: Size },

    /// Address is inside the image, but no debug info covers it
    /// (no -g, stripped, runtime/library code, linker-synthesized stubs, missing OSO).
    Unattributed { reason: UnattributedReason },
}

pub struct InlineFrame {
    pub function: FunctionRef,          // abstract origin (name, decl loc)
    pub loc: SourceLoc,                 // line inside this function for this address
    pub call_site: Option<SourceLoc>,   // DW_AT_call_file/line/column of this inline instance
}
```

These combine: an address can be `Inlined` *inside* an `Instantiation`, which is *inside* a `Folded` block. The result type for an address is therefore a structured record, not a single variant:

```rust
pub struct AddressResult {
    pub address: u64,
    pub section: Option<SectionKey>,
    pub symbol: Option<SymbolInfo>,
    pub provenance: Vec<Provenance>,   // outermost fact first; e.g. [Folded, Instantiation, Inlined]
    pub evidence: Vec<Evidence>,
    pub diagnostics: Vec<Diagnostic>,
}
```

## Outcome: source → binary

For a `SourceLoc`, *what did it become?*

```rust
pub struct SourceResult {
    pub query: SourceLoc,
    pub resolved_line: Option<u32>,     // if the query was snapped to the nearest line with code (opt-in)
    pub outcome: SourceOutcome,
    pub evidence: Vec<Evidence>,
    pub diagnostics: Vec<Diagnostic>,
}

#[non_exhaustive]
pub enum SourceOutcome {
    /// One entry per distinct code range. `via` explains each (direct, inlined into X, instantiation Y…).
    Produced { ranges: Vec<(CodeRange, Provenance)> },

    /// The CU for this file is present, the line is in the line table of an object file
    /// (Mach-O .o) or of an -O0 comparison build, but no surviving range starts there.
    OptimizedOut { confidence: Confidence },

    /// Function containing the line existed in an object file, but the linker removed it
    /// (gc-sections, dead_strip). Evidence: map "dead stripped" list, lld tombstone address, .o DWARF.
    DiscardedByLinker { symbol: Option<String> },

    /// No line-table row for this file:line anywhere we can see.
    /// Does NOT claim elimination: the line may simply contain no code (comment, declaration).
    NoCode,

    /// The file is not part of any compile unit we have debug info for.
    FileNotInDebugInfo,
}
```

> **Honesty note.** Clang emits no line-table rows for code it removed early, and (✅ verified) very small inlined functions can leave **no** `DW_TAG_inlined_subroutine` at all. So `NoCode` and `OptimizedOut` can't always be told apart from one optimized binary. The API reports the weaker claim unless there is stronger evidence. See [05 §Elimination](05-correlation.md#6-elimination).

## Layout

```rust
pub struct LayoutResult {
    pub matches: Vec<TypeLayout>,      // >1 when a name is ambiguous or has ODR conflicts
    pub not_found_reason: Option<NotFoundReason>, // DeclarationOnly | NoSuchType | LanguageUnsupported
    pub diagnostics: Vec<Diagnostic>,
}

pub struct TypeLayout {
    pub name: String,                  // qualified, normalized by LanguageSupport
    pub kind: AggregateKind,           // Struct | Class | Union
    pub size: u64,                     // DW_AT_byte_size
    pub alignment: Option<u64>,        // DW_AT_alignment if present, else computed (and marked Evidence::Computed)
    pub decl: Option<SourceLoc>,
    pub members: Vec<LayoutEntry>,     // sorted by bit offset
    pub holes: Vec<Hole>,              // derived
    pub tail_padding: u64,
    pub cacheline: Option<CachelineView>,
    pub suggestion: Option<ReorderSuggestion>,
    pub evidence: Vec<Evidence>,
}

pub enum LayoutEntry {
    Field     { name: Option<String>, ty: TypeName, offset_bits: u64, size_bits: u64, align: Option<u64> },
    Bitfield  { name: Option<String>, ty: TypeName, offset_bits: u64, width_bits: u32, storage_bytes: Option<u64> },
    Base      { ty: TypeName, offset_bits: u64, size_bits: u64, is_virtual: bool, is_empty: bool },
    VtablePtr { offset_bits: u64, size_bits: u64 },
    Anonymous { kind: AggregateKind, offset_bits: u64, size_bits: u64, members: Vec<LayoutEntry> },
    FlexibleArray { name: String, ty: TypeName, offset_bits: u64 },
}

pub struct ReorderSuggestion {
    pub order: Vec<String>,
    pub new_size: u64,
    pub saved: u64,
    pub caveats: Vec<String>, // "changes ABI", "virtual bases not moved", "bitfields kept adjacent"
}
```

Offsets are in **bits** internally to represent bitfields exactly. Frontends present bytes.

## Summary and snapshot

```rust
pub struct Summary {
    pub total: Size,
    pub rows: Vec<SummaryRow>,         // grouped by requested dimensions (segment > section > cu > symbol)
    pub unattributed: Size,            // always present; rows + unattributed == total
    pub shared: Size,                  // bytes in Folded blocks (counted once in total)
}

pub struct Snapshot {
    pub schema_version: u32,
    pub stratum_version: String,
    pub image: ImageIdentity,
    pub sections: Vec<SectionEntry>,
    pub symbols: Vec<SymbolEntry>,
    pub types: Vec<TypeLayout>,        // optional filter by name pattern
}
```

Diffs compare snapshots by stable keys. Renames look like remove + add. No fuzzy matching in v1.

## Evidence

```rust
#[non_exhaustive]
pub enum Evidence {
    Dwarf { source: DebugSourceKind, unit_offset: u64, die_offset: Option<u64> },
    LineTable { source: DebugSourceKind, unit_offset: u64 },
    SymbolTable,
    LinkMap { parser: String },
    DebugMap,                          // Mach-O OSO/stabs
    Computed { rule: &'static str },   // e.g. "alignment = max(member alignment)"
    Heuristic { rule: &'static str },  // lower-trust inference
}
```

## Diagnostics

```rust
pub struct Diagnostic {
    pub severity: Severity,            // Info | Warning | Error
    pub code: DiagCode,                // stable machine-readable, e.g. "debug-info-mismatch"
    pub message: String,               // human-readable
    pub subject: Option<String>,       // path, CU name, symbol…
}
```

Initial codes: `no-debug-info`, `debug-info-mismatch`, `oso-object-missing`, `oso-object-stale`, `dwo-missing`, `unsupported-language`, `dwarf-parse-error`, `declaration-only-type`, `odr-conflict`, `address-map-gap`, `lto-object`, `map-parse-error`, `map-image-mismatch`.

## Schema versioning

- `SCHEMA_VERSION: u32` is embedded in every top-level JSON response and snapshot.
- **Minor changes** (adding fields or variants) don't bump the version.
- **Breaking changes** (renames, removals, semantic changes) bump it. The CLI keeps `--schema-version N` output for at least one previous version.
- JSON Schemas are generated from the Rust types (`schemars`) and committed under `schemas/`, so the CI diff shows any change to the contract.
