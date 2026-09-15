#!/bin/sh
# Seeds the open_any corpus with one small fixture per format and architecture.
set -eu
cd "$(dirname "$0")"
mkdir -p corpus/open_any
for f in \
  ../fixtures/bin/linux-gcc/x86_64/O2/packed/packed \
  ../fixtures/bin/linux-clang/aarch64/O2/packed/packed \
  ../fixtures/bin/arm-none-eabi-gcc/thumbv6m/O2/packed/packed \
  ../fixtures/bin/riscv-gcc/rv32imac/O2/packed/packed \
  ../fixtures/bin/apple-clang/arm64/O2/packed/packed \
  ../fixtures/bin/apple-clang/x86_64/O2/packed/packed.dSYM/Contents/Resources/DWARF/packed \
  ../fixtures/bin/msvc/x64/O2/packed/packed.exe \
  ../fixtures/bin/clang-cl/arm64/O2/packed/packed.exe; do
  cp "$f" "corpus/open_any/$(echo "$f" | tr '/' '_' | sed 's/^\.\._fixtures_bin_//')"
done
echo "seeded $(ls corpus/open_any | wc -l | tr -d ' ') inputs"
