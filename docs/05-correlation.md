# 05 — Correlation algorithms

> **Amended by ADR-0018:** algorithms run on the neutral `DebugIr`. DWARF terms below (CU, DIE, `DW_TAG_*`) describe the DWARF backend; the PDB backend maps them per [11 §4.3](11-platforms.md#43-what-had-to-be-generalized) (module, TPI/IPI records, `S_INLINESITE`, C13 lines).

How the engine turns IR plus DWARF into answers. Each section states the algorithm, the edge cases, and the **limits we report honestly** instead of papering over.

Build order (dependencies flow downward):

```
CU index ─┬─► type table ─────────────► struct_layout
          ├─► subprogram/inline trees ─┐
          └─► line tables ─────────────┼─► address range index ─► by_address / by_source_line
symbols + sections ─► ICF groups ──────┤
demangled names ─► instantiation groups┤
link map (optional) ─► discarded set ──┘
```

## 1. CU index (always first, cheap)

For every debug source and unit: offset, `DW_AT_name`, `DW_AT_comp_dir`, `DW_AT_language`, `DW_AT_producer`, `DW_AT_ranges`/`low_pc`, DWARF version, and (split DWARF) the skeleton → `.dwo` link. It doesn't walk DIEs.
- Build `address → CU` using `.debug_aranges` when present and trustworthy, otherwise CU ranges.
- Producer strings reveal compiler, version and flags (`-flto`, `-O`) → used for diagnostics like `lto-object`.

## 2. Struct layout

**Input:** a type name or pattern. **Output:** `LayoutResult` ([03](03-data-model.md#layout)).

1. **Find candidates.** Build a lazy name index over `DW_TAG_{structure,class,union}_type` DIEs, including those nested in namespaces and classes (qualified names are built from the parent chain). With `-gsimple-template-names`, names are rebuilt from template parameter DIEs.
2. **Resolve declarations.** Skip `DW_AT_declaration` DIEs, then search all CUs for a definition with the same qualified name. Follow `DW_AT_signature` (type units, `-fdebug-types-section`) and `DW_AT_specification`.
   - No definition anywhere → `not_found_reason = DeclarationOnly` + a hint (on non-Darwin Clang: vtable-homing / `-fstandalone-debug`).
3. **Dedupe across CUs.** Same qualified name + same size + same member signature → one layout (ODR). Otherwise → multiple matches + `odr-conflict` diagnostic (common with `#ifdef`-dependent structs, which is exactly what users want to see).
4. **Walk members** in DIE order, then sort by bit offset:
   - `DW_TAG_member`: `DW_AT_data_member_location` (constant; rare location expressions are evaluated if trivial, otherwise a diagnostic).
   - Bitfields: `DW_AT_data_bit_offset` (DWARF 4+) **or** legacy `DW_AT_bit_offset` + `DW_AT_byte_size` (endian-adjusted).
   - `DW_TAG_inheritance`: base-class subobjects, `DW_AT_virtuality`. Empty bases (EBO) have size 0 or share an offset.
   - Vtable pointer: `_vptr$Name` artificial member (Clang), or inferred at offset 0 for dynamic classes with no non-virtual primary base (`Evidence::Heuristic`).
   - Virtual bases: their location is a runtime expression. Report them as `Base { is_virtual: true }` with the offset known only for the complete object; otherwise flagged as "offset depends on most-derived type".
   - Anonymous unions and structs: recurse and inline them as an `Anonymous` entry.
   - Flexible array member: an array with no upper bound at the end.
   - `[[no_unique_address]]`: members may overlap; overlaps are allowed and reported, never "fixed".
5. **Compute holes:** walk sorted entries in bits; gaps become `Hole { offset, size }`; `size*8 − end_of_last` is tail padding. Bitfield storage units are handled per pahole's model.
6. **Alignment:** `DW_AT_alignment` if present (emitted for explicit `alignas`); otherwise the max member alignment, recursively, from base types (`Evidence::Computed`).
7. **Cache lines:** mark boundaries every `cacheline` bytes (default 64) and list which members straddle a boundary.
8. **Reorder suggestion:** decreasing alignment, then decreasing size; a flexible array member stays last. Offered only
   when it shrinks the type and its preconditions hold (power-of-two alignments, sizes that are multiples of alignment,
   plain fields, not packed). Under those preconditions the order is **provably minimal** over all orderings: any order
   needs at least `round_up(Σ size, struct alignment)`, and in decreasing-alignment order every running offset is
   already aligned, so no padding is inserted (proof in `libstratum_core::layout::suggest_reorder`, checked against
   brute force over all permutations). Bases, vptr and virtual bases are never moved. Caveats are always attached.

**Limits:** layouts reflect what *this* build (flags, target, `#ifdef`s) produced. No debug info → no layout. Types never used by the program aren't emitted by Clang at all.


**Members sharing storage.** In a struct, two *non-empty* data members (fields or bitfields) whose bit ranges intersect
get a note: that's an anonymous union flattened by the debug format (PDBs never record the union). Members whose type
is empty are excluded because `[[no_unique_address]]` lets them share an address by design; bitfields in one storage unit
occupy disjoint bits. No size threshold is involved: `union { char a; char b; }` is detected
(fixture `layout/overlap.cpp`).

## 3. Sections, symbols, summaries

1. Base map from segments + sections, with VM and file extents (Bloaty's idea). Every file byte and every VM byte belongs to exactly one section or is `unattributed` (headers, padding, load commands).
2. Symbol sizes:
   - ELF: `st_size`. Zero-size symbols are labels; they are assigned to the enclosing sized symbol.
   - Mach-O: nlist has no size. Precedence: link map size → DWARF subprogram `high_pc − low_pc` → distance to the next symbol in the same section (`Evidence::Heuristic`).
3. Summary rows group by the requested dimensions. **Invariant:** `sum(rows) + unattributed == total`, per space, and folded bytes are counted once.

## 4. Identical code folding (ICF)

1. Group function symbols by `(section, start, size)`. Groups with more than one *distinct* function → `Folded` (aliases produced by the same source, like C1/C2 constructors, are recognized by demangled-name equivalence and **not** called folds).
2. Enrich with evidence: lld `--print-icf-sections` output or map data (if provided); DWARF subprograms whose ranges point at the same address; tombstoned DWARF for folded-away functions.
3. **Attribution:** `shared = size`, `candidates = all`. Per-symbol size reports show `size` with a `folded_with` list, and totals count it once.

**Verified rules (M0 spike S8):**
- lld: folded functions keep symbol-table aliases at the representative address, and their DWARF subprograms are tombstoned (`low_pc = 0`), **exactly like garbage-collected functions**. So: tombstone + live alias → `Folded`; tombstone + no symbol → `DiscardedByLinker`.
- GCC `-fipa-icf`: merged static functions keep both symbols at one address, but their DWARF subprograms have **no `low_pc` at all**. The merge is visible only through the symbol table.
- ld-prime folds non-external functions only; lld `--icf=all` also folds externals; GCC `-O2` didn't merge externals.

**Limits:** compiler-internal merging (GCC `-fipa-icf`, LLVM MergeFunctions) can leave a single symbol, or a thunk, so no fold is visible in the symbol table. Documented as undetectable without extra evidence.

## 5. Instantiations and duplicates

1. Demangle (Short style) → `instantiation_key` (template name without arguments, plus enclosing scope).
2. Join with DWARF: subprograms sharing `DW_AT_decl_file/line` (via `DW_AT_specification` or `DW_AT_abstract_origin`) and differing in template parameters are one group.
3. Also group **per-CU copies** of `static`/`inline` functions that weren't merged (same decl location, different addresses, internal linkage).
4. Report per group: count, total size, and sizes per instance. This is the "template bloat" view.

## 6. Elimination

We make **three distinct, evidence-backed claims**, never one vague "eliminated":

| Outcome | Evidence required |
|---|---|
| `DiscardedByLinker` | ELF: DWARF subprogram with tombstone `low_pc` · Mach-O: `.o` DWARF subprogram with no debug-map entry, **or** link map `# Dead Stripped Symbols` · any: map discarded-sections list |
| `OptimizedOut` | Line present in a line table we can see (Mach-O `.o`, or a user-supplied `-O0` reference build via `--reference`) **but** no surviving range in the final image covers it. `confidence: High` when the reference build is the same source revision |
| `NoCode` | No line-table row for that `file:line` anywhere. We explicitly **don't** claim elimination: the line may be a comment, a declaration, or code the compiler dropped before emitting line info |

Why the original "diff the line program" idea is insufficient: the line program of an optimized build only contains rows for code that *survived* codegen. Lines removed during optimization produce no rows, so there is nothing to diff against within one binary. (✅ verified that tiny inlined functions leave neither rows nor inline DIEs attributed to their own lines.)

## 7. Address → source (inline chains)

1. Address → CU (aranges or CU ranges) → the deepest DIE whose ranges contain the address, walking `DW_TAG_subprogram` → nested `DW_TAG_inlined_subroutine`s.
2. The line-table row for the address gives the **innermost** location. Each enclosing inline DIE's `DW_AT_call_file/line/column` gives the location in its parent. The result is the chain, innermost first.
3. Each frame's function name comes from `DW_AT_abstract_origin` → `DW_AT_linkage_name`/`DW_AT_name`, following references across CUs (needed for GCC LTO; `DieRef` is global).
4. Layer on `Folded` and `Instantiation` facts from §4 and §5 for the containing symbol.

**Implementation choice:** reuse `addr2line`'s frame resolution (as a dependency or vendored logic) behind our `DebugSource` abstraction. It already handles DWARF 5, split DWARF, and Mach-O loading edge cases. (❓ confirm its API lets us supply our own section loader and address maps. Spike in M3.)

## 8. Source line → code

1. Normalize the query path (absolute via comp dir; prefix remaps; case-sensitivity per platform) → the set of file entries across all line tables.
2. **Range index:** for each line table, collapse consecutive rows into `[start, end) → (file, line, column, discriminator)` and invert them into `file → line → Vec<range>`. Built lazily per CU that references the file.
3. For each range, compute provenance via §7 (sampling the range start) and merge adjacent ranges with the same provenance.
4. `is_stmt` rows define "the line starts here". Non-statement rows are included but marked, so a UI can show "part of line N" vs "line N begins".
5. Empty → run the §6 classification to choose `DiscardedByLinker` / `OptimizedOut` / `NoCode` / `FileNotInDebugInfo`.

## 9. LTO

- **Clang LTO** (full or thin): DWARF is regenerated at link time. CU names stay per-source, but cross-module inlining is common and `.o` files are LLVM bitcode (no DWARF). On Mach-O, the debug map points to an **LTO temporary object** (`-object_path_lto`). If the user didn't keep it, there is no debug info → `lto-object` diagnostic with the flag hint. 📚❓ (verify in M0)
- **GCC LTO** (later): early debug info in `.gnu.debuglto_*` sections plus cross-unit abstract origins. Handled by global `DieRef`s; must be validated when GCC is added.
