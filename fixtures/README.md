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

Status: directory layout only. Build scripts and the first `layout/` fixtures are milestone M0.

## Local macOS note

Pin the Xcode SDK explicitly; the Command Line Tools SDK may be newer than the active linker
(docs/04-formats-and-toolchains.md §6):

```bash
clang++ -isysroot /Applications/Xcode.app/Contents/Developer/Platforms/MacOSX.platform/Developer/SDKs/MacOSX.sdk -g -O2 -c x.cpp -o x.o
```
