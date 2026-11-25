#![no_std]
#![no_main]

use clv_abi::{PROBE_VALUE, SYS_EXIT, SYS_PROBE, SYS_TIMER_FIRED, SYS_WRITE_CONSOLE};
use core::arch::global_asm;
use core::panic::PanicInfo;

global_asm!(
    include_str!("hello.S"),
    probe_call = const SYS_PROBE,
    exit_call = const SYS_EXIT,
    console_call = const SYS_WRITE_CONSOLE,
    timer_call = const SYS_TIMER_FIRED,
    probe_value = const PROBE_VALUE,
);

#[panic_handler]
fn panic(_info: &PanicInfo<'_>) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
