ENTRY(_start)

PHDRS
{
    text PT_LOAD FLAGS(5);
    data PT_LOAD FLAGS(6);
}

SECTIONS
{
    /* OpenSBI starts execution at the lowest address loaded by QEMU. */
    . = 0x80200000;

    .text : {
        KEEP(*(.text.entry))
        *(.text .text.*)
    } :text

    .rodata : {
        *(.rodata .rodata.* .srodata .srodata.*)
    } :text

    . = ALIGN(0x1000);

    .data : {
        *(.data .data.* .sdata .sdata.* .got .got.*)
    } :data

    .bss (NOLOAD) : {
        *(.bss .bss.* .sbss .sbss.* COMMON)
    } :data

    .stack ALIGN(16) (NOLOAD) : {
        . += 16K;
        __stack_top = .;
    } :data

    .frame_pool ALIGN(0x1000) (NOLOAD) : {
        KEEP(*(.frame_pool))
    } :data
}
