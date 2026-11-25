#![no_std]
#![no_main]

use clv_abi::{
    PROBE_VALUE, SYS_CLOSE, SYS_EXIT, SYS_MESSAGE_SEND, SYS_OPEN_AT, SYS_PROBE, SYS_READ,
    SYS_TIMER_FIRED, SYS_WRITE_CONSOLE, SYSCALL_ERROR,
};
use core::arch::{asm, global_asm};
use core::panic::PanicInfo;

global_asm!(
    include_str!("hello.S"),
    probe_call = const SYS_PROBE,
    exit_call = const SYS_EXIT,
    console_call = const SYS_WRITE_CONSOLE,
    timer_call = const SYS_TIMER_FIRED,
    probe_value = const PROBE_VALUE,
    send_hello_message = sym send_hello_message,
    print_granted_hello = sym print_granted_hello,
);

extern "C" fn send_hello_message() {
    let mut message = *b"hello ready";
    syscall(
        SYS_MESSAGE_SEND,
        message.as_mut_ptr().expose_provenance(),
        message.len(),
        0,
    );
}

extern "C" fn print_granted_hello(directory: usize) -> usize {
    let mut buffer = [0; 128];
    let address = buffer.as_mut_ptr().expose_provenance();
    let name = b"hello.txt";
    buffer[..name.len()].copy_from_slice(name);
    let handle = syscall(SYS_OPEN_AT, directory, address, name.len());
    if handle == SYSCALL_ERROR {
        let message = b"cannot open hello.txt\r\n";
        buffer[..message.len()].copy_from_slice(message);
        syscall(SYS_WRITE_CONSOLE, address, message.len(), 0);
        return 1;
    }

    let status = loop {
        let count = syscall(SYS_READ, handle, address, buffer.len());
        if count == 0 {
            break 0;
        }
        if count > buffer.len() || syscall(SYS_WRITE_CONSOLE, address, count, 0) != count {
            break 1;
        }
    };
    syscall(SYS_CLOSE, handle, 0, 0);
    status
}

fn syscall(number: usize, mut a0: usize, a1: usize, a2: usize) -> usize {
    // SAFETY: The calls here return their result in a0 and preserve other registers.
    unsafe {
        asm!(
            "ecall",
            inlateout("a0") a0,
            in("a1") a1,
            in("a2") a2,
            in("a7") number,
            options(nostack),
        );
    }
    a0
}

#[panic_handler]
fn panic(_info: &PanicInfo<'_>) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
