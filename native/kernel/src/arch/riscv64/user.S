    .section .text.enter_user, "ax", @progbits
    .balign 4
    .globl enter_user
    .type enter_user, @function
enter_user:
    csrw sie, zero
    li t0, 0x46720  # Clear SUM, FS, VS, SPP and SPIE.
    csrc sstatus, t0
    lla t0, user_entry
    csrw sepc, t0
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

    .section .user_text, "ax", @progbits
    .balign 4
    .type user_entry, @function
user_entry:
    addi sp, sp, -16
    li s0, 0x123
    li t1, 0x456
    sd s0, 0(sp)
    li a7, {probe_call}
    ecall
    li t0, {probe_value}
    bne a0, t0, .Luser_failed
    li t0, 0x456
    bne t1, t0, .Luser_failed
    li t0, 0x123
    bne s0, t0, .Luser_failed
    ld t1, 0(sp)
    bne t1, t0, .Luser_failed
    li a0, 0
    j .Luser_exit
.Luser_failed:
    li a0, 1
.Luser_exit:
    li a7, {exit_call}
    ecall
1:
    j 1b
    .size user_entry, . - user_entry
