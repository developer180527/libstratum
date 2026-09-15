# 10 — Embedded systems

Embedded firmware is where the gap between "what I wrote" and "what runs" is most expensive. Flash and RAM are measured in kilobytes, stacks are fixed and small, a wrong struct offset on a memory-mapped register silently corrupts hardware, and failures show up as a HardFault with nothing but a program counter. It's also where LLM guesses do the most damage.

> **Status:** Accepted: embedded is Tier 1 ([ADR-0019](08-decisions.md#adr-0019-v1-platform-matrix)). §7 records the constraints this replaced (x86_64 + aarch64 only; Clang first). See [§7](#7-impact-on-accepted-decisions) and [ADR-0015](08-decisions.md#adr-0015-embedded-targets-in-v1-scope)/[ADR-0016](08-decisions.md#adr-0016-three-address-spaces-and-memory-regions).

## 1. How stratum fits the embedded workflow

stratum runs **on the host**, never on the device. Firmware toolchains already produce everything it needs:

```
 source ─► arm-none-eabi-gcc / clang / armclang / IAR ─► firmware.elf (+ DWARF) ─► objcopy ─► firmware.bin/.hex ─► flash
                                                           │ firmware.map
                                                           │ *.su / .stack_sizes
                                                           │ device.svd
                                                           ▼
                                                        stratum  ◄── HardFault PC/LR dump, SWO/ETM trace, CI budgets, agent
```

The `.elf` file is the key: even though the device only receives a raw `.bin`, the ELF with DWARF is always produced along the way. Embedded binaries are almost always **ELF**, so the ELF plugin covers nearly all of them; only the architecture list and some address semantics grow.

## 2. What embedded changes technically

### 2.1 Architectures (32-bit)
| Family | Arch | Notes |
|---|---|---|
| Arm Cortex-M (M0/M0+/M3/M4/M7/M33/M55) | ARMv6-M / v7-M / v8-M, **Thumb only** | The dominant MCU family |
| RISC-V MCUs | rv32imac / rv32imc | Growing fast (ESP32-C3/C6, GD32V, CH32V) |
| Xtensa | ESP32, ESP32-S3 | GCC only |
| AVR, MSP430 | 8/16-bit | Out of scope unless demanded |

The IR already carries `address_size` and `endian` (see [04 §4](04-formats-and-toolchains.md#4-architectures-x86_64-aarch64)), so 32-bit is a supported shape. Arch-specific facts to handle:
- **Thumb bit.** On Arm ELF, function symbol values have **bit 0 set** for Thumb code (`0x08000401` means code at `0x08000400`). DWARF addresses don't set it. The ELF plugin must normalize symbol addresses and keep `is_thumb` as an attribute, or symbol ↔ DWARF joins fail by one byte.
- **Mapping symbols** `$t`, `$a`, `$d` mark Thumb/Arm/data regions inside sections. They must be filtered out of symbol lists, but they're useful to the disassembler plugin (literal pools are data, not code).
- **Veneers and literal pools:** linker-generated long-branch veneers and inline constant pools appear in `.text`. Classify them as `Unattributed { LinkerSynthesized }` and data-in-code, respectively.

### 2.2 Three address spaces: VM, file, **and load**
On a microcontroller, `.data` is **stored in flash** (its load memory address, LMA) and **copied to RAM at boot** (its virtual memory address, VMA). It costs flash *and* RAM. `.bss` costs only RAM. `.text` and `.rodata` cost only flash (unless code is relocated to RAM for speed).

ELF records this without a map file: `PT_LOAD` segments carry `p_vaddr` (run address) and `p_paddr` (load address). So:
- `Size { vm, file }` becomes **`Size { vm, file, load }`**, or more usefully the embedded view **`Footprint { flash, ram }`**, derived per section from which *memory region* its LMA and VMA fall into.
- A section can live in two regions: `.data` has LMA in FLASH and VMA in RAM.

### 2.3 Memory regions
Linker scripts define regions (`MEMORY { FLASH (rx) : ORIGIN = 0x08000000, LENGTH = 1M; RAM (rwx) : ORIGIN = 0x20000000, LENGTH = 192K }`). Sources, in order of preference:
1. GNU ld map file `Memory Configuration` table (name, origin, length, attributes) 📚
2. The linker script itself (parse only the `MEMORY` block)
3. User-provided regions (CLI flags, config, or from devicetree/SVD)
4. Fallback: infer regions by clustering segment addresses (`Evidence::Heuristic`)

The result is `MemoryRegion { name, origin, length, attrs }` in the IR, plus a new summary dimension (`region > section > symbol`) and **utilization** (`used / length`, remaining bytes).

### 2.4 Toolchains
| Toolchain | Compiler base | Output | Notes |
|---|---|---|---|
| **GNU Arm Embedded (`arm-none-eabi-gcc`)** | GCC | ELF + DWARF | Widely used as the default in vendor SDKs and IDEs (e.g. STM32CubeIDE; ESP-IDF uses GCC) ❓ market share not verified |
| **Arm Toolchain for Embedded (ATfE)** / LLVM Embedded Toolchain for Arm | LLVM/Clang | ELF + DWARF | Official Arm LLVM toolchain; supported by Zephyr 📚 |
| **Arm Compiler for Embedded 6 (armclang + armlink)** | LLVM/Clang (armlink is proprietary) | ELF + DWARF (DWARF 5 with `-gdwarf-5`) | Keil MDK 📚; armlink map format differs from GNU ld |
| **IAR Embedded Workbench (ILINK)** | Proprietary | ELF + DWARF only 📚 | Proprietary DWARF producer: expect quirks |
| Zephyr SDK | GCC | ELF + DWARF | Zephyr's `size_report` already uses DWARF via pyelftools 📚 |

**Consequence:** in embedded, **GCC is at least as important as Clang**, possibly more. See §7.

### 2.5 Bare metal
- No dynamic linking, PLT/GOT (usually), or `.dynsym`. Simpler.
- Often `-ffunction-sections -fdata-sections -Wl,--gc-sections` → linker-discard evidence (tombstones) matters a lot.
- `-Os`/`-Oz` and LTO are common → the provenance model (inlining, folding, "no code") is exercised heavily.
- Custom sections are common: `.isr_vector`, `.noinit`, `.ramfunc`, `.ccmram`, `.persistent`, `.fastrun`. Section kinds must fall back gracefully to `Other` with raw names and flags, and regions give them meaning.

## 3. Embedded lenses

These reuse the engine from [09-suite.md](09-suite.md); only the providers and views are embedded-specific.

| Lens | Question | Data | Value |
|---|---|---|---|
| **Memory budget** | How much FLASH and RAM does each component, file and symbol use? How close are we to the limit? Which region overflows next? | ELF segments (LMA/VMA), map `Memory Configuration`, symbols, DWARF CUs | CI gate: fail if FLASH > 95% or a module grows > N bytes. Replaces ad-hoc `size`/Zephyr `rom_report` scripts |
| **Fault decoding** | HardFault at `PC=0x0800_1A3C, LR=0x0800_0F11`: where is that, through which inline chain, in which ISR? | `by_address` + Thumb normalization + inline chains | Turns a crash log into source lines, the most common embedded debugging task. Commercial services (e.g. Memfault) do this; a local, scriptable library doesn't exist in this form |
| **Worst-case stack** | What is the deepest call chain from `main` and from each ISR? Which frames are big? | `.su` / `.stack_sizes` / `--callgraph-info` + call graph from disassembly + user annotations for function pointers, recursion and interrupt nesting | Stack overflows are silent corruption on MCUs. Static tools exist but are toolchain-specific; indirect calls **must** be reported as unknown, not ignored 📚 |
| **Register layout vs hardware** | Does `USART_TypeDef` in my HAL match the SVD: offsets, sizes, reserved gaps? | DWARF layout + CMSIS-SVD provider | Catches wrong register offsets and padding in hand-written peripheral structs, a class of bug that is otherwise found with a logic analyzer |
| **Packed/protocol struct audit** | Is this over-the-wire struct packed, the expected size, the right endianness? Is it read with unaligned access (a fault on Cortex-M0)? | DWARF layout + `DW_AT_alignment` + codegen lens | Protocol and bootloader bugs |
| **ISR and hot-path audit** | How big is each ISR? What did it inline? Does it call anything placed in flash while flash is being written (for RAM-function requirements)? | Vector table parse + symbols + correlation + regions | Timing- and safety-critical code |
| **Optimization report** | Why wasn't this inlined at `-Os`? Which templates or `static inline`s duplicate across TUs? | Opt remarks (Clang-based toolchains) + instantiation groups | Size tuning with facts |

## 4. Agents in embedded work

Embedded questions are exactly where an LLM answering from memory fails dangerously:
- "`sizeof(struct can_frame)` on Cortex-M4": depends on packing, the `enum` size (varies with `-fshort-enums` and toolchain ABI defaults ❓), and alignment.
- "Will this fit in flash?": unknowable without the real build.
- "What caused this HardFault?": requires the ELF, not intuition.
- "Is this register struct correct?": requires the SVD plus the real layout.

With the MCP frontend, an agent can:
1. Call `capabilities()` → learn that `.su` files are missing → ask the user to add `-fstack-usage`.
2. Make a change → rebuild → `memory_budget()` before and after → report real numbers.
3. Decode a pasted fault dump with `by_address` instead of guessing.
4. Verify a peripheral struct against the SVD before claiming it's correct.

## 5. Suggested embedded fixtures

Added to [06-testing.md](06-testing.md) once accepted:
- Cortex-M4F (`thumbv7em`, hard float) and Cortex-M0+ (`thumbv6m`) with `arm-none-eabi-gcc` and ATfE clang
- rv32imac with GCC and clang
- Linker scripts with FLASH/RAM, `.data` LMA≠VMA, `.noinit`, `.ramfunc`
- `-Os`, `-Oz`, `-flto`, `--gc-sections`, `-fshort-enums`
- A minimal SVD plus a matching and a deliberately mismatched register struct
- A synthetic HardFault dump with PC/LR inside inlined code

## 6. Prior art in embedded

| Tool | Scope | Gap stratum fills |
|---|---|---|
| `arm-none-eabi-size`, `nm`, map files | Section totals, raw symbols | No regions or components, no source joins |
| Zephyr `size_report` (`rom_report`/`ram_report`) | DWARF-based ROM/RAM trees, Zephyr only 📚 | Tied to Zephyr; no layout, stack or faults |
| puncover | Code size, static RAM, stack usage, web UI | Python app, not a library; GCC-oriented |
| WorstCaseStack, StackAnalyzer (AbsInt) | Worst-case stack | Separate tools; commercial or GCC-only |
| Memfault | Fault decoding, fleet analytics | Commercial SaaS |
| Vendor IDEs (STM32CubeIDE build analyzer, Keil, IAR) | Memory usage views | Locked into one IDE and toolchain; not scriptable for agents or CI |

## 7. Impact on accepted decisions

| Accepted today | Embedded pressure | Proposal |
|---|---|---|
| Architectures: x86_64 + aarch64 | Nearly all MCUs are 32-bit Arm Thumb or RV32 | **ADR-0015 (Proposed):** add `arm` (Thumb, ARMv6-M/v7-M/v8-M) and `riscv32` to v1 fixtures. Cost is small (ELF plugin + Thumb normalization); the IR is already 32-bit-ready |
| Clang first, GCC later | `arm-none-eabi-gcc` is a very common firmware toolchain ❓ | Keep Clang first for hosted targets, but **bring GCC forward for ELF embedded fixtures** (M3–M4 instead of "later"). The architecture already anticipates GCC ([ADR-0005](08-decisions.md#adr-0005-clang-first-gcc-later-without-redesign)) |
| `Size { vm, file }` ([ADR-0010](08-decisions.md#adr-0010-every-size-is-reported-in-both-vm-and-file-space)) | `.data` costs flash and RAM; regions matter | **ADR-0016 (Proposed):** add `load` space and `MemoryRegion`s to the IR and model. Additive, and cheap to do **now**; a breaking model change later |
| Mach-O is a first-class format | Irrelevant for embedded | No change; the plugin boundary already isolates it |

**Recommendation:** accept ADR-0016 now regardless of timeline, because it's a data-model decision and changing the model later is the expensive kind of debt. Decide ADR-0015 (embedded in v1 or not) based on who the first users are.

## Sources

- Zephyr footprint tools: https://github.com/zephyrproject-rtos/zephyr/blob/main/scripts/footprint/size_report
- Arm Compiler for Embedded 6 (armclang is LLVM-based; DWARF 5): https://developer.arm.com/documentation/107811/6-19/?lang=en · https://developer.arm.com/documentation/dui1093/d/Getting-Started/Introduction-to-Arm-Compiler-6
- Arm Toolchain for Embedded (LLVM): https://github.com/arm/arm-toolchain · https://docs.zephyrproject.org/latest/develop/toolchains/arm_toolchain_for_embedded.html · https://learn.arm.com/install-guides/llvm-embedded/
- Clang for Cortex-M firmware: https://interrupt.memfault.com/blog/arm-cortexm-with-llvm-clang
- IAR ILINK emits ELF/DWARF only: https://docs.iar.com/ewarm/10.1x/en/iar-c-c---development/linking-your-application/linking-considerations/producing-output-formats-other-than-elf-dwarf.html
- Stack analysis on Cortex-M: https://interrupt.memfault.com/blog/measuring-stack-usage · https://blog.japaric.io/stack-analysis/ · https://www.keil.com/appnotes/files/apnt_316.pdf · https://github.com/PeterMcKinnis/WorstCaseStack/blob/master/README.md
- Map files in firmware: https://interrupt.memfault.com/blog/get-the-most-out-of-the-linker-map-file · https://www.embeddedrelated.com/showarticle/900.php
- Firmware size tools survey: https://interrupt.memfault.com/blog/best-firmware-size-tools
