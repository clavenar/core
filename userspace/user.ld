ENTRY(_start)

MEMORY
{
    user_image : ORIGIN = 0x00400000, LENGTH = 28K
}

PHDRS
{
    image PT_LOAD FLAGS(5);
}

SECTIONS
{
    .text : {
        *(.rodata .rodata.*)
        . = ALIGN(4);
        KEEP(*(.text.entry))
        *(.text .text.*)
    } >user_image :image

    /* Boot user programs use panic=abort. */
    /DISCARD/ : {
        *(.eh_frame .eh_frame.*)
    }
}
