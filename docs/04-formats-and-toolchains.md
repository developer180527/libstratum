# 04 — Formats and toolchains

Legend: **✅ verified** locally (Apple clang 21.0.0 `clang-2100.1.1.101`, Apple `ld-1267` (ld-prime), arm64, macOS 27, 2026-09-15) · **📚 sourced** from documentation (must be re-verified by fixtures in M0) · **❓ open**, to be investigated.

## 1. What a correlation needs, per format

| Need | ELF | Mach-O |
|---|---|---|
| Segments / sections | Program headers + section headers | `LC_SEGMENT_64` containing sections |
| Symbols + sizes | `.symtab` / `.dynsym` with `st_size` | `LC_SYMTAB` nlist; **no sizes** → derive from next symbol or from the section end (map file gives exact sizes ✅) |
| Identity | `NT_GNU_BUILD_ID` note (Clang on ELF may need `-Wl,--build-id` 📚) | `LC_UUID` (always present 📚) |
| Debug info location | Embedded `.debug_*`; split `.dwo`/`.dwp`; `.gnu_debuglink`; build-ID directory | `.dSYM` bundle, **or** debug map (stabs `N_OSO`) pointing to `.o` files |
| Linker map | GNU ld `-Map`, lld `-Map` (different formats) | `-Wl,-map,<file>` (ld64 / ld-prime format) |

## 2. ELF

### Debug info location (priority order in `FsLocator`)
1. Embedded `.debug_*` sections (possibly compressed: `SHF_COMPRESSED` with zlib or zstd; `object` handles this 📚).
2. **Split DWARF** (`-gsplit-dwarf`): skeleton units in the binary point to `.dwo` files through `DW_AT_dwo_name` + `DW_AT_comp_dir`, or to a packaged `.dwp`. 📚 (`addr2line` already supports this.)
3. `.gnu_debuglink` (file name + CRC32) searched in the binary's dir, `.debug/`, and `/usr/lib/debug/`.
4. Build-ID path `/usr/lib/debug/.build-id/xx/rest.debug`.
- **Identity check:** the build ID must match; otherwise emit a `debug-info-mismatch` diagnostic and refuse (unless `strict_identity=false`).

