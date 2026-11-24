    .section .text.entry, "ax", @progbits
    .balign 4
    .globl _start
    .type _start, @function
_start:
    lla sp, __stack_top
    tail kernel_main
    .size _start, . - _start
