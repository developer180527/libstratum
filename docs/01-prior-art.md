# 01 — Prior art

We researched existing tools to avoid rebuilding what exists, borrow proven ideas, and be precise about where stratum differs.

## Summary matrix

| Tool | Focus | Formats | Source↔binary | Struct layout | Embeddable API | Language | Status |
|---|---|---|---|---|---|---|---|
| **Bloaty** (Google) | Size attribution | ELF, Mach-O; PE and Wasm experimental | Compile units, inlines (size only) | No | No (CLI) | C++ | Active (commits Sept 2026) |
| **pahole** (dwarves) | Struct layout, BTF encoding | ELF (DWARF, BTF, CTF) | No | Yes: holes, cacheline, reorganize | libdwarves (C, Linux) | C | Active |
| **VS Memory Layout** | Layout in editor | n/a (compiler frontend) | No | Yes: offsets, padding, vtables | No (VS only) | — | Shipped VS 2022 17.9 |
| **StructLayout** (VS ext.) | Layout in editor | n/a (Clang/PDB) | No | Yes | No | C# / C++ | Community |
| **SuperSize** (Chromium) | Size tracking and diffs | Chrome Android builds | Symbols → source paths | No | No | Python | Chromium-only |
| **SizeBench** (Microsoft) | Size investigation | PE + PDB | Yes (Windows) | Yes (type layouts) | No | C# | not checked |
| **puncover** | Firmware size and stack | ELF | Per file/function | No | No (web UI) | Python | Active (CI, PyPI) |
| **twiggy** | Size, dominator trees | Wasm | No | No | Rust lib | Rust | not checked |
| **addr2line** (gimli-rs) | Address → source, inline frames | ELF, Mach-O (dSYM, unpacked), split DWARF | Address → source only | No | Rust lib | Rust | Active |
| **symbolic** (Sentry) | Symbolication | ELF, Mach-O, PE, PDB, Breakpad, Wasm | Address → source | No | Rust lib + C ABI | Rust | Active |
| **Compiler Explorer** | Source ↔ asm (single file) | Assembly output, not binaries | Line colouring, single translation unit | No | No | TS | Active |

## Bloaty McBloatface (the closest relative)

**What it is.** A size profiler that attributes every byte of a binary to a label (segment, section, symbol, compile unit, inline). It uses custom ELF, Mach-O and DWARF parsers. It has a diff mode, CSV/TSV output, and custom regex data sources. 📚

**How it works internally** (from `doc/how-bloaty-works.md`) 📚
- **RangeMap:** a sparse map of address ranges to labels. On overlap, **the first label wins**, so Bloaty scans the most specific data sources first.
- **Two address spaces:** a *VM* map (what the loaded program occupies) and a *file* map (bytes on disk), bundled as a `DualMap`. `.bss` exists only in VM space; debug sections exist only in the file.
- **Base map:** built from segments or sections. It defines "the whole binary", translates between VM addresses and file offsets, and guarantees 100% coverage using `[Unmapped]` labels.
- **Data sources are layered** over the base map, each refining the one below.
- **Deep symbol attribution:** a symbol is charged for its code plus `.eh_frame`, symtab entry, strtab name, and relocations. Anonymous `.rodata` is attributed via disassembly (Capstone).
- **Explicit limitation:** "no concept of sharing the cost". If two functions share data, the first one scanned gets all of it.

**Other documented behavior** 📚
- Verifies that binary and debug file match via build ID. Mismatches are called "a very easy mistake to make". Clang on ELF needs `-Wl,--build-id`.
- Doesn't support GNU debuglink or GDB-style build-ID directory lookup.
- Finds `.dSYM` bundles on Mach-O.
- `shortsymbols` demangling drops template parameters so instantiations group together.

**What we borrow**
1. **Separate VM and file sizes.** Every size in stratum is reported as `{vm, file}`.
2. **Exhaustive coverage.** Section and symbol summaries always add up to the real total, with explicit `unattributed` buckets.
3. **Refuse mismatched debug info** (build ID or `LC_UUID`).
4. **Diffs as a first-class feature.**
5. **Grouping instantiations** by template name with parameters removed.

**Where stratum deliberately differs**
| Bloaty | stratum |
|---|---|
| One label per byte; first one wins | Many-to-many provenance; shared cost is explicit (ICF, inlining) |
| Size-centric | Correlation-centric: size is one query among several |
| Source → binary is not a query | Bidirectional queries (line → ranges, address → inline chain) |
| No type layout | Layout is the first feature |
| C++ CLI, no stable library API, bundles protobuf/re2/capstone | Rust library first, format plugins, CLI as a client |

**Should we reuse Bloaty instead?** No. It has no library API, it's written in C++, and its one-label-per-byte model can't express our provenance model. Its design ideas are what we reuse.

