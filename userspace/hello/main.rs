#![no_std]
#![no_main]

use clv::{LaunchRequest, fs, pipe};
use clv_abi::{
    GrantKind, MESSAGE_MAX_BYTES, PROBE_VALUE, ReplyKind, RequestKind, SYS_EXIT, SYS_PROBE,
    SYS_TIMER_FIRED, SYS_WRITE_CONSOLE, SYSCALL_ERROR,
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

extern "C" fn reply_or_report_ready(handle: usize, grant_kind: usize, writer: usize) -> usize {
    let mut buffer = [0; MESSAGE_MAX_BYTES];
    if grant_kind == GrantKind::File as usize {
        if writer != 0 {
            let status = copy_file(handle, |bytes| pipe::write(writer, bytes));
            pipe::close(writer);
            return status;
        }
        return reply_with_file(Some(handle), &mut buffer);
    }
    if let Some(LaunchRequest::Received { kind, bytes, .. }) = clv::receive_request(&mut buffer) {
        return match kind {
            RequestKind::Echo => usize::from(!clv::send_reply(ReplyKind::Echo, bytes)),
            RequestKind::ReadFile if grant_kind != GrantKind::Directory as usize => {
                clv::send_reply(ReplyKind::Denied, &[]);
                1
            }
            RequestKind::ReadFile => {
                let file = fs::open_at(handle, bytes);
                reply_with_file(file, &mut buffer)
            }
            RequestKind::Argument => {
                clv::send_reply(ReplyKind::ReadFailed, &[]);
                1
            }
        };
    }

    clv::send_reply(ReplyKind::Echo, b"hello ready");
    // The assembly entry continues with the ordinary greeting or granted file.
    SYSCALL_ERROR
}

fn reply_with_file(file: Option<usize>, buffer: &mut [u8; MESSAGE_MAX_BYTES]) -> usize {
    let contents = file.and_then(|handle| {
        let contents = fs::read(handle, buffer);
        fs::close(handle);
        contents
    });
    match contents {
        Some(bytes) => usize::from(!clv::send_reply(ReplyKind::FileData, bytes)),
        None => {
            clv::send_reply(ReplyKind::ReadFailed, &[]);
            1
        }
    }
}

extern "C" fn print_granted_hello(directory: usize) -> usize {
    let Some(handle) = fs::open_at(directory, b"hello.txt") else {
        clv::print(b"cannot open hello.txt\r\n");
        return 1;
    };
    copy_file(handle, clv::write_console)
}

fn copy_file(handle: usize, write: impl Fn(&[u8]) -> Option<usize>) -> usize {
    let mut buffer = [0; 128];
    let status = 'file: loop {
        let Some(mut bytes) = fs::read(handle, &mut buffer) else {
            break 1;
        };
        if bytes.is_empty() {
            break 0;
        }
        while !bytes.is_empty() {
            match write(bytes) {
                Some(count) if count > 0 && count <= bytes.len() => bytes = &bytes[count..],
                _ => break 'file 1,
            }
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
