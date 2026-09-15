# libstratum

**A low-level observability library for native code:** it answers precise questions about what C and C++
source became in the binary: memory layouts and padding, where symbols landed, which source line produced
which machine code through which inline chain, and how flash, RAM and sections are spent. It gets these
answers from the compiler's and linker's own output instead of guesses.

- **Platforms (Tier 1):** Windows (PE + PDB; MSVC, clang-cl) · Linux (ELF + DWARF; Clang, GCC) ·
  macOS (Mach-O + DWARF) · embedded (ELF + DWARF; Cortex-M, RV32)
- **Extensible:** container formats and debug-info formats are plugins that translate into one neutral model
- **Honest:** answers carry evidence and diagnostics; ambiguity (inlining, folding, elimination) is explicit

> Status: **M2 complete; M3 next.** ELF, Mach-O and PE binaries open with identity, sections and symbols;
> struct layouts come from DWARF and PDB on every Tier 1 platform, cross-checked against `llvm-dwarfdump`,
> `llvm-pdbutil` and `pahole`. Next: symbols, sizes and memory regions. See
> [docs/07-roadmap.md](docs/07-roadmap.md).

## Workspace

| Crate | Role |
|---|---|
| `libstratum` | Facade: default plugin set behind features, convenience I/O |
| `libstratum-model` | Public serializable types (`SCHEMA_VERSION`) |
| `libstratum-core` | Plugin SPI, neutral IR, engine and session; no parser dependencies |
| `libstratum-format-{elf,macho,pe}` | Container plugins |
| `libstratum-debug-{dwarf,pdb}` | Debug-info backends |
| `libstratum-demangle` | Itanium and MSVC name demangling |
| `libstratum-lang-cpp` | C/C++ semantics |
| `libstratum-arch` | Architecture facts (pointer size, Thumb bit, mapping symbols) |

Delivery order: library → validation on real code (a game engine, firmware) → CLI (`stratum`) → other frontends
([ADR-0020](docs/08-decisions.md#adr-0020-delivery-sequence-library--real-code-validation--cli)).

## Development

```bash
cargo test --workspace
```

```bash
cargo clippy --workspace --all-targets -- -D warnings
```

```bash
python3 scripts/check_dependency_rules.py
```

```bash
cargo run -p libstratum --example info -- path/to/binary
```

```bash
cargo run -p libstratum --example layout -- path/to/binary TypeName
```

Design documents live in [docs/](docs/README.md). Architecture decisions are recorded as ADRs in
[docs/08-decisions.md](docs/08-decisions.md); change a decision by adding a superseding ADR.

## License

MIT
