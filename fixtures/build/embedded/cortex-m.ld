/* Generic Cortex-M memory map for fixtures (STM32F4-like): 1M flash, 192K RAM. */
ENTRY(reset_handler)

MEMORY
{
    FLASH (rx)  : ORIGIN = 0x08000000, LENGTH = 1M
    RAM   (rwx) : ORIGIN = 0x20000000, LENGTH = 192K
}

__stack_top = ORIGIN(RAM) + LENGTH(RAM);

SECTIONS
{
    .isr_vector : { KEEP(*(.isr_vector)) } > FLASH
    .text       : { *(.text .text.*) } > FLASH
    .rodata     : { *(.rodata .rodata.*) } > FLASH
    .ARM.exidx  : { *(.ARM.exidx*) } > FLASH
    .data       : { *(.data .data.*) } > RAM AT > FLASH   /* LMA in flash, VMA in RAM */
    .bss (NOLOAD) : { *(.bss .bss.* COMMON) } > RAM
    .noinit (NOLOAD) : { *(.noinit .noinit.*) } > RAM
}
