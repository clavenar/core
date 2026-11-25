    .section .text.trap, "ax", @progbits
    .balign 4
    .globl supervisor_trap_entry
    .type supervisor_trap_entry, @function
supervisor_trap_entry:
    # sscratch holds the supervisor stack top in U-mode and zero in S-mode.
    csrrw sp, sscratch, sp
    beqz sp, .Lsupervisor_trap

    addi sp, sp, -{frame_size}
    sd zero, 0(sp)
    .irp reg, 1, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31
    sd x\reg, (8 * \reg)(sp)
    .endr

    csrrw t0, sscratch, zero
    sd t0, 16(sp)  # Saved user sp (x2).
    csrr t0, sepc
    sd t0, {sepc_offset}(sp)
    csrr t0, sstatus
    sd t0, {sstatus_offset}(sp)

    mv a0, sp
    csrr a1, scause
    csrr a2, stval
    call supervisor_user_trap

.Lrestore_user:
    ld t0, {sepc_offset}(sp)
    csrw sepc, t0
    ld t0, {sstatus_offset}(sp)
    csrw sstatus, t0
    lla t0, __stack_top
    csrw sscratch, t0
    .irp reg, 1, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31
    ld x\reg, (8 * \reg)(sp)
    .endr
    ld sp, 16(sp)
    sret

.Lsupervisor_trap:
    csrw sscratch, zero
    li t0, {sum_mask}
    csrc sstatus, t0
    lla sp, __stack_top
    csrr a0, scause
    csrr a1, sepc
    csrr a2, stval
    tail supervisor_trap_fatal
    .size supervisor_trap_entry, . - supervisor_trap_entry

    .balign 4
    .globl enter_user
    .type enter_user, @function
enter_user:
    fence.i
    mv sp, a0
    j .Lrestore_user
    .size enter_user, . - enter_user
