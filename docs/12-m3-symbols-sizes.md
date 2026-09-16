# 12 — M3 design: symbols, sizes, sections, memory regions

Status: **approved (2026-09-17)**; ADR-0022, ADR-0023 and ADR-0024 accepted. Refines [03 §Summary](03-data-model.md#summary-and-snapshot),
[05 §3–5](05-correlation.md#3-sections-symbols-summaries) and [10 §2.2–2.3](10-embedded.md#22-three-address-spaces-vm-file-and-load)
into buildable decisions.

## 1. Questions M3 must answer

1. What symbols does this binary contain, where are they, and how big is each (with evidence for the size)?
2. How are the image's bytes spent: by segment, section, compile unit and symbol, in VM, file and load space?
3. On embedded targets: how much FLASH and RAM does each region use, how much is left, and who uses it?
4. Which functions were folded together (ICF), and which template instantiations duplicate code?

Every answer obeys the M2 honesty rules: sizes carry evidence, totals add up exactly, gaps are named, heuristics are labeled.

## 2. Public model (additions to `libstratum-model`)

```rust
pub struct SymbolEntry {
    pub raw_name: String,
    pub demangled: Option<String>,
    pub kind: SymbolKind,                 // Function | Object | Tls | Label | Unknown
    pub binding: Binding,                 // Global | Local | Weak
    pub section: Option<SectionKey>,
    pub address: u64,                     // Thumb bit cleared; absolute VA (PE: image base + RVA)
    pub size: Option<u64>,
    pub size_evidence: Option<Evidence>,  // SymbolTable | LinkMap | DebugInfo | Heuristic{"distance to next symbol"}
    pub aliases: Vec<String>,             // other symbols with identical (address, size)
    pub folded: bool,                     // aliases include a *distinct* function (ICF), §6
    pub unit: Option<String>,             // compile unit / PDB module that defined it, when known
}

pub struct SymbolResult { pub query: String, pub matches: Vec<SymbolEntry>, pub diagnostics: Vec<Diagnostic> }

pub struct Summary {
    pub dimensions: Vec<Dimension>,       // e.g. [Region, Section, Unit, Symbol]
    pub total: Size,                      // vm / file / load
    pub rows: Vec<SummaryRow>,            // leaf rows; `path` = one key per dimension
    pub unattributed: Vec<SummaryRow>,    // named gaps: headers, alignment padding, bytes no symbol covers
    pub shared: Size,                     // bytes owned by alias/fold groups (counted once in total)
    pub diagnostics: Vec<Diagnostic>,
}
pub struct SummaryRow { pub path: Vec<String>, pub size: Size, pub evidence: Vec<Evidence> }

pub struct RegionUsage {                  // embedded view (ADR-0016)
    pub name: String, pub origin: u64, pub length: Option<u64>, pub attributes: String,
    pub used: u64, pub free: Option<u64>, pub utilization: Option<f64>,
    pub contributors: Vec<SummaryRow>,    // sections (and their load images) placed in the region
    pub source: Evidence,                 // LinkMap | LinkerScript | UserProvided | Heuristic
}

pub struct BerkeleySizes { pub text: u64, pub data: u64, pub bss: u64 }  // `size` (Berkeley) compatible
```

Queries on `Session`:

| Query | Returns |
|---|---|
| `symbols(filter)` | all symbols matching a name pattern / kind / section (raw or demangled, language-normalized) |
| `by_symbol(name)` | `SymbolResult` (0..n matches; overloads and locals are not collapsed) |
| `summary(dimensions)` | `Summary` for the requested grouping |
| `memory_regions()` | `Vec<RegionUsage>` |
| `berkeley_sizes()` | `BerkeleySizes` |

## 3. Byte attribution (the invariant)

Per address space (VM, file, load), independently:

1. **Base partition = sections.** Every byte of the image is either inside exactly one section or an unattributed gap
   named by kind: `headers` (ELF/PE/Mach-O headers, load commands), `alignment` (between sections), `non-section`
   (file bytes outside any section, e.g. symbol/string tables not described as sections on Mach-O). `.bss` exists only
   in VM space; `.debug_*` only in file space; `load` only for bytes stored at a load address (ADR-0016).
2. **Within a section, symbols partition the bytes they cover.** Symbols are intervals `[address, address+size)`.
   - **Identical intervals** (aliases, ICF folds, C1/C2 constructors) form one *group* that owns the bytes once;
     every member lists the others in `aliases`; the bytes count toward `shared`.
   - **Nested intervals** (a sized symbol fully inside a larger one, e.g. a local label with a size): the **outermost**
     symbol owns the bytes; the inner one is reported with its own size but contributes nothing to totals.
     (ADR-0022)
   - **Partial overlaps** that are neither identical nor nested are malformed input: the earlier-starting symbol owns
     the overlap, and a `symbol-overlap` diagnostic names both.
   - **Gaps** between symbols inside a section become an `unattributed` row `(section, "no symbol")` — alignment
     padding, literal pools, anonymous data. Never silently absorbed into a neighbour.
3. **Invariant, tested on every fixture:** `Σ rows + Σ unattributed == total` in each space, and folded bytes appear
   exactly once.

**As implemented (slice 2, ✅ verified on all 382 fixture binaries and the game engine):**
- **Universe per space** = union of segment and section extents; file space = the whole input (the slice, for a
  universal Mach-O). A mapping with **no access** (Mach-O `__PAGEZERO`) reserves address space but occupies none, so
  it's excluded from VM totals — on the engine's `engine_player` the VM total then equals the sum of the real
  segments reported by `size -m`.
- **PE** has no segments; the plugin models the loader's view: one `SizeOfImage` mapping plus the headers
  (`SizeOfHeaders`, stored and mapped at the image base).
- **Gap names:** `[headers]` covers exactly the container headers (`Image::headers_size`: ELF header + program
  headers, Mach-O header + load commands, PE `SizeOfHeaders`); the padding after them is `[alignment]`. Section-less
  named segments are reported by name (`[__LINKEDIT]`). File bytes after the last section and outside every segment
  (ELF section header table, PE certificates/overlay) are `[non-section]`.
- **`.tbss`** is a TLS template that ELF places at the address of the following section; it yields to overlapping
  sections silently. Any other section overlap is a `section-overlap` warning (none on the corpus).
- Rows with `[Section, Symbol]` sum per section to the `[Section]` row: in-section bytes no symbol covers are
  `[section, "[no symbol]"]` unattributed rows. Symbol rows carry the symbols' size evidence.
- TLS symbols are excluded: their values are offsets into the TLS block, not addresses.

Rationale for ADR-0022: Bloaty's "first label wins" makes totals depend on scan order. Outermost-owns is order
independent, matches how nested symbols arise (sub-labels, `$t`/`$d`-style markers, sized local labels in assembly),
and keeps inner symbols visible with their own sizes.

## 4. Symbol sizes and evidence

| Format | Size source, in precedence order | Evidence |
|---|---|---|
| ELF | `st_size` (0 = label: no size) → link map → next-symbol distance | SymbolTable / LinkMap / Heuristic |
| Mach-O | DWARF subprogram entry range / `DW_OP_addr` variable type size → link map → next symbol in the same section | DebugInfo / LinkMap / Heuristic |
| PE (symbols from PDB) | procedure `len` (`S_GPROC32`/`S_LPROC32`) → DBI section contribution for data → next public in the same section | DebugInfo / Heuristic |

**Finding (slice 1, ✅ verified on 212 symbols):** ld-prime map sizes are *atom* sizes — the object plus the
alignment padding up to the next atom (`_g_outer`: 16-byte type, 24-byte atom because the next symbol is 32-byte
aligned; a 75-byte x86-64 `main` is an 80-byte atom). DWARF is therefore the exact size and ranks first; a map size
is used only when no debug info covers the symbol, and summaries (slice 2) must not double-count that padding as
symbol bytes. On Windows, PDB publics match `/MAP` `Rva+Base` for all 22 538 compared symbols.

**Literal pools (slice 2 finding, engine):** in sections the linker deduplicates (Mach-O `__cstring`, `__literal4/8/16`;
ELF `SHF_MERGE`, IR `SectionKind::Literals`), labels don't delimit objects and most are dropped, so no heuristic
size is inferred there: a `__cstring` label in the engine's release player otherwise "owned" 95 KB of unrelated
strings. Those bytes are `[no symbol]`.

A heuristic size never crosses a section end, never swallows the next symbol, and is never used for totals when a
non-heuristic source exists for the same bytes.

## 5. Link maps and memory regions

**Attachment (ADR-0023).** Link maps are optional enrichment (ADR-0006). The library attaches them only
explicitly: `session.attach_link_map(source)`; a registered `MapFileParser` is chosen by sniffing. The facade offers
`libstratum::find_link_map(image_path)` that returns `<image>.map` / `<stem>.map` if present, so frontends can opt in
to the convention — but `open_path` never attaches implicitly. Reason: a stale map next to a rebuilt binary silently
corrupts sizes. On attach, the map is checked against the image (placed symbols must exist at the same addresses in a
sample); a mismatch is a `map-image-mismatch` error and the map is ignored.

**Parsers (M3):** GNU ld (sections, input sections, symbols, `Memory Configuration`, discarded sections), lld ELF
(VMA/LMA/Size table), ld-prime (verified format, incl. dead-stripped symbols), MSVC `/MAP` and lld-link `/MAP`
(`section:offset` publics with `Rva+Base`, `Lib:Object`). Each is version-sniffed; unknown layouts fail with
`map-parse-error`, never guess.

**Memory regions, source precedence (ADR-0016):** attached map `Memory Configuration` → attached linker script
(`MEMORY { … }` block only) → regions supplied through the API → heuristic clustering of `PT_LOAD` segments
(`Evidence::Heuristic`; no length, so no utilization). `*default*` entries are ignored.

**Region usage semantics:** a section occupies a region in each space whose address falls in it:
`.text`/`.rodata` → FLASH (VM = load); `.data` → RAM (VM) **and** FLASH (load); `.bss`/`.noinit` → RAM (VM only).
`used` sums distinct bytes (overlapping claims counted once); `free = length − used`.

**`size`-compatible totals:** `text` = allocated sections without write permission (code, rodata, vectors, exidx),
`data` = allocated, writable, with file contents, `bss` = allocated without file contents. This is the Berkeley rule
of GNU `size` 📚 — **verified in M3** by comparing with `arm-none-eabi-size` / `size` in the Docker cross-check on
every ELF fixture before it's relied on. Access comes from the format (ELF `SHF_WRITE`/`SHF_EXECINSTR`, Mach-O segment
`initprot`, PE section characteristics). On Mach-O the same rule reproduces `size -m`'s per-segment totals
(✅ engine release player: text 3 906 004 = `__TEXT`, data = `__DATA_CONST` + initialized `__DATA`, bss = zero-fill).

## 6. Folds (ICF) and instantiations

- **Aliases:** function symbols with identical `(section, address, size)`.
- **`folded = true`** only when the alias group contains *distinct* functions: demangled names that differ after
  removing constructor/destructor variant markers (C1/C2, D0/D1/D2) and MSVC `_Function`-style public aliases seen in
  M0 S7. Verified rules from M0 S8 (lld tombstones, GCC `-fipa-icf` without `low_pc`) enrich evidence in M4.
- **Instantiation groups:** demangled "short" names (template arguments removed) + DWARF declaration location; report
  count, total size and per-instance sizes. **ADR-0024:** ship in M3 only for ELF/Mach-O via DWARF; PDB
  instantiation grouping follows in M4 with the function/module work it depends on.

## 7. Compile-unit dimension

Summaries by unit need address → unit mapping, originally planned for M4. Moved into M3:
- DWARF: `.debug_aranges` when present and consistent, else unit `DW_AT_ranges` / `low_pc`+`high_pc`.
- PDB: DBI **section contributions** (module index per section range) — they also cover data, which DWARF units don't
  describe; for DWARF, data symbols get their unit from `DW_TAG_variable` locations where available, else "unknown unit".
- Mach-O debug map: the `N_OSO` object name is the unit for the symbols between its `SO` markers.

## 8. Fixtures and verification

- New fixtures: `sizes/` (functions of known sizes, `static` duplicates, an identical-body pair for ICF, a template
  instantiated three times, initialized and zero-initialized data, a `.noinit`-style section for embedded) built on all
  10 toolchains, with ICF enabled where the linker supports it (lld `--icf=all`, `/OPT:ICF`, ld-prime `-deduplicate`).
- Goldens for `summary([Section, Symbol])`, `memory_regions()`, `berkeley_sizes()` per fixture.
- Cross-checks (extend `fixtures/crosscheck`): symbols vs `llvm-nm -S` / `llvm-pdbutil dump -publics`; section sizes
  vs `llvm-objdump -h`; Berkeley totals vs `size`/`arm-none-eabi-size`; region usage vs the GNU ld map.
- **Exit:** invariant holds on every fixture in every space; Berkeley totals equal `size`; region usage equals the map;
  fold groups match the M0 S1/S8 verified behavior per linker.

## 9. Slices

1. Symbols API + sizes with evidence (ELF, Mach-O incl. DWARF subprogram ranges, PDB procs/publics) + aliases.
2. Section summary + attribution invariant + Berkeley totals.
3. Map parsers + `attach_link_map` + regions + `RegionUsage`.
4. Unit dimension (aranges, section contributions, debug map).
5. ICF classification + instantiation groups; `sizes/` fixtures; cross-checks; goldens.

## 10. Decisions needed

| ADR | Decision | Recommendation |
|---|---|---|
| 0022 | Ownership of overlapping symbol intervals | Identical → shared group; nested → outermost owns; partial → earlier owns + diagnostic |
| 0023 | How link maps attach | Explicit `attach_link_map`; `find_link_map` helper; never implicit in `open_path` |
| 0024 | Instantiation groups for PDB in M3 or M4 | DWARF in M3, PDB in M4 |
