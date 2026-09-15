# libstratum documentation

`stratum` answers one question from many angles: **"what did this piece of C/C++ source become in the binary?"**
This folder is the design record. It is written *before* code on purpose: the decisions in here are expensive to reverse.

## Reading order

| # | Document | What it settles |
|---|---|---|
| 00 | [Vision and scope](00-vision.md) | Why the project exists, goals, non-goals, phases |
| 01 | [Prior art](01-prior-art.md) | Bloaty, pahole, Visual Studio Memory Layout, SuperSize, symbolic… what we borrow, where we differ |
| 02 | [Architecture](02-architecture.md) | Layers, crates, dependency rules, extension points, public API, embedding rules |
| 03 | [Data model](03-data-model.md) | Entities, provenance, outcomes, evidence, schema versioning |
| 04 | [Formats and toolchains](04-formats-and-toolchains.md) | ELF, Mach-O, debug-info location, linker maps, Clang vs GCC, verified experiments |
| 05 | [Correlation algorithms](05-correlation.md) | Struct layout, sections/symbols, ICF, instantiation, inlining, elimination, and their honest limits |
| 06 | [Testing strategy](06-testing.md) | Fixture corpus, golden files, toolchain matrix |
| 07 | [Roadmap](07-roadmap.md) | Phase 1 library milestones, Phase 2 CLI, later frontends |
| 08 | [Decision log](08-decisions.md) | Architecture decision records (ADRs) and open questions |
| 09 | [The stratum suite](09-suite.md) | One engine, many lenses; artifact providers; capabilities; agent evaluation |
| 10 | [Embedded systems](10-embedded.md) | Flash/RAM regions, Thumb, fault decoding, stack, SVD |
| 11 | [Platforms](11-platforms.md) | Platform matrix, Windows (PE/PDB/MSVC) in depth, debug-info-neutral IR, updated crate map |

## Fixed constraints (agreed)

- **Sequence:** library (= the backend) → validate on real code (the game engine, firmware) → CLI for humans and LLMs → other frontends later ([ADR-0017](08-decisions.md#adr-0017-backend-only-scope), [ADR-0020](08-decisions.md#adr-0020-delivery-sequence-library--real-code-validation--cli)).
- **Platforms (Tier 1):** Windows (PE + PDB; MSVC, clang-cl), Linux (ELF + DWARF; Clang, GCC), macOS (Mach-O + DWARF; Apple Clang), embedded (ELF + DWARF; arm-none-eabi-gcc, LLVM; Cortex-M, RV32) ([ADR-0019](08-decisions.md#adr-0019-v1-platform-matrix), [11](11-platforms.md)).
- **Extensible by design:** container formats, debug-info formats (DWARF, PDB), map parsers, demanglers, languages and artifact providers are all plugins ([ADR-0018](08-decisions.md#adr-0018-debug-info-neutral-ir-dwarf-and-pdb-backends)).
- **Languages:** C and C++ only for now. Rust later.
- **Suite:** one engine, many lenses over shared join keys ([09](09-suite.md)).
- **Implementation:** Rust; `object` (containers), `gimli` (DWARF), `ms-pdb`/`pdb2` (PDB, pending Q14).

## Status

Design phase. No code yet. **Where docs 02–07 conflict with 11 or later ADRs, the later ADRs win** (e.g. DWARF-centric wording in 02/05 now means "debug info via `DebugIr`"). Claims marked **✅ verified** were reproduced locally (Apple clang 21, ld-1267, arm64, macOS 27) on 2026-09-15; claims marked **📚 sourced** come from cited documentation and must be re-verified by the fixture corpus in milestone M0.
