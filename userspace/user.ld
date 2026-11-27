ENTRY(_start)

MEMORY
{
    /* The kernel build passes the boot ABI limit to the linker. */
    user_image : ORIGIN = 0x00400000, LENGTH = USER_IMAGE_MAX_BYTES
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
