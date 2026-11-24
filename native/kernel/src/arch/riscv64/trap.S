    .section .text.trap, "ax", @progbits
    .balign 4
    .globl supervisor_trap_entry
    .type supervisor_trap_entry, @function
supervisor_trap_entry:
    csrr a0, scause
    csrr a1, sepc
    csrr a2, stval
    tail supervisor_trap_fatal
    .size supervisor_trap_entry, . - supervisor_trap_entry
