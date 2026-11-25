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

    /* Boot user programs use panic=abort. */
    /DISCARD/ : {
        *(.eh_frame .eh_frame.*)
    }

    ASSERT(SIZEOF(.text) <= 0x2000, "user program must fit in two pages")
}
