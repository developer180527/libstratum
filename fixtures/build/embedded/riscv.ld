/* Generic RV32 microcontroller memory map for fixtures. */
ENTRY(reset_handler)

MEMORY
{
    FLASH (rx)  : ORIGIN = 0x20000000, LENGTH = 512K
    RAM   (rwx) : ORIGIN = 0x80000000, LENGTH = 64K
}

__stack_top = ORIGIN(RAM) + LENGTH(RAM);

SECTIONS
{
    .isr_vector : { KEEP(*(.isr_vector)) } > FLASH
    .text       : { *(.text .text.*) } > FLASH
    .rodata     : { *(.rodata .rodata.* .srodata .srodata.*) } > FLASH
    .data       : { *(.data .data.* .sdata .sdata.*) } > RAM AT > FLASH
    .bss (NOLOAD) : { *(.bss .bss.* .sbss .sbss.* COMMON) } > RAM
}
