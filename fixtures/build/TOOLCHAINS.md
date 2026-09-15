# Pinned fixture toolchains

Fixture binaries in `fixtures/bin/` were produced with exactly these tools. Each output's `build.json`
repeats the versions for that build. Rebuild only deliberately, and update this file when a version changes.

## macOS host (apple-clang)
| Tool | Version |
|---|---|
| Apple clang | 21.0.0 (`clang-2100.1.1.101`) |
| Apple ld (ld-prime) | `ld-1267` |
| SDK | Xcode `MacOSX.sdk` (26.5), pinned via `-isysroot`, not the Command Line Tools SDK |

## macOS host (apple-clang-lto), built 2026-09-16
Bitcode inputs (`-flto=thin`) linked without a `-flto` flag, so the debug map gets the `n_strx == 0`
sentinel N_OSO (docs/04 §3). No `dsymutil` step: there are no object files to collect from.

| Tool | Version |
|---|---|
| Apple clang | 21.0.0 (`clang-2100.3.34.2`) |
| Apple ld (ld-prime) | LTO support using LLVM 21.0.0 (`ld -v`) |
| SDK | Xcode `MacOSX.sdk`, pinned via `-isysroot` |

⚠️ This is a **newer Apple clang build** than the one above that produced the `apple-clang` fixtures
(`clang-2100.1.1.101`). Harmless while the two groups test different things, but rebuilding `apple-clang`
on this host would change its goldens — rebuild both together, or pin deliberately.

## Linux and embedded (Docker image `libstratum-fixtures`, built 2026-09-15)
Image id `sha256:eadb7cad6b17c3fc2bf9c3ad52cccef1c74507523e24287e94e353ed55ece360`, base Debian 13.6 (trixie).

| Toolchain | Tool | Version |
|---|---|---|
| linux-clang, linux-clang-types5/4, arm-llvm | clang / ld.lld | Debian clang 19.1.7 (3+b1) / LLD 19.1.7 (types5/4: `-fdebug-types-section` with DWARF 5 / 4) |
| linux-gcc | x86_64-linux-gnu-gcc, aarch64-linux-gnu-gcc | GCC 14.2.0 (Debian 14.2.0-19) |
| arm-none-eabi-gcc | arm-none-eabi-gcc | 14.2.1 20241119 (15:14.2.rel1-1) |
| riscv-gcc | riscv64-unknown-elf-gcc | 14.2.0 (14.2.0+19) |
| (cross-check) | pahole | v1.30 |

arm-llvm links GCC's `libgcc` for the matching CPU (compiler-rt bare-metal builtins aren't packaged).

## Windows (msvc, clang-cl): GitHub Actions `windows-latest`, built 2026-09-16
Workflow run 35023865100 (`fixtures.yml`, commit `e375b67`); environment via `ilammy/msvc-dev-cmd` (x64, amd64_arm64).

| Toolchain | Tool | Version |
|---|---|---|
| msvc | cl / link | 19.51.36256 / 14.51.36256.0 |
| clang-cl | clang-cl / lld-link | clang 20.1.8 / LLD 20.1.8 (Visual Studio bundled) |

Both link the CRT dynamically (`/MD`) with `/INCREMENTAL:NO`; O2 builds add `/OPT:REF /OPT:ICF`.
Binaries' CodeView `RSDS` records keep the runner's absolute PDB path (`D:\a\libstratum\libstratum\fixtures\bin\...`),
which doesn't exist elsewhere, a realistic case for the PDB locator (fall back to the PDB next to the image).
