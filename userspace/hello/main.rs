#![no_std]
#![no_main]

use clv::{ChildReply, ChildStatus, LaunchRequest, agent, fs, pipe};
use clv_abi::agent::DELEGATION_REQUEST_MAX_BYTES;
use clv_abi::{
    GrantKind, MESSAGE_MAX_BYTES, PROBE_VALUE, Program, ReplyKind, RequestKind, SYS_EXIT,
    SYS_PROBE, SYS_TIMER_FIRED, SYS_WRITE_CONSOLE, SYSCALL_ERROR,
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
    let mut buffer = [0; DELEGATION_REQUEST_MAX_BYTES];
    match clv::receive_request(&mut buffer) {
        Some(LaunchRequest::Received {
            kind: RequestKind::DelegateWithFile,
            bytes,
            ..
        }) => supervise_helper(agent::FileDelegation::decode(bytes)),
        Some(LaunchRequest::Received {
            kind: RequestKind::PlanIntent,
            bytes,
            ..
        }) => supervise_helper(agent::PlanIntent::decode(bytes).map(mock_plan)),
        _ if grant_kind == GrantKind::File as usize && writer != 0 => {
            let status = copy_file(handle, |bytes| pipe::write(writer, bytes));
            pipe::close(writer);
            status
        }
        _ if grant_kind == GrantKind::File as usize => reply_with_file(Some(handle)),
        Some(LaunchRequest::Received {
            kind: RequestKind::Echo,
            bytes,
            ..
        }) => usize::from(!clv::send_reply(ReplyKind::Echo, bytes)),
        Some(LaunchRequest::Received {
            kind: RequestKind::ReadFile,
            bytes,
            ..
        }) => {
            if grant_kind == GrantKind::Directory as usize {
                reply_with_file(fs::open_at(handle, bytes))
            } else {
                clv::send_reply(ReplyKind::Denied, &[]);
                1
            }
        }
        Some(LaunchRequest::Received {
            kind: RequestKind::Argument,
            ..
        }) => {
            clv::send_reply(ReplyKind::ReadFailed, &[]);
            1
        }
        _ => {
            clv::send_reply(ReplyKind::Echo, b"hello ready");
            // The assembly entry continues with the ordinary greeting or granted file.
            SYSCALL_ERROR
        }
    }
}

fn mock_plan(intent: agent::PlanIntent) -> agent::FileDelegation<'static> {
    match intent {
        agent::PlanIntent::FindGreeting => agent::FileDelegation {
            program: Program::Grep,
            reply_bytes: 18,
            argument: b"Hello",
        },
    }
}

fn supervise_helper(request: Option<agent::FileDelegation<'_>>) -> usize {
    let Some(helper) = request.and_then(|request| agent::delegate_from_current(&request)) else {
        clv::send_reply(ReplyKind::DelegationFailed, &[]);
        return 1;
    };
    // Finish the helper and return its unused reservation before forwarding the reply.
    let status = clv::wait(helper);
    let mut buffer = [0; MESSAGE_MAX_BYTES];
    match (status, clv::receive_reply(helper, &mut buffer)) {
        (Some(ChildStatus::Exited(code)), Some(ChildReply::Received { kind, bytes, .. }))
            if clv::send_reply(kind, bytes) =>
        {
            code
        }
        _ => 2,
    }
}

fn reply_with_file(file: Option<usize>) -> usize {
    let mut buffer = [0; MESSAGE_MAX_BYTES];
    let contents = file.and_then(|handle| {
        let contents = fs::read(handle, &mut buffer);
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
