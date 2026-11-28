#![no_std]
#![no_main]

mod mcp_client;

use clv::{ChildReply, ChildStatus, LaunchRequest, agent, bridge, fs, pipe};
use clv_abi::agent::{LAUNCH_REQUEST_MAX_BYTES, decode_rename_pair};
use clv_abi::net::{HTTPS_PROBE_BODY, HttpsRequest, HttpsStatus, MCP_INIT_REPLY};
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
        }) => run_plan(kind, bytes),
        Some(LaunchRequest::Received {
            kind: kind @ (RequestKind::HttpsProbe | RequestKind::McpInit),
            ..
        }) => {
            let (request, body, failure) = if kind == RequestKind::McpInit {
                (
                    HttpsRequest::McpInit,
                    MCP_INIT_REPLY,
                    ReplyKind::McpInitFailed,
                )
            } else {
                (
                    HttpsRequest::Probe,
                    HTTPS_PROBE_BODY,
                    ReplyKind::HttpsFailed,
                )
            };
            let result = clv::net::request_https(request);
            if result == HttpsStatus::Complete {
                usize::from(!clv::send_reply(ReplyKind::Matches, body))
            } else {
                clv::send_reply(failure, &[result as u8]);
                1
            }
        }
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

fn run_plan(kind: RequestKind, bytes: &[u8]) -> usize {
    match agent::DefinitionId::from_request(kind, bytes) {
        Some(agent::FIND_GREETING_V1) => {
            let action = include_bytes!(concat!(env!("OUT_DIR"), "/find-greeting-v1.action"));
            supervise_helper(delegate_file(action))
        }
        Some(agent::FIND_GREETING_V2) => {
            let request = agent::FileToolInvocation::decode(include_bytes!(concat!(
                env!("OUT_DIR"),
                "/find-greeting-v2.action"
            )));
            let helper =
                request.and_then(|request| agent::invoke_file_tool(agent::CURRENT_AGENT, &request));
            supervise_helper(helper)
        }
        Some(agent::FIND_GREETING_V3) => {
            let Some(request) = agent::FileToolInvocation::decode(include_bytes!(concat!(
                env!("OUT_DIR"),
                "/find-greeting-v3.action"
            ))) else {
                return delegation_failed();
            };
            supervise_stdio_tool(&request)
        }
        Some(
            id @ (agent::FIND_GREETING_V4
            | agent::FIND_GREETING_V5
            | agent::FIND_GREETING_V6
            | agent::FIND_GREETING_V8),
        ) => {
            let action: &[u8] = match id {
                agent::FIND_GREETING_V4 => {
                    include_bytes!(concat!(env!("OUT_DIR"), "/find-greeting-v4.action"))
                }
                agent::FIND_GREETING_V5 => {
                    include_bytes!(concat!(env!("OUT_DIR"), "/find-greeting-v5.action"))
                }
                agent::FIND_GREETING_V6 => {
                    include_bytes!(concat!(env!("OUT_DIR"), "/find-greeting-v6.action"))
                }
                _ => include_bytes!(concat!(env!("OUT_DIR"), "/find-greeting-v8.action")),
            };
            let Some(request) = agent::FileToolInvocation::decode(action) else {
                return delegation_failed();
            };
            if id != agent::FIND_GREETING_V4
                && let Err(reason) = plan_search()
            {
                return host_failure(ReplyKind::HostPlanFailed, reason);
            }
            run_mcp_search(&request, id == agent::FIND_GREETING_V8)
        }
        Some(agent::FIND_GREETING_V7) => {
            let mut frame = [0; bridge::MAX_FRAME_BYTES];
            match bridge::tool_text(&mut frame) {
                Ok(text) => send_matches(text),
                Err(reason) => host_failure(ReplyKind::HostToolFailed, reason),
            }
        }
        _ => delegation_failed(),
    }
}

fn plan_search() -> Result<(), bridge::BridgeFailure> {
    match bridge::plan_action()? {
        bridge::SEARCH_GREETING => Ok(()),
        bridge::BUDGET_EXHAUSTED => Err(bridge::BridgeFailure::BudgetExhausted),
        bridge::SKIP => Err(bridge::BridgeFailure::Skipped),
        bridge::MODEL_UNAVAILABLE => Err(bridge::BridgeFailure::ModelUnavailable),
        _ => Err(bridge::BridgeFailure::InvalidReply),
    }
}

fn run_mcp_search(invocation: &agent::FileToolInvocation<'_>, include_host: bool) -> usize {
    let mut matches = [0; MESSAGE_MAX_BYTES];
    let length = match mcp_client::search(invocation, &mut matches) {
        Ok(length) => length,
        Err(ReplyKind::DelegationFailed) => return delegation_failed(),
        Err(kind) => {
            clv::send_reply(kind, &[]);
            return 2;
        }
    };
    if !include_host {
        return send_matches(&matches[..length]);
    }
    let mut frame = [0; bridge::MAX_FRAME_BYTES];
    let text = match bridge::tool_text(&mut frame) {
        Ok(text) => text,
        Err(reason) => return host_failure(ReplyKind::HostToolFailed, reason),
    };
    let separator = usize::from(length != 0 && !text.is_empty() && matches[length - 1] != b'\n');
    let end = length + separator + text.len();
    let Some(destination) = matches.get_mut(length..end) else {
        clv::send_reply(ReplyKind::ResultTooLarge, &[]);
        return 2;
    };
    if separator != 0 {
        destination[0] = b'\n';
    }
    destination[separator..].copy_from_slice(text);
    send_matches(&matches[..end])
}

fn send_matches(text: &[u8]) -> usize {
    if clv::send_reply(ReplyKind::Matches, text) {
        usize::from(text.is_empty())
    } else {
        2
    }
}

fn host_failure(kind: ReplyKind, reason: bridge::BridgeFailure) -> usize {
    clv::send_reply(kind, &[reason as u8]);
    2
}

fn delegation_failed() -> usize {
    clv::send_reply(ReplyKind::DelegationFailed, &[]);
    1
}

fn supervise_helper(helper: Option<usize>) -> usize {
    let Some(helper) = helper else {
        return delegation_failed();
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

fn supervise_stdio_tool(request: &agent::FileToolInvocation<'_>) -> usize {
    let Some(helper) = agent::invoke_file_tool_stdio(agent::CURRENT_AGENT, request) else {
        return delegation_failed();
    };
    let mut input = [0; MESSAGE_MAX_BYTES];
    let input = &mut input[..request.argument.len()];
    input.copy_from_slice(request.argument);
    let sent = pipe::write_all(helper, input).is_some();
    let stdin_closed = pipe::close_stdin(helper);

    let mut output = [0; MESSAGE_MAX_BYTES];
    let bytes = if sent && stdin_closed {
        pipe::read_to_end(helper, &mut output)
    } else {
        None
    };
    pipe::close(helper);
    let status = clv::wait(helper);
    let (Some(ChildStatus::Exited(code @ 0..=1)), Some(bytes)) = (status, bytes) else {
        let kind = match clv::receive_reply(helper, &mut []) {
            Some(ChildReply::Received {
                kind: kind @ (ReplyKind::ReadFailed | ReplyKind::ResultTooLarge),
                ..
            }) => kind,
            _ => ReplyKind::ReadFailed,
        };
        clv::send_reply(kind, &[]);
        return 2;
    };
    if clv::send_reply(ReplyKind::Matches, bytes) {
        code
    } else {
        2
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
