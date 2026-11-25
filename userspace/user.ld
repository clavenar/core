ENTRY(_start)

PHDRS
{
    image PT_LOAD FLAGS(5);
}

SECTIONS
{
    . = 0x00400000;

    .text : {
        *(.rodata .rodata.*)
        . = ALIGN(4);
        KEEP(*(.text.entry))
        *(.text .text.*)
    } :image

    ASSERT(SIZEOF(.text) <= 0x1000, "user program must fit in one page")
}
