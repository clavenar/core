#![no_std]
#![no_main]

mod memory;
#[path = "arch/riscv64/timer.rs"]
mod timer;

use core::arch::{asm, global_asm};
use core::panic::PanicInfo;
use core::ptr::{without_provenance, without_provenance_mut};
use memory::frame::FrameAllocator;
use memory::sv39;
use memory::user_image;
use memory::usercopy::{SSTATUS_SUM, copy_from_user_stack};

global_asm!(include_str!("arch/riscv64/boot.S"));
global_asm!(
    include_str!("arch/riscv64/trap.S"),
    frame_size = const size_of::<TrapFrame>(),
    sepc_offset = const core::mem::offset_of!(TrapFrame, sepc),
    sstatus_offset = const core::mem::offset_of!(TrapFrame, sstatus),
    sum_mask = const SSTATUS_SUM,
);
global_asm!(include_str!("arch/riscv64/user.S"));

// Make Cargo rebuild the kernel when the linker script changes.
const _: &str = include_str!("../../linker/riscv64.ld");

const UART_BASE: usize = 0x1000_0000;
const UART_LINE_STATUS: usize = 5;
const UART_TX_READY: u8 = 1 << 5;
const USER_ECALL: usize = 8;
const SYS_PROBE: usize = 0;
const SYS_EXIT: usize = 1;
const SYS_WRITE_CONSOLE: usize = 2;
const SYS_TIMER_FIRED: usize = 3;
const SYSCALL_ERROR: usize = usize::MAX;
const CONSOLE_WRITE_LIMIT: usize = 128;
const PROBE_VALUE: usize = 42;
const REG_A0: usize = 10;
const REG_A1: usize = 11;
const REG_A7: usize = 17;
const USER_ELF: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/hello.elf"));

#[repr(C, align(16))]
struct TrapFrame {
    registers: [usize; 32],
    sepc: usize,
    sstatus: usize,
}

unsafe extern "C" {
    fn enter_user(entry: usize) -> !;
}

// SAFETY: The boot stub calls this symbol with a RISC-V ABI-aligned stack.
#[unsafe(no_mangle)]
extern "C" fn kernel_main() -> ! {
    let banner = concat!(
        "Clavenar kernel booted (v",
        env!("CARGO_PKG_VERSION"),
        ")\r\n"
    );
    uart_write(banner.as_bytes());

    // SAFETY: The single boot hart claims the frame pool here once.
    let mut frames = unsafe { FrameAllocator::from_boot_pool() };
    let frame = frames.allocate_zeroed().expect("boot frame pool is empty");
    uart_write(b"boot frame=");
    uart_write_hex(frame.physical_address());
    uart_write(b"\r\n");

    let user_code = frames.allocate_zeroed().expect("boot frame pool is empty");
    let Some(entry) = user_image::load(USER_ELF, user_code.bytes_mut()) else {
        uart_write(b"User ELF load failed\r\n");
        idle();
    };
    uart_write(b"User ELF loaded entry=");
    uart_write_hex(entry);
    uart_write(b"\r\n");

    // SAFETY: This is the first address-space switch on the boot hart.
    if !unsafe {
        sv39::enable_identity_map(frame, &mut frames, user_code.physical_address(), UART_BASE)
    } {
        uart_write(b"Sv39 paging setup failed\r\n");
        idle();
    }
    uart_write(b"Sv39 paging enabled\r\n");
    // SAFETY: The user code and stack pages are mapped and the supervisor
    // trap vector is installed.
    unsafe { enter_user(entry) }
}

// SAFETY: The trap stub uses this unique symbol with the RISC-V C ABI.
#[unsafe(no_mangle)]
extern "C" fn supervisor_user_trap(frame: &mut TrapFrame, scause: usize, stval: usize) {
    if scause == timer::INTERRUPT_CAUSE {
        timer::complete();
        uart_write(b"user timer interrupt\r\n");
        // Resume at the interrupted user instruction.
        return;
    }
    if scause != USER_ECALL {
        supervisor_trap_fatal(scause, frame.sepc, stval);
    }

    match frame.registers[REG_A7] {
        SYS_PROBE => {
            uart_write(b"user probe\r\n");
            frame.registers[REG_A0] = if timer::arm_once() {
                PROBE_VALUE
            } else {
                uart_write(b"SBI timer setup failed\r\n");
                SYSCALL_ERROR
            };
        }
        SYS_EXIT => {
            uart_write(b"user exit status=");
            uart_write_hex(frame.registers[REG_A0]);
            uart_write(b"\r\n");
            idle();
        }
        SYS_WRITE_CONSOLE => {
            frame.registers[REG_A0] =
                write_console(frame.registers[REG_A0], frame.registers[REG_A1]);
        }
        SYS_TIMER_FIRED => frame.registers[REG_A0] = usize::from(timer::fired()),
        _ => frame.registers[REG_A0] = SYSCALL_ERROR,
    }
    // ECALL is a 32-bit instruction, including on cores with compressed code.
    frame.sepc += 4;
}

fn write_console(address: usize, length: usize) -> usize {
    let mut buffer = [0; CONSOLE_WRITE_LIMIT];
    let Some(bytes) = buffer.get_mut(..length) else {
        return SYSCALL_ERROR;
    };
    if !copy_from_user_stack(address, bytes) {
        return SYSCALL_ERROR;
    }

    uart_write(bytes);
    length
}

// SAFETY: The trap stub uses this unique symbol with the RISC-V C ABI.
#[unsafe(no_mangle)]
extern "C" fn supervisor_trap_fatal(scause: usize, sepc: usize, stval: usize) -> ! {
    uart_write(b"fatal trap scause=");
    uart_write_hex(scause);
    uart_write(b" sepc=");
    uart_write_hex(sepc);
    uart_write(b" stval=");
    uart_write_hex(stval);
    uart_write(b"\r\n");
    idle();
}

fn uart_write(bytes: &[u8]) {
    for &byte in bytes {
        uart_write_byte(byte);
    }
}

fn uart_write_hex(value: usize) {
    uart_write(b"0x");
    for shift in (0..usize::BITS).step_by(4).rev() {
        let digit = (value >> shift) & 0xf;
        uart_write_byte(b"0123456789abcdef"[digit]);
    }
}

fn uart_write_byte(byte: u8) {
    let line_status = without_provenance::<u8>(UART_BASE + UART_LINE_STATUS);
    let transmit = without_provenance_mut::<u8>(UART_BASE);

    // SAFETY: QEMU virt maps byte-wide UART registers here. OpenSBI has
    // configured the UART for console output before handing off to us.
    unsafe {
        while line_status.read_volatile() & UART_TX_READY == 0 {
            core::hint::spin_loop();
        }
        transmit.write_volatile(byte);
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo<'_>) -> ! {
    idle();
}

fn idle() -> ! {
    timer::cancel();
    loop {
        // SAFETY: WFI is available in supervisor mode on this target and
        // does not access memory or modify the stack.
        unsafe { asm!("wfi", options(nomem, nostack)) };
    }
}