### Linker behaviors that affect correlation
- **Garbage collection** (`--gc-sections`, needs `-ffunction-sections -fdata-sections`): discarded functions **keep their DWARF**, but relocations to them resolve to a **tombstone** value. lld uses 0 for `.debug_info` (with `-1` planned) and 1 for pre-v5 `.debug_ranges`/`.debug_loc`. `.debug_line` is excluded from tombstoning for ICF-folded code. 📚
  → **Rule:** a subprogram with `low_pc` ∈ {0, 1, −1, −2} (and `0` not inside any executable section) is not code at address 0. ✅ Verified with lld 19 and GNU ld (M0 S8), which both use 0; distinguishing folded from discarded needs the symbol table (see [05 §4](05-correlation.md#4-identical-code-folding-icf)).
- **ICF:** lld `--icf=all|safe|none`. `safe` relies on `.llvm_addrsig` (address-significance) sections. **Folded symbols stay in `.symtab` as aliases at the representative address.** ✅ (lld 19, M0 S8) `--print-icf-sections` lists folds. 📚 gold has ICF; GNU ld (bfd) has none; mold has `--icf=all`. 📚
  → **Rule:** multiple function symbols with identical `(start, size)` form a `Folded` group. DWARF for the non-representative functions may point at the same address or be tombstoned.
- **Map formats:** GNU ld and lld maps differ and **carry no stability guarantee** (there are requests for a JSON map, but none exists). 📚 Treat map parsers as best-effort, version-sniffed plugins.

## 3. Mach-O

### Debug info model (Apple's "lazy DWARF")
- The compiler writes DWARF into the `.o`. **The linker does not copy DWARF into the executable.** It writes a **debug map**: stabs entries (`SO` source, `OSO` object path + mtime, `FUN`/`STSYM` symbol addresses) so tools can relocate `.o` DWARF to final addresses. ✅📚
- `dsymutil` reads the debug map, links the `.o` DWARF, and writes a `.dSYM` bundle with final addresses. The binary's `LC_UUID` is copied into it. ✅📚
- LLDB finds dSYMs by UUID. Without them, it reads the `.o` files through the OSO paths, **so those files must still exist**. 📚

### Verified facts that shape the design ✅

1. **The executable contains no `__DWARF` segment.** Only the debug map (`nm -ap` shows `SO`, `OSO`, `FUN` entries).
2. **Single-step `clang++ -g file.cpp -o app` writes the `.o` to a temp dir, runs `dsymutil` automatically, then deletes the `.o`.** So `app.dSYM` exists, but the binary's OSO entries point to a deleted file. If that dSYM is lost or not shipped, running `dsymutil` again fails ("unable to open object file… no debug symbols in executable"), and **there is no debug info at all**. Separate compile and link steps (`-c`, then link) produce **no** dSYM automatically; debug info then lives only in the `.o` files.
   → The locator must try the dSYM first, then OSO objects, and emit `oso-object-missing` when both fail. User docs must explain both build styles. Fixture builds compile with `-c` and run `dsymutil` explicitly, so we test both the dSYM and the debug-map paths.
3. **`dsymutil` drops DWARF for dead-stripped functions.** With `-Wl,-dead_strip`, `unused_fn` has a subprogram DIE in `b.o` but **zero occurrences** in `b_o2.dSYM`.
   → On Mach-O, `DiscardedByLinker` can only be proven from (a) the `.o` DWARF via the debug map, or (b) the link map's `# Dead Stripped Symbols:` section. A dSYM alone can't show it.
4. **The ld-prime `-map` format** (tab-separated):
   ```
   # Path: b_o2
   # Arch: arm64
   # Object files:
   [  0] linker synthesized
   [  1] /…/b.o
   # Sections:
   # Address	Size    	Segment	Section
   0x1000003F8	0x00000030	__TEXT	__text
   # Symbols:
   # Address	Size    	File  Name
   0x100000498	0x00000010	[  1] __Z2h1l
   0x100000574	0x00000014	[  1] literal string: %d %ld %ld %d %lld\n
   # Dead Stripped Symbols:
   #        	Size    	File  Name
   <<dead>>	0x0000000C	[  1] __Z9unused_fnv
   ```
   It gives exact symbol sizes (Mach-O nlist lacks them), object-file attribution, literal/stub/GOT entries, and the dead-stripped list. **It's the most valuable optional input on Mach-O.**
5. **⚠️ Corrected by [M0 spike S1](spikes/m0-report.md#s1-ld-prime-folding-scope-):** that link had no `-O`; with `-O2` at link time, ld-prime folds identical local/inline/template functions (not strong external ones). Original note: **identical functions were not folded** by ld-prime at `-O2` in a default link: `h1` and `h2` (identical bodies) kept distinct addresses, as did `add3<int>` and `add3<long long>`. ICF-like dedup on Apple exists (`-deduplicate`/`-no_deduplicate`; drivers pass `-no_deduplicate` at `-O0` 📚), but its exact scope under ld-prime is ❓ → fixture M0 must test `-Wl,-deduplicate` explicitly and weak/`linkonce_odr` functions.
6. **Apple clang 21 emits DWARF 5** by default (`version = 0x0005`). An earlier assumption that Apple prefers DWARF 4 is **wrong** for current toolchains. Support v4 *and* v5 regardless (older Xcode, `-gdwarf-4`).
7. **Inlining:** `helper()` inlined into `main` shows `DW_TAG_inlined_subroutine` with `DW_AT_abstract_origin`, `low_pc/high_pc`, and `DW_AT_call_file/line/column`. But in another fixture, tiny `sq()` and `twice()` were inlined and folded away with **no inlined-subroutine DIE left**. → Inlined code isn't always observable (see [03 honesty note](03-data-model.md#outcome-source--binary)).
8. **Struct layout is directly in DWARF:** `Packet { char tag; double v; int n; short s; }` → `DW_AT_byte_size 0x18`, members at 0, 8, 16, 20. Holes and tail padding are computed from that.

### Locator order (Mach-O)
1. Explicit `--dsym` path.
2. `<binary>.dSYM/Contents/Resources/DWARF/<name>` next to the binary.
3. Debug map: OSO objects. Archive members appear as `lib.a(member.o)`. The OSO mtime is checked against the object file's mtime and flagged `oso-object-stale` on mismatch.
4. (❓ later) Spotlight/`DBGShellCommands`-style lookup. Not in v1.
- Paths inside the OSO and `DW_AT_comp_dir` may be sandbox/temporary paths (Bazel, remote caching). Support **prefix remapping** in `FsLocator`. 📚
- **Universal (fat) binaries:** `probe` returns `YesContainer`; `OpenOptions.arch` selects the slice. Default is the host arch if present, otherwise an error listing available slices.

### Mach-O debug-map address translation
For each OSO object: build `AddressMap` = {object symbol address → final address} from `FUN` (functions; the size comes from the paired end entry) and `STSYM` (static data) stabs. Addresses in the `.o` DWARF that fall inside a mapped symbol are translated; unmapped ones (dead-stripped) become `DiscardedByLinker` evidence. This is exactly what `dsymutil` does, and `addr2line`'s loader implements an "unpacked Mach-O" path that we should study or reuse. 📚

## 4. Architectures (x86_64, aarch64)

- DWARF register numbers and the call frame information (CFI) format differ, but **we don't unwind**, so this is mostly irrelevant.
- Both are 64-bit little-endian in our scope. The IR still carries `endian` and `address_size` so 32-bit ARM or big-endian can be added later without a model change.
- aarch64 Mach-O: `__stubs`, `__auth_stubs` (arm64e), `__got`, and linker-synthesized branch islands in large binaries show up as symbols from file `[0] linker synthesized` → classified as `Unattributed { reason: LinkerSynthesized }`.
- Struct layout ABIs: SysV x86_64 psABI and AAPCS64 (Apple arm64 has small deviations, e.g. in `long double` and some bitfield alignment rules). **We don't compute layout from ABI rules**, we read the compiler's decision from DWARF. ABI knowledge is used only for *reorder suggestions*, and those are marked `Evidence::Computed`.

## 5. Clang vs GCC

stratum reads standard DWARF, symbol tables and maps. **"Clang first" decides which toolchain produces our fixtures and whose quirks we handle first. It doesn't create a code dependency on Clang.**

| Area | Clang (incl. Apple Clang) | GCC | Consequence for stratum |
|---|---|---|---|
| Platforms | ELF and Mach-O; the only real option on macOS (`/usr/bin/gcc` on macOS **is** Apple clang ✅) | ELF in practice | Clang covers both formats with one fixture toolchain |
| Default DWARF | v5 (Apple clang 21 ✅; upstream on Linux 📚) | v5 since GCC 11 📚 | Support v4 and v5 |
| **Type info completeness** | `-fstandalone-debug` is the default **only on Darwin**. Elsewhere Clang omits type definitions that could be forward declarations, and emits a dynamic C++ class's type info **only in the CU that holds its vtable** ✅ (man page) | Emits fuller type info by default | Layout lookup must search **all** CUs for a definition; if only a declaration exists → `declaration-only-type` diagnostic with a hint (`-fstandalone-debug`) |
| Unreferenced types | Never emitted, even with standalone debug ✅ (man page) | Similar with `-feliminate-unused-debug-types` (default) | "Type not found" must suggest that the type may simply be unused |
| Inline info | `DW_TAG_inlined_subroutine` + call file/line/column ✅; very small inlines may disappear ✅ | Richer: location views, entry values, `DW_TAG_call_site` | Provenance model already allows missing frames |
| LTO debug info | Full LTO / ThinLTO; DWARF regenerated at link time, one CU per source still | "Early LTO debug" in `.gnu.debuglto_*` sections, with **cross-unit `DW_AT_abstract_origin` references** 📚 | GCC LTO needs cross-CU abstract-origin resolution: design `DieRef` to be global, not CU-local, **now** |
| ICF | Linker-driven (lld, ld-prime dedup); Clang may alias C1/C2 constructors | Also `-fipa-icf` **inside the compiler**, before the linker sees anything | GCC folds may leave one symbol with no aliases: fold detection from the symbol table is incomplete for GCC (documented limitation) |
| Bitfields | `DW_AT_data_bit_offset` | Modern GCC too; old GCC used `DW_AT_bit_offset` (big-endian relative) + `DW_AT_byte_size` | Support both encodings in the DWARF layer |
| Template names | Full names; optional `-gsimple-template-names` (names without args, rebuilt from template parameter DIEs) | Full names | Name normalization must be able to rebuild names from `DW_TAG_template_*_parameter` |
| Split DWARF | `-gsplit-dwarf`, `.dwp` via `llvm-dwp` | Same, plus pre-standard GNU extension format (DWARF 4) | ELF locator handles both |
| Mangling | Itanium on ELF and Mach-O (Mach-O adds a leading `_`) ✅ | Itanium | One demangler, strip the Mach-O `_` prefix |

**Adding GCC later** = add a GCC column to the fixture matrix (ELF only), then fix whatever the golden files expose. The architecture items above (global `DieRef`, both bitfield encodings, cross-CU declaration lookup, incomplete-fold disclaimer) are **designed in now** so that step needs no redesign.

## 6. Local environment note (this machine)

Linking with the Command Line Tools SDK fails against Xcode's linker (`tapi error: malformed file … unknown architecture arm64e.x1-macos`), because the CLT SDK is newer than the active linker understands. Workaround used for the experiments:

```bash
clang++ -isysroot /Applications/Xcode.app/Contents/Developer/Platforms/MacOSX.platform/Developer/SDKs/MacOSX.sdk -g -O2 -c a.cpp -o a.o
```

The fixture build script must pin its SDK path explicitly for the same reason.

## Sources

- Apple lazy DWARF scheme: https://wiki.dwarfstd.org/Apple's_%22Lazy%22_DWARF_Scheme.md
- dsymutil: https://llvm.org/docs/CommandGuide/dsymutil.html
- Understanding Apple debug info (OSO, dSYM, path pitfalls): https://www.smileykeith.com/2025/09/21/understanding-apple-debug-info/
- Clang `-fstandalone-debug`: local `man clang`; https://releases.llvm.org/6.0.0/tools/clang/docs/UsersManual.html
- lld tombstones and GC: https://maskray.me/blog/2021-02-28-linker-garbage-collection · https://releases.llvm.org/11.0.0/tools/lld/docs/ReleaseNotes.html
- Linker introspection options and map formats: https://maskray.me/blog/2022-02-27-analysis-and-introspection-options-in-linkers
- lld ICF options: https://manpages.debian.org/experimental/lld-10/ld.lld-10.1 · https://github.com/llvm-mirror/lld/blob/master/ELF/ICF.cpp · mold safe ICF: https://github.com/rui314/mold/issues/484
- GCC early LTO debug: https://gcc.gnu.org/legacy-ml/gcc-patches/2016-08/msg01842.html
- Apple ld `-no_deduplicate`: https://github.com/ldc-developers/ldc/issues/3521 · ld-prime: https://mjtsai.com/blog/2023/10/20/an-apple-library-primer/
- Bloaty build-ID and dSYM handling: https://github.com/google/bloaty/blob/main/doc/using.md
- DWARF 5 errata: https://dwarfstd.org/errata-dwarf5.html
