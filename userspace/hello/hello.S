    # Boot syscall numbers from the Clavenar user ABI.
    .equ SYS_PROBE, 0
    .equ SYS_EXIT, 1
    .equ SYS_WRITE_CONSOLE, 2
    .equ PROBE_VALUE, 42

    .section .text.entry, "ax", @progbits
    .balign 4
    .globl _start
    .type _start, @function
_start:
    addi sp, sp, -16
    li s0, 0x123
    li t1, 0x456
    sd s0, 0(sp)
    li a7, SYS_PROBE
    ecall
    li t0, PROBE_VALUE
    bne a0, t0, .Luser_failed
    li t0, 0x456
    bne t1, t0, .Luser_failed
    li t0, 0x123
    bne s0, t0, .Luser_failed
    ld t1, 0(sp)
    bne t1, t0, .Luser_failed

    # Copy the message into the user stack for the console syscall.
    addi sp, sp, -32
    lla t0, .Luser_message
    lla t1, .Luser_message_end
    sub a1, t1, t0
    mv t2, sp
.Lcopy_message:
    lbu t3, 0(t0)
    sb t3, 0(t2)
    addi t0, t0, 1
    addi t2, t2, 1
    bne t0, t1, .Lcopy_message
    mv a0, sp
    li a7, SYS_WRITE_CONSOLE
    ecall
    bne a0, a1, .Luser_failed
    li a0, 0
    j .Luser_exit
.Luser_failed:
    li a0, 1
.Luser_exit:
    li a7, SYS_EXIT
    ecall
1:
    j 1b
    .size _start, . - _start

    .section .rodata, "a", @progbits
.Luser_message:
    .ascii "Hello from user mode\r\n"
.Luser_message_end:
