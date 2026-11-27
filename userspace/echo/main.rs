#![no_std]
#![no_main]

use clv::{LaunchRequest, pipe};
use clv_abi::{MESSAGE_MAX_BYTES, NO_CHILD_HANDLE, PIPE_CAPACITY, ReplyKind, RequestKind};
use core::panic::PanicInfo;

// SAFETY: The kernel enters this unique symbol with an aligned user stack.
#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
extern "C" fn _start(_task: usize, _file: usize, grant_kind: usize, io: usize) -> ! {
    if grant_kind == 0 && io != NO_CHILD_HANDLE {
        clv::exit(usize::from(echo_stdio(io).is_none()));
    }
    let mut buffer = [0; MESSAGE_MAX_BYTES];
    let reply: &[u8] = match clv::receive_request(&mut buffer) {
        Some(LaunchRequest::Empty) => b"echo ready",
        Some(LaunchRequest::Received {
            kind: RequestKind::ObserveGoal,
            ..
        }) => clv::exit(usize::from(clv::goal::observe().is_none())),
        Some(LaunchRequest::Received {
            kind: RequestKind::ResumeAgent,
            ..
        }) => clv::exit(usize::from(!clv::agent::resume_granted())),
        Some(LaunchRequest::Received {
            kind: RequestKind::ReviewRename,
            ..
        }) => {
            let mut diff = [0; clv_abi::goal::RENAME_BATCH_DIFF_MAX_BYTES];
            clv::exit(usize::from(
                clv::goal::review_batch_granted(&mut diff).is_none(),
            ))
        }
        Some(LaunchRequest::Received {
            kind: RequestKind::Echo,
            bytes,
            ..
        }) => bytes,
        Some(LaunchRequest::Received { .. }) => {
            clv::send_reply(ReplyKind::ReadFailed, &[]);
            clv::exit(1)
        }
        None => clv::exit(1),
    };
    let sent = clv::send_reply(ReplyKind::Echo, reply);
    clv::exit(usize::from(!sent));
}

fn echo_stdio(handle: usize) -> Option<()> {
    let mut buffer = [0; PIPE_CAPACITY];
    loop {
        let bytes = pipe::read(handle, &mut buffer)?;
        if bytes.is_empty() {
            return Some(());
        }
        pipe::write_all(handle, bytes)?;
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo<'_>) -> ! {
    clv::exit(1)
}