## pahole (dwarves)

- Shows struct layouts from DWARF, BTF and CTF, including alignment holes and padding. 📚
- `--cacheline_size` controls cache-line markers.
- A `--reorganize` mode moves members and combines bitfields to remove holes. 📚
- Architecture: pluggable *loaders* that turn format-specific data (DWARF DIEs, BTF kinds) into common internal structures. This is the same shape as our format/debug-info layering, and it has worked for 15+ years.

**What we borrow:** the output vocabulary (holes, padding, cacheline boundaries, "sum members vs size") and reorganize-style suggestions. Its reorder algorithm is a good reference for ours, including bitfield demotion.
**Difference:** pahole is Linux/ELF only and a CLI. It doesn't do source↔binary correlation.

## Visual Studio Memory Layout and StructLayout

- VS 2022 17.8 shows size and alignment in hover tooltips. 17.9 added an interactive Memory Layout view with offsets, sizes, padding and vtable pointers. 📚
- It uses the IntelliSense compiler frontend, not debug info. So it shows what the *frontend thinks*, for the active configuration, without needing a build.
- **StructLayout** (community extension) does the same using Clang or PDB.

**Lesson:** users want layout in a hover with zero friction. The frontend approach gives instant results without a build, while the debug-info approach reflects what the compiler actually produced. We chose DWARF (see [ADR-0002](08-decisions.md#adr-0002-struct-layout-comes-from-dwarf-not-libclang)). A later, optional "frontend" layout provider can plug in behind the same query without breaking anything.

## Chromium SuperSize and Microsoft SizeBench

- **SuperSize** archives a `.size` file per build and powers a size-breakdown bot on every Chromium code review. It relies on Chrome's build system (`.ninja` files). 📚
- **SizeBench** investigates PE+PDB size, including type layouts, template bloat and duplicate data. 📚

**Lesson:** size regression gating in CI is the killer workflow. A persisted, versioned snapshot format is what makes diffs cheap. This shapes our cache/snapshot design (see [02-architecture.md §Cache](02-architecture.md#8-cache-and-snapshots)).

## Rust ecosystem building blocks

- **`gimli`:** zero-copy, lazy DWARF reader/writer. Format-agnostic by design; loading sections is left to the caller. Apache-2.0/MIT. 📚
- **`object`:** unified read API over ELF, Mach-O, PE/COFF, XCOFF, Wasm and archives, plus low-level format-specific APIs. Handles compressed sections. Targets *trusted* input, with little effort spent on denial-of-service hardening. 📚
- **`addr2line`:** built on both. Resolves an address to inline frames. Its `Loader` already handles dSYMs, unpacked Mach-O debug maps and split DWARF. 📚 **We should reuse its logic or depend on it** for address → inline chains instead of rewriting the tricky parts.
- **`symbolic`** (Sentry): a production multi-format debug-info abstraction (ELF, Mach-O, PE, PDB, Breakpad, Wasm) with a C ABI crate. Proves that "one abstraction over many formats, plus a C ABI" works in Rust at scale. 📚 It's a good reference for trait design and the C ABI.
- **`cpp_demangle`:** Itanium C++ demangler (used by symbolic and others).

## Consequences for our design

1. Treat **debug info as a layer separate from the container format** (gimli, pahole and symbolic all do this).
2. **Two-space sizes** and **exhaustive summaries** (Bloaty).
3. **Identity checks** between binary and debug info are mandatory (Bloaty, LLDB).
4. **Explicit shared-cost modeling** is our main technical differentiator. Bloaty explicitly lacks it.
5. **Snapshots and diffs** are core to the CI use case (SuperSize, Bloaty).
6. Reuse `addr2line` where possible. Don't reinvent inline-frame resolution.

## Sources

- Bloaty: https://github.com/google/bloaty · https://github.com/google/bloaty/blob/main/doc/how-bloaty-works.md · https://github.com/google/bloaty/blob/main/doc/using.md
- pahole: https://github.com/acmel/dwarves/blob/master/man-pages/pahole.1 · https://www.mankier.com/1/pahole
- Visual Studio: https://devblogs.microsoft.com/visualstudio/size-alignment-and-memory-layout-insights-for-c-classes-structs-and-unions/
- StructLayout: https://github.com/Viladoman/StructLayout
- SuperSize: https://chromium.googlesource.com/chromium/src/+/refs/heads/main/tools/binary_size/libsupersize/README.md
- SizeBench: https://github.com/microsoft/SizeBench
- puncover: https://github.com/HBehrens/puncover
- Size tool survey: https://interrupt.memfault.com/blog/best-firmware-size-tools
- gimli: https://github.com/gimli-rs/gimli · object: https://github.com/gimli-rs/object · addr2line: https://github.com/gimli-rs/addr2line
- symbolic: https://github.com/getsentry/symbolic
