# Fixtures

Committed binaries and golden outputs that libstratum is verified against (docs/06-testing.md).

```
src/        tiny single-purpose C/C++ programs, one tricky case each
build/      build scripts and pinned toolchain definitions (per platform)
bin/        COMMITTED build outputs: <toolchain>/<arch>/<opt>/<fixture>/ (binary, dSYM/PDB/.dwo, map, build.log)
golden/     expected libstratum-model JSON for each bin/ entry
```

Binaries change only when someone deliberately re-runs the build scripts, so golden diffs show real
toolchain differences, never incidental CI noise.

Status: `layout/` fixtures built for 10 toolchains (apple-clang, linux-clang, linux-clang-types5, linux-clang-types4,
linux-gcc, arm-none-eabi-gcc, arm-llvm, riscv-gcc, msvc, clang-cl): 334 builds, versions in
[build/TOOLCHAINS.md](build/TOOLCHAINS.md). Windows builds of `opaque` are pending the fixtures workflow.

`debugmap/` covers Mach-O debug-map shapes the layout corpus can't express, on macOS only: `thinlto`
(`apple-clang-lto`, 2 builds) links bitcode inputs, so ld writes an N_OSO with the `n_strx == 0` "no name"
sentinel (docs/04 §3).

Cross-check goldens against reference tools (inside the Docker image):

```bash
docker run --rm -v "$PWD":/work -w /work libstratum-fixtures python3 fixtures/crosscheck/crosscheck.py
```

```bash
python3 fixtures/build/build.py --list
```

```bash
python3 fixtures/build/build.py --toolchain apple-clang
```

## Local macOS note

Pin the Xcode SDK explicitly; the Command Line Tools SDK may be newer than the active linker
(docs/04-formats-and-toolchains.md §6):

```bash
clang++ -isysroot /Applications/Xcode.app/Contents/Developer/Platforms/MacOSX.platform/Developer/SDKs/MacOSX.sdk -g -O2 -c x.cpp -o x.o
```
