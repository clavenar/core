    .section .text.enter_user, "ax", @progbits
    .balign 4
    .globl enter_user
    .type enter_user, @function
enter_user:
    csrw sie, zero
    li t0, 0x46720  # Clear SUM, FS, VS, SPP and SPIE.
    csrc sstatus, t0
    csrw sepc, a0
    fence.i
    # Start with clean integer registers and the user stack.
    .irp reg, ra, gp, tp, t0, t1, t2, t3, t4, t5, t6, s0, s1, s2, s3, s4, s5, s6, s7, s8, s9, s10, s11, a0, a1, a2, a3, a4, a5, a6, a7
    mv \reg, zero
    .endr
    lla sp, __user_stack_top
    lla t0, __stack_top
    csrw sscratch, t0
    mv t0, zero
    sret
    .size enter_user, . - enter_user
