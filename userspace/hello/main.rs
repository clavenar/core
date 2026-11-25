#![no_std]
#![no_main]

use clv_abi::{
    FILE_REPLY_ERROR, FILE_REPLY_OK, MESSAGE_MAX_BYTES, NO_CHILD_HANDLE, PROBE_VALUE, SYS_CLOSE,
    SYS_EXIT, SYS_MESSAGE_RECV, SYS_MESSAGE_SEND, SYS_OPEN_AT, SYS_PROBE, SYS_READ,
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
    reply_or_report_ready = sym reply_or_report_ready,
    print_granted_hello = sym print_granted_hello,
);

extern "C" fn reply_or_report_ready(directory: usize, scoped: usize) -> usize {
    let mut buffer = [0; MESSAGE_MAX_BYTES];
    let address = buffer.as_mut_ptr().expose_provenance();
    let count = syscall(SYS_MESSAGE_RECV, address, buffer.len(), NO_CHILD_HANDLE);
    if (1..=buffer.len()).contains(&count) {
        if scoped != 0 {
            return reply_with_file(directory, &mut buffer, count);
        }
        return usize::from(syscall(SYS_MESSAGE_SEND, address, count, 0) != count);
    }

    let ready = b"hello ready";
    buffer[..ready.len()].copy_from_slice(ready);
    syscall(SYS_MESSAGE_SEND, address, ready.len(), 0);
    // Continue with the ordinary greeting or granted file.
    SYSCALL_ERROR
}

fn reply_with_file(
    directory: usize,
    buffer: &mut [u8; MESSAGE_MAX_BYTES],
    path_length: usize,
) -> usize {
    let address = buffer.as_mut_ptr().expose_provenance();
    let handle = syscall(SYS_OPEN_AT, directory, address, path_length);
    let count = if handle == SYSCALL_ERROR {
        SYSCALL_ERROR
    } else {
        let data = &mut buffer[1..];
        let count = syscall(
            SYS_READ,
            handle,
            data.as_mut_ptr().expose_provenance(),
            data.len(),
        );
        syscall(SYS_CLOSE, handle, 0, 0);
        count
    };
    let (status, length) = if count < buffer.len() {
        (FILE_REPLY_OK, count + 1)
    } else {
        (FILE_REPLY_ERROR, 1)
    };
    buffer[0] = status;
    let sent = syscall(SYS_MESSAGE_SEND, address, length, 0);
    usize::from(status == FILE_REPLY_ERROR || sent != length)
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
    // SAFETY: Clavenar returns results in a0 and a1 and preserves other registers.
    unsafe {
        asm!(
            "ecall",
            inlateout("a0") a0,
            inlateout("a1") a1 => _,
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
