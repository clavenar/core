#![no_std]
#![no_main]

use clv::LaunchRequest;
use clv_abi::{MESSAGE_MAX_BYTES, ReplyKind, RequestKind};
use core::panic::PanicInfo;

// SAFETY: The kernel enters this unique symbol with an aligned user stack.
#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
extern "C" fn _start() -> ! {
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

#[panic_handler]
fn panic(_info: &PanicInfo<'_>) -> ! {
    clv::exit(1)
}
