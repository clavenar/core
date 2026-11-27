#![no_std]
#![no_main]

use clv::{ChildReply, ChildStatus, LaunchRequest, agent, fs, pipe};
use clv_abi::agent::{LAUNCH_REQUEST_MAX_BYTES, decode_rename_pair};
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

extern "C" fn reply_or_report_ready(handle: usize, grant_kind: usize, extra: usize) -> usize {
    let mut buffer = [0; LAUNCH_REQUEST_MAX_BYTES];
    match clv::receive_request(&mut buffer) {
        // Submit every rename request so the kernel can attribute a missing grant.
        Some(LaunchRequest::Received {
            kind: RequestKind::StageRename,
            bytes,
            ..
        }) => usize::from(!agent::stage_rename(handle, bytes)),
        Some(LaunchRequest::Received {
            kind: RequestKind::StageRenamePair,
            bytes,
            ..
        }) => {
            if grant_kind != GrantKind::RenamePair as usize {
                return 1;
            }
            let Some([first, second]) = decode_rename_pair(bytes) else {
                return 1;
            };
            let staged_first = agent::stage_rename(handle, first);
            let staged_second = agent::stage_rename(extra, second);
            usize::from(!(staged_first && staged_second))
        }
        Some(LaunchRequest::Received {
            kind: RequestKind::DelegateWithFile,
            bytes,
            ..
        }) => supervise_helper(delegate_file(bytes)),
        Some(LaunchRequest::Received {
            kind: kind @ (RequestKind::PlanIntent | RequestKind::PlanDefinition),
            bytes,
            ..
        }) => supervise_helper(launch_planned_helper(kind, bytes)),
        _ if grant_kind == GrantKind::File as usize && extra != 0 => {
            let status = copy_file(handle, |bytes| pipe::write(extra, bytes));
            pipe::close(extra);
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

fn delegate_file(bytes: &[u8]) -> Option<usize> {
    agent::delegate_from_current(&agent::FileDelegation::decode(bytes)?)
}

fn launch_planned_helper(kind: RequestKind, bytes: &[u8]) -> Option<usize> {
    match agent::DefinitionId::from_request(kind, bytes)? {
        agent::FIND_GREETING_V1 => delegate_file(include_bytes!(concat!(
            env!("OUT_DIR"),
            "/find-greeting-v1.action"
        ))),
        agent::FIND_GREETING_V2 => {
            let request = agent::FileToolInvocation::decode(include_bytes!(concat!(
                env!("OUT_DIR"),
                "/find-greeting-v2.action"
            )))?;
            agent::invoke_file_tool(agent::CURRENT_AGENT, &request)
        }
        _ => None,
    }
}

fn supervise_helper(helper: Option<usize>) -> usize {
    let Some(helper) = helper else {
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
