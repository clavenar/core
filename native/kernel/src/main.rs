#![no_std]
#![no_main]

use core::arch::{asm, global_asm};
use core::panic::PanicInfo;
use core::ptr::{without_provenance, without_provenance_mut};

global_asm!(include_str!("arch/riscv64/boot.S"));

// Make Cargo rebuild the kernel when the linker script changes.
const _: &str = include_str!("../../linker/riscv64.ld");

const UART_BASE: usize = 0x1000_0000;
const UART_LINE_STATUS: usize = 5;
const UART_TX_READY: u8 = 1 << 5;

// SAFETY: The boot stub calls this symbol with a RISC-V ABI-aligned stack.
#[unsafe(no_mangle)]
extern "C" fn kernel_main() -> ! {
    for &byte in b"Clavenar kernel booted\r\n" {
        uart_write_byte(byte);
    }
    idle();
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
