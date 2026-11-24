    .section .text.trap, "ax", @progbits
    .balign 4
    .globl supervisor_trap_entry
    .type supervisor_trap_entry, @function
supervisor_trap_entry:
    # This handler ends execution, so it can reset the supervisor stack.
    lla sp, __stack_top
    csrr a0, scause
    csrr a1, sepc
    csrr a2, stval
    tail supervisor_trap
    .size supervisor_trap_entry, . - supervisor_trap_entry
