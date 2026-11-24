#![no_std]
#![no_main]

use core::arch::{asm, global_asm};
use core::panic::PanicInfo;
use core::ptr::{without_provenance, without_provenance_mut};

global_asm!(include_str!("arch/riscv64/boot.S"));
global_asm!(include_str!("arch/riscv64/trap.S"));

// Make Cargo rebuild the kernel when the linker script changes.
const _: &str = include_str!("../../linker/riscv64.ld");

const UART_BASE: usize = 0x1000_0000;
const UART_LINE_STATUS: usize = 5;
const UART_TX_READY: u8 = 1 << 5;

// SAFETY: The boot stub calls this symbol with a RISC-V ABI-aligned stack.
#[unsafe(no_mangle)]
extern "C" fn kernel_main() -> ! {
    let banner = concat!(
        "Clavenar kernel booted (v",
        env!("CARGO_PKG_VERSION"),
        ")\r\n"
    );
    uart_write(banner.as_bytes());
    idle();
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
    loop {
        // SAFETY: WFI is available in supervisor mode on this target and
        // does not access memory or modify the stack.
        unsafe { asm!("wfi", options(nomem, nostack)) };
    }
}
