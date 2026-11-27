ENTRY(_start)

PHDRS
{
    text PT_LOAD FLAGS(5);
    rodata PT_LOAD FLAGS(4);
    data PT_LOAD FLAGS(6);
}

SECTIONS
{
    /* OpenSBI starts execution at the lowest address loaded by QEMU. */
    . = 0x80200000;
    __text_start = .;

    .text : {
        KEEP(*(.text.entry))
        *(.text .text.*)
    } :text

    . = ALIGN(0x1000);
    __rodata_start = .;

    .rodata : {
        *(.rodata .rodata.* .srodata .srodata.* .data.rel.ro .data.rel.ro.*)
    } :rodata

    . = ALIGN(0x1000);
    __writable_start = .;

    .data : {
        *(.data .data.* .sdata .sdata.* .got .got.*)
    } :data

    .bss (NOLOAD) : {
        *(.bss .bss.* .sbss .sbss.* COMMON)
    } :data

    .stack ALIGN(16) (NOLOAD) : {
        /* Unoptimized boot code builds the registered task array on this stack. */
        . += 64K;
        __stack_top = .;
    } :data

    .frame_pool ALIGN(0x1000) (NOLOAD) : {
        KEEP(*(.frame_pool))
    } :data

    __kernel_end = .;
    ASSERT((__text_start & 0x1fffff) == 0 && __kernel_end - __text_start <= 0x200000,
           "boot kernel must fit in one aligned 2 MiB region")
}
