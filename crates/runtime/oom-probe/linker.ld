/* EnerOS OOM probe linker script — QEMU virt 机器 RAM 基址 0x40000000。 */
ENTRY(_start)

SECTIONS {
    . = 0x40000000;
    .text : {
        *(.text.start)
        *(.text*)
    }
    .rodata : { *(.rodata*) }
    .data : { *(.data*) }
    .bss (NOLOAD) : {
        __bss_start = .;
        *(.bss*)
        __bss_end = .;
    }
}
