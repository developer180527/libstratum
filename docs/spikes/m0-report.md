# M0 spike report

Verification spikes from [07-roadmap.md §M0](../07-roadmap.md#m0-foundations-and-verification-spikes).
Host: macOS 27 arm64, Apple clang 21.0.0 (`clang-2100.1.1.101`), Apple ld `ld-1267` (ld-prime), 2026-09-15.

| # | Spike | Status |
|---|---|---|
| S1 | Apple ld-prime identical-function folding scope | ✅ done |
| S2 | Mach-O LTO object in the debug map | ✅ done |
| S3 | Thumb bit and mapping symbols in Arm ELF | ✅ done (objects and linked images) |
| S4 | Windows CodeView in COFF objects | ✅ partial; PDB needs a linker (Windows CI) |
| S5 | Reuse `addr2line`? (Q1) | ✅ decided |
| S6 | Fixture pipeline findings (dSYM, OSO paths, type loss) | ✅ done |
| S7 | PDB reader choice (Q14), `/OPT:ICF` in PDBs | ⏳ fixtures ready (msvc, clang-cl on GitHub Actions); spike next |
| S8 | lld ICF aliases and tombstones; GCC `-fipa-icf`; GCC LTO early debug | ✅ done (Debian server, Docker) |
| S9 | `.data` LMA/VMA from `PT_LOAD`, embedded map `Memory Configuration` | ✅ done |
| S10 | Linux and embedded fixture corpus | ✅ 176 builds, verified |

## S1: ld-prime folding scope ✅

Program: identical bodies as external functions, `static` functions, `inline` (linkonce_odr) functions, and two
template instantiations (`tmpl<long>`, `tmpl<unsigned long>`), all `noinline`, compiled `-O2 -g`.

| Link command | External (`ext_a`, `ext_b`) | `static`, `inline`, template instances |
|---|---|---|
| `clang++ u1.o m.o` (no `-O` at link) | distinct | **distinct** |
| `clang++ -O2 u1.o m.o` | distinct | **all 6 folded to one address** |
| `clang++ … -Wl,-deduplicate` | distinct | all 6 folded |
| `clang++ … -Wl,-no_deduplicate` | distinct | distinct |

**Conclusions**
- ld-prime folds identical functions that aren't externally visible (local, linkonce/weak, template instances). It did **not** fold strong external functions in this test.
- Folding depends on **whether the link step receives `-O`**. Build systems that pass optimization flags at link time (CMake Release typically does) get folding by default. This **corrects [04 §3 fact 5](../04-formats-and-toolchains.md#3-mach-o)**: that experiment linked without `-O`.
- Folded symbols stay in the symbol table as aliases at one address → the `Folded` provenance detection from the symbol table works on Mach-O.

## S2: LTO debug map ✅

| ThinLTO link | Debug map `OSO` entries | Debug info recoverable? |
|---|---|---|
| without `-object_path_lto` | **empty paths** | **No** (temporary LTO objects deleted; no dSYM produced at a link-only step) |
| `-Wl,-object_path_lto,ltoobj` | `ltoobj/0.arm64.thinlto.o`, `1.arm64.thinlto.o` | Yes: `dsymutil` works (5 subprograms) |

**Consequence:** diagnostic `lto-object` must fire on empty or missing OSO paths with the hint `-Wl,-object_path_lto,<dir>` or "run dsymutil at link time".

## S3: Thumb bit ✅ (objects)

`clang --target=thumbv7em-none-eabihf -O2 -g -c`: raw ELF `st_value` of `thumb_fn` is **`0x1`** (size 16, STT_FUNC); data symbols are even; a `$t` mapping symbol is present. **`llvm-nm` hides bit 0** (prints `00000000`), so tools that trust `nm` output silently normalize, while raw parsers don't. Confirms `libstratum_arch::split_thumb_bit` and mapping-symbol filtering. Linked images with GNU ld and lld: S9.

## S4: CodeView in COFF objects ✅ partial

Apple clang cross-compiles `--target=x86_64-pc-windows-msvc -gcodeview` objects with `.debug$S` (symbols, lines) and `.debug$T` (types), and MSVC-mangled names (`?g_packet@@3UPacket@@A`). Linking to PE+PDB needs `lld-link` or `link.exe`, which aren't available on this host. `/Z7`-style objects are a legitimate debug location ([11 §2.3](../11-platforms.md#23-msvc-toolchain-behaviors-that-affect-correlation)); PDB work proceeds on the Windows runner (S7).

## S5: `addr2line` reuse (Q1) ✅ decided

`addr2line::Context` offers `from_dwarf`/`from_sections`, `find_frames` (address → inline frames), `find_location(_range)`, and callback-driven split DWARF loading. It exposes **no** line → address lookup and **no** iteration over functions or inline trees. Our `DebugReader` needs `functions()`, `inline_trees()` and `line_table()` for source → binary queries and lens joins, and PDB needs its own implementation anyway.

**Decision:** implement DWARF inline trees and line tables directly on `gimli` in `libstratum-debug-dwarf`. Use `addr2line` as a **dev-dependency test oracle**: for every fixture address, our inline chain must equal `find_frames`.

## S6: Fixture pipeline findings ✅

1. **`dsymutil` exits 0 while producing an empty dSYM** when it can't open OSO objects (warnings only). The build driver now fails on those warnings. The library needs the same care: an image whose dSYM has zero units must produce `oso-object-missing`/`no-debug-info` instead of being treated as "no types".
2. **Relative OSO paths** (`-Wl,-oso_prefix`) are resolved by `dsymutil` relative to its **working directory**, not the binary. `FsLocator` must try relative to the binary's directory first, then the host-provided search paths.
3. **Type info disappears with optimized-out variables.** In `odr_conflict`, `static Config g_config_a` unused beyond constant reads was removed at `-O2`, and Clang emitted **no** `Config` type at all. The layout lens's "type not found" must mention that types used only by optimized-out entities may not be emitted. Fixture fixed to use external linkage.
4. Apple fixture builds are **bit-for-bit reproducible** (binary and dSYM hashes identical across rebuilds with `ZERO_AR_DATE=1`, `-ffile-compilation-dir=.`, `-oso_prefix`).
5. ODR conflict captured: the dSYM holds **two** `Config` definitions (32 and 16 bytes) on arm64 and x86_64 at both `-O0` and `-O2`.


## S8: lld and GCC folding, tombstones, GCC LTO ✅

Host: Debian 13 x86_64, Docker image `libstratum-fixtures` ([TOOLCHAINS.md](../../fixtures/build/TOOLCHAINS.md)).
Program: identical external `ext_a`/`ext_b`, identical `static` `st_a`/`st_b`, unused `unused_fn`; `-O2 -g -ffunction-sections`.

**lld 19 `--icf=all --gc-sections`**
- Folds **external and static** identical functions: `ext_a`, `ext_b`, `st_a`, `st_b` all at `0x1700` (unlike ld-prime, which kept strong externals separate).
- **Folded symbols remain in `.symtab` as aliases** at the representative address. ✅ confirms the fold-detection rule.
- `--print-icf-sections` and `--print-gc-sections` print machine-readable-ish lists (selected/removed sections), a useful optional input.
- **DWARF:** the representative (`ext_a`) keeps `low_pc 0x1700`; the folded-away functions (`ext_b`, `st_a`, `st_b`) **and** the garbage-collected `unused_fn` all get **`low_pc = 0`** (tombstone).

**Consequence (refines [05 §4 and §6](../05-correlation.md#4-identical-code-folding-icf)):** a tombstoned DWARF subprogram alone does **not** tell folded from discarded. Rule: tombstoned subprogram + a symbol-table alias at a live address → `Folded` (evidence: symtab alias, `low_pc` tombstone); tombstoned and no symbol → `DiscardedByLinker`.

**GCC 14 `-O2` + GNU ld `--gc-sections`**
- `-fipa-icf` merged the identical **static** functions (`st_a`, `st_b` → one address, both symbols kept), but **not** the external ones (`ext_a` at `0x1160`, `ext_b` at `0x1170`, identical code, separate copies).
- DWARF for GCC-merged `st_a`/`st_b`: subprograms have **no `low_pc` at all** (neither real nor tombstone). The merge is only visible through the symbol table.
- GNU ld tombstones the gc-ed `unused_fn` with `low_pc = 0`.

**GCC LTO (`-flto -g`)**
- Final DWARF has an `<artificial>` compile unit (the LTRANS output) whose concrete and inlined entries point via `DW_AT_abstract_origin` into the early-debug units: **9 `DW_FORM_ref_addr` cross-unit references** in a two-file program.
- ✅ Confirms [ADR-0005](../08-decisions.md)'s design requirement: `DebugRef` must be global (cross-unit), and function names/decl locations come from the abstract origin in another unit.

## S9: embedded load vs run addresses ✅

Cortex-M4, GNU ld, linker script with `.data > RAM AT > FLASH`, one initialized global, one zeroed global:

| Segment | VirtAddr (run) | PhysAddr (load) | FileSiz | MemSiz |
|---|---|---|---|---|
| RX (`.text`, `.rodata`) | `0x08000000` | `0x08000000` | 0x28 | 0x28 |
| RW (`.data`, `.bss`) | `0x20000000` | **`0x08000028`** | 0x4 (`.data`) | 0x8 (+ `.bss`) |

- ✅ ADR-0016 holds **without a map file**: per-section load address = segment `p_paddr` + (section `sh_addr` − segment `p_vaddr`). Flash cost = FileSiz of load segments; RAM cost = MemSiz of RW segments.
- The GNU ld map states it directly (`.data 0x20000000 0x4 load address 0x08000028`) and lists `Memory Configuration` (FLASH `0x08000000` len `0x100000` `xr`, RAM `0x20000000` len `0x30000` `xrw`, plus `*default*`, which the parser must ignore).
- The corpus's `layout/` programs have only zero-initialized globals (RW FileSiz 0). A dedicated `memory/` fixture with initialized data is needed for the M3 memory-region lens.
- Linked Thumb images: function symbols are odd (`main` at `0x08000009`, `reset_handler` `0x08000029`), 27 mapping symbols (`$t`, `$d`) in a small binary. ✅ S3 extends to linked images.

## S10: Linux and embedded corpus ✅

| Toolchain | Builds | Notes |
|---|---|---|
| linux-clang (x86_64, aarch64) | 32 | DWARF 5, build IDs |
| linux-gcc (x86_64, aarch64) | 32 | DWARF 5, build IDs |
| arm-none-eabi-gcc (thumbv6m, thumbv7em, thumbv8m) | 48 | DWARF 5, build IDs, FLASH/RAM map |
| arm-llvm (same CPUs) | 48 | DWARF 5, lld |
| riscv-gcc (rv32imac) | 16 | DWARF 5 |

Every toolchain keeps all layout types at `-O2`, and both conflicting `Config` layouts (32 and 16 bytes) survive on GCC and Clang. arm-llvm and riscv-gcc report 4 structure DIEs for `odr_conflict` instead of 2 (likely declaration + definition pairs): to examine in M2.

**Toolchain quirks found while building (all encoded in `build.py`)**
1. **Clang mangles `main` under `-ffreestanding` in C++** (treats it as an ordinary function); GCC doesn't. LLVM embedded builds use `-fno-builtin` instead.
2. Global destructor registration differs: GCC Arm EABI calls `__aeabi_atexit`, Clang calls `__cxa_atexit`.
3. C++ can't place a function pointer in a `void*` constant table; vector tables live in C (`vectors.c`).
4. Debian's LLVM lacks bare-metal compiler-rt builtins; soft-float Cortex-M links need `libgcc` (`__aeabi_d2iz`).
5. The server's Tailscale MagicDNS had no upstream resolvers; fixed by enabling global nameservers with "Override DNS servers" (environment note, not a stratum finding).

## S11: Windows fixtures via GitHub Actions ✅

First run: every build succeeded, but MSVC's default static CRT (`/MT`) put the entire CRT into each PDB and map
(3.6–5 MB PDBs, 10 MB of maps per 16 builds), and `/DEBUG` enabled incremental linking (29.6 MB of `.ilk`).
With `/MD` and `/INCREMENTAL:NO`: 64 builds (msvc and clang-cl × x64, arm64 × O0, O2), about 35 MB of committed
PDB + map + exe + build.json. Machine types verified (`x86-64`, `Aarch64`). The `RSDS` PDB path is the runner's absolute path.

## Blocked spikes: what's needed

| Spike | Needs | How |
|---|---|---|
| clang-cl cross-build on macOS (Q15) | `lld-link` + Windows SDK/CRT (e.g. `xwin`, which requires accepting Microsoft's license) | User decision |
