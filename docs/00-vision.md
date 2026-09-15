# 00 — Vision and scope

## Problem

A compiler and linker know exactly how source became machine code: which function landed in which section, what got inlined where, which lines vanished, how a struct is padded. Debuggers use this knowledge, but developers can only reach it through one-shot tools (`objdump`, `readelf`, `nm`, `addr2line`, `dwarfdump`, `pahole`) that each show one slice, in text meant for humans who already know the toolchain.

## Product statement

`libstratum` is an **embeddable library** that ingests a linked C/C++ binary and its debug information, builds a **correlation graph** between source locations, symbols/types, and binary locations, and answers structured queries about it in both directions. Every frontend (CLI, GUI app, custom IDE, VS Code, LSP, MCP for AI agents) is a thin client of that library.

## Core questions it must answer

1. **Layout:** How is type `T` laid out in memory: offsets, sizes, padding, alignment, cache-line boundaries? Would reordering the fields shrink it?
2. **Symbol → binary:** Where did function or variable `S` end up (section, address range, size)? Is it shared with other symbols (ICF)?
3. **Binary → source:** For address `A`, which source location produced it, through which inline chain?
4. **Source → binary:** What did `file:line` produce? This can be zero, one or many code ranges, and when the answer is zero it should say why.
5. **Summary:** How are the bytes spread across segments, sections, compile units and symbols? What changed between two builds?

## Principles

1. **Tell the truth about optimization.** Queries return lists. Zero, many, shared and unknown are first-class results, never silently collapsed.
2. **Show evidence.** Every answer records where it came from (DWARF, symbol table, linker map, inference), so users and agents can judge how much to trust it.
3. **Never invent attribution.** If data is missing (stripped binary, missing `.o`, mismatched debug file), say so with a diagnostic. Don't guess.
4. **Library before UI.** The core has no terminal, no editor concepts, no global state and no I/O policy.
5. **Formats are plugins.** The correlation and query logic never knows whether it is looking at ELF or Mach-O.
6. **Compiler-agnostic by construction.** Read standard artifacts (DWARF, symbol tables, maps). Never link against or shell out to a specific compiler for core functionality.

## Target users

- Systems, embedded and performance engineers optimizing size, layout and hot paths.
- CI pipelines that enforce size and layout budgets.
- AI coding agents that need facts about codegen instead of guesses (via a future MCP frontend).
- Tool builders embedding the engine in IDEs and GUIs.

## Goals for v1 (backend)

- Struct/class/union layout for C and C++ from DWARF, on Windows (PE/PDB), Linux and embedded (ELF/DWARF), and macOS (Mach-O/DWARF).
- Section and symbol summaries with ICF detection.
- Address → source with full inline chains.
- Source line → code ranges, with explicit "nothing found" outcomes.
- Diffing two builds (size and layout).
- A stable, versioned serializable model (`Request`/`Response`) ready for future frontends.
- A cache that makes repeated queries on an unchanged binary effectively free.

## Non-goals (for now)

- Rust, Swift, Objective-C or Zig semantics. We ignore those compile units gracefully.
- Disassembly-based analysis (e.g., attributing `.rodata` via instruction references as Bloaty does). This could be a later plugin.
- Runtime profiling, performance counters, cache simulation.
- Editing or rewriting binaries.
- Parsing C/C++ source code. We rely on the compiler's output, not our own frontend.
- Hardening against hostile inputs beyond "doesn't crash on malformed files". (`object` explicitly targets trusted inputs.)

## Phases

| Phase | Deliverable | Notes |
|---|---|---|
| Now | **Backend**: library, plugins for PE/PDB, ELF/DWARF, Mach-O/DWARF, embedded | Fixtures, goldens, agent evaluation, `examples/`. See [11](11-platforms.md), [ADR-0017](08-decisions.md#adr-0017-backend-only-scope) |
| Later | CLI, C ABI, JSON-RPC server, LSP, MCP, GUI app, custom IDE (e.g. game-engine observability), VS Code extension | Must need **no** core redesign; see [02-architecture.md §Future frontends](02-architecture.md#10-future-frontends) |

## Beyond v1

stratum is designed as a **suite**: one engine with many lenses over compiler and linker artifacts ([09-suite.md](09-suite.md)). Embedded firmware is the highest-value domain for it ([10-embedded.md](10-embedded.md)).

## What makes stratum different

See [01-prior-art.md](01-prior-art.md). In short:
- **Bloaty** attributes every byte to exactly one label, and is a C++ CLI.
- **pahole** shows layouts, but as a Linux CLI.
- **Visual Studio** shows layouts from its IntelliSense frontend, but only inside VS on Windows.

Nothing combines **bidirectional source↔binary correlation**, **honest multi-result provenance**, **layout**, and an **embeddable, format-pluggable library API**.
