# 06 — Testing strategy

stratum's value is trust. Correlation logic is verified against **committed binaries** and **golden outputs**, not against whatever compiler happens to be installed.

## 1. Fixture corpus

```
fixtures/
  src/                       # tiny single-purpose C/C++ programs
    layout/                  #   padding, bitfields, EBO, vptr, virtual bases, anon unions, flex array, alignas, no_unique_address, ODR conflict via #ifdef
    inline/                  #   always_inline chains, 3-level nesting, inlined into multiple callers, tiny vanishing inline
    icf/                     #   identical bodies (external, static, weak, linkonce_odr), C1/C2 ctor aliases
    template/                #   one template × N types; static inline in header across CUs
    elim/                    #   constant-false branch, unused static, unused external + dead_strip/gc-sections
    lto/                     #   cross-TU inlining under -flto / -flto=thin
    split/                   #   -gsplit-dwarf, .dwp
    macho/                   #   dSYM vs debug-map-only; archive member OSO; missing OSO; universal binary
    misc/                    #   no -g; stripped; mismatched debug file; compressed debug sections
  build/
    build.sh                 # the single entry point; deterministic flags, pinned toolchains
    toolchains.toml          # exact versions: Apple clang (Xcode build), LLVM clang/lld (Docker image digest)
    Dockerfile.elf           # pinned LLVM for ELF fixtures
  bin/<toolchain>/<arch>/<opt>/<fixture>/   # COMMITTED outputs: binary, .dSYM/.dwo/.dwp, map file, build.log
  golden/<same path>/*.json                  # expected libstratum-model output per query
```

### Why commit binaries
Compiler output changes between versions. If CI rebuilt fixtures, golden files would break on toolchain updates for reasons unrelated to our code. Binaries change **only** when someone deliberately re-runs `build.sh`. The diff of golden files then shows exactly how the new toolchain differs, which is itself valuable research.

- Use Git LFS if the corpus exceeds ~50 MB.
- Reproducibility flags: `-ffile-compilation-dir=.`, `-fdebug-prefix-map`, `-Wl,-oso_prefix,.` (Mach-O), `ZERO_AR_DATE=1`, fixed source paths.

## 2. Toolchain matrix (v1, per [ADR-0019](08-decisions.md#adr-0019-v1-platform-matrix))

| Platform | Toolchains | Arches | Built on |
|---|---|---|---|
| Windows PE+PDB | MSVC (`/Zi` and `/Z7`; `/O2 /DEBUG`, with and without `/OPT:REF /OPT:ICF`; `/GL /LTCG`), clang-cl + lld-link | x64, arm64 | Windows CI runner (MSVC); clang-cl optionally cross-built (Q15) |
| Linux ELF+DWARF | Clang + lld, GCC + GNU ld | x86_64, aarch64 | Docker (pinned image digests) |
| macOS Mach-O+DWARF | Apple Clang + ld-prime (dSYM and debug-map variants) | arm64, x86_64 | macOS runner (SDK pinned via `-isysroot`) |
| Embedded ELF+DWARF | arm-none-eabi-gcc, LLVM/ATfE clang; GCC and Clang for RV32 | thumbv6m, thumbv7em (hard float), thumbv8m, rv32imac | Docker |

| Axis | Values |
|---|---|
| Optimization | `-O0`/`/Od`, `-O2`/`/O2`, `-Os`/`-Oz` (embedded), LTO/LTCG |
| Linker | default; gc-sections / dead_strip / `/OPT:REF`; ICF (`--icf=all`, `/OPT:ICF`, `-deduplicate`) |
| Debug | DWARF 5 (+ a DWARF 4 subset), split DWARF, `-fstandalone-debug` vs default; PDB via `/Zi` and `/Z7` |

Not every fixture runs every combination; `toolchains.toml` declares the set per fixture directory. **Every M2 layout fixture must exist on all four platforms**, since it's the abstraction proof.

## 3. Test layers

| Layer | What | Tooling |
|---|---|---|
| Unit | Hole computation, bitfield math, map-file parsing, name normalization, range merging | `cargo test`, `proptest` for layout math (random member lists → invariants: no overlaps unless allowed, holes + members + tail = size) |
| Plugin conformance | Every `BinaryFormat` passes one shared test suite (sections cover file, symbols are in sections, identity stable, probe doesn't panic on random bytes) | Generic test harness in `libstratum-core::spi::testing` |
| Golden | Query outputs over the fixture corpus | `insta` JSON snapshots; review changes with `cargo insta review` |
| Cross-check | `fixtures/crosscheck/crosscheck.py` (Docker image) compares every golden layout with `llvm-dwarfdump`, `pahole` and `llvm-pdbutil`. Also compare with reference tools where they overlap: `dwarfdump`/`llvm-dwarfdump`, `pahole`, `llvm-pdbutil`, `/d1reportSingleClassLayout` (layouts); `addr2line -i`, `atos`, `llvm-symbolizer` (inline frames, including PDB); `nm`/maps/`arm-none-eabi-size` (symbols, sizes) | Scripts in `fixtures/crosscheck/`, run on demand and in nightly CI |
| Robustness | Truncated or corrupt inputs never panic | `cargo fuzz` targets for probe/open, DWARF unit walk, map parsers |
| API contract | JSON Schemas generated from `libstratum-model` match the committed `schemas/`; semver checks | `schemars`, `cargo-semver-checks` |
| Performance | See §4 | `criterion` |

## 4. Performance benchmarks

Real-world binaries (not committed; downloaded or built by a script): a large Windows C++ build with a multi-GB PDB (e.g. the game engine or Chromium), an LLVM `clang` debug build (several GB of DWARF), a macOS app with dSYM, and firmware images (Cortex-M, RV32).

Tracked metrics:
- `open()` time and RSS (target: sub-second for multi-GB debug info because indexing is lazy)
- first `struct_layout` hover (cold) and repeat (warm)
- first `by_address` (cold) and a 1000-address batch
- full `section_summary`
- full `snapshot`

Regressions over 10% fail nightly CI.

## 5. Correctness bar per milestone

A query family is "done" when:
1. Every relevant fixture × matrix cell has a reviewed golden file.
2. Cross-checks agree, or each disagreement is documented with a reason in the golden file's metadata.
3. Every limit listed in [05](05-correlation.md) has a fixture that demonstrates the honest output.
