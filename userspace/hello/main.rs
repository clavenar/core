#![no_std]
#![no_main]

use clv::{LaunchRequest, fs};
use clv_abi::{
    FILE_REPLY_ERROR, FILE_REPLY_OK, GrantKind, MESSAGE_MAX_BYTES, PROBE_VALUE, RequestKind,
    SYS_EXIT, SYS_PROBE, SYS_TIMER_FIRED, SYS_WRITE_CONSOLE, SYSCALL_ERROR,
};
use core::arch::global_asm;
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

extern "C" fn reply_or_report_ready(handle: usize, grant_kind: usize) -> usize {
    let mut buffer = [0; MESSAGE_MAX_BYTES];
    if grant_kind == GrantKind::File as usize {
        return reply_with_file(Some(handle), &mut buffer);
    }
    if let Some(LaunchRequest::Received { kind, bytes, .. }) = clv::receive_request(&mut buffer) {
        return match kind {
            RequestKind::Echo => usize::from(!clv::send_message(bytes)),
            RequestKind::ReadFile => {
                let file = if grant_kind == GrantKind::Directory as usize {
                    fs::open_at(handle, bytes)
                } else {
                    None
                };
                reply_with_file(file, &mut buffer)
            }
        };
    }

    clv::send_message(b"hello ready");
    // The assembly entry continues with the ordinary greeting or granted file.
    SYSCALL_ERROR
}

fn reply_with_file(file: Option<usize>, buffer: &mut [u8; MESSAGE_MAX_BYTES]) -> usize {
    let count = file.and_then(|handle| {
        let count = fs::read(handle, &mut buffer[1..]).map(<[u8]>::len);
        fs::close(handle);
        count
    });
    let (status, length) = match count {
        Some(count) => (FILE_REPLY_OK, count + 1),
        None => (FILE_REPLY_ERROR, 1),
    };
    buffer[0] = status;
    let sent = clv::send_message(&buffer[..length]);
    usize::from(status == FILE_REPLY_ERROR || !sent)
}

extern "C" fn print_granted_hello(directory: usize) -> usize {
    let Some(handle) = fs::open_at(directory, b"hello.txt") else {
        clv::print(b"cannot open hello.txt\r\n");
        return 1;
    };

    let mut buffer = [0; 128];
    let status = loop {
        let Some(bytes) = fs::read(handle, &mut buffer) else {
            break 1;
        };
        if bytes.is_empty() {
            break 0;
        }
        if clv::write_console(bytes) != Some(bytes.len()) {
            break 1;
        }
    };
    fs::close(handle);
    status
}

#[panic_handler]
fn panic(_info: &PanicInfo<'_>) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
