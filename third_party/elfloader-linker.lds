/*
 * Copyright 2020, Data61, CSIRO (ABN 41 687 119 230)
 *
 * SPDX-License-Identifier: GPL-2.0-only
 */

#include <autoconf.h>
#include <elfloader/gen_config.h>
#include "image_start_addr.h"

ENTRY( _start )

/* EnerOS：显式 PHDRS 分离 RX 与 RW 段——elfloader 必须满足 W^X
 * （xtask pack_elf64 拒绝 RWX/RW 同段 LOAD）。ld 默认把无 PHDRS 的
 * 输出合并为单个 RWE LOAD 段，无法通过镜像校验。 */
PHDRS
{
    text PT_LOAD FLAGS(5); /* R+X */
    data PT_LOAD FLAGS(6); /* R+W */
}

SECTIONS
{
    . = IMAGE_START_ADDR;
    _text = .;
    .start :
    {
        *(.text.start)
    } :text
    .text :
    {
        *(.text)
    } :text
    . = ALIGN(16);
    .rodata :
    {
        *(.srodata*)
        . = ALIGN(16);
        *(.rodata)
        *(.rodata.*)
        /*
         * ld crashes when we add this here: *(_driver_list)
         */
        . = ALIGN(16);
        _archive_start = .;
        *(._archive_cpio)
        _archive_end = .;
    } :text
    . = ALIGN(16);
    .data :
    {
        __global_pointer$ = . + 0x800;
        *(.sdata*)
        *(.data)
        *(.data.*)
    } :data
    . = ALIGN(16);
    .bss (NOLOAD) :
    {
        . = ALIGN(0x1000);
        core_stack_alloc = .;
        . = . + CONFIG_MAX_NUM_NODES * (1 << 12);
        core_stack_alloc_end = .;
        _bss = .;
        *(.sbss*)
        *(.bss)
        *(.bss.*)
        _bss_end = .;
    } :data
     _end = .;

    /* With lld on riscv32 a debug-only non-empty .eh_frame ends up ahead of
      .start, and OpenSBI always jumps to the first address. We could place it
      somewhere explicitly, but discarding it for elfloader is easier and safe
      (application payload is unaffected). */
    /DISCARD/ :
    {
        *(.eh_frame)
    }
}
