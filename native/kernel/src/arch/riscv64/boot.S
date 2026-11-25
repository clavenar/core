    .section .text.entry, "ax", @progbits
    .balign 4
    .globl _start
    .type _start, @function
_start:
    csrci sstatus, 2  # Clear SIE.
    csrw sie, zero
    csrw sscratch, zero
    lla sp, __stack_top
    lla t0, supervisor_trap_entry
    csrw stvec, t0
    tail kernel_main
    .size _start, . - _start
