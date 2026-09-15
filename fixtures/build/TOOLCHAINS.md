# Pinned fixture toolchains

Fixture binaries in `fixtures/bin/` were produced with exactly these tools. Each output's `build.json`
repeats the versions for that build. Rebuild only deliberately, and update this file when a version changes.

## macOS host (apple-clang)
| Tool | Version |
|---|---|
| Apple clang | 21.0.0 (`clang-2100.1.1.101`) |
| Apple ld (ld-prime) | `ld-1267` |
| SDK | Xcode `MacOSX.sdk` (26.5), pinned via `-isysroot`, not the Command Line Tools SDK |

## Linux and embedded (Docker image `libstratum-fixtures`, built 2026-09-15)
Image id `sha256:eadb7cad6b17c3fc2bf9c3ad52cccef1c74507523e24287e94e353ed55ece360`, base Debian 13.6 (trixie).

| Toolchain | Tool | Version |
|---|---|---|
| linux-clang, arm-llvm | clang / ld.lld | Debian clang 19.1.7 (3+b1) / LLD 19.1.7 |
| linux-gcc | x86_64-linux-gnu-gcc, aarch64-linux-gnu-gcc | GCC 14.2.0 (Debian 14.2.0-19) |
| arm-none-eabi-gcc | arm-none-eabi-gcc | 14.2.1 20241119 (15:14.2.rel1-1) |
| riscv-gcc | riscv64-unknown-elf-gcc | 14.2.0 (14.2.0+19) |
| (cross-check) | pahole | v1.30 |

arm-llvm links GCC's `libgcc` for the matching CPU (compiler-rt bare-metal builtins aren't packaged).

## Windows (msvc, clang-cl)
Not built yet. To be recorded after the Windows session (Visual Studio Build Tools version, MSVC toolset, clang-cl/lld-link version).
