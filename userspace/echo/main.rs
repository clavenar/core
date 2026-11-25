#![no_std]
#![no_main]

use clv_abi::{MESSAGE_MAX_BYTES, NO_CHILD_HANDLE};
use core::panic::PanicInfo;

// SAFETY: The kernel enters this unique symbol with an aligned user stack.
#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
extern "C" fn _start() -> ! {
    let mut buffer = [0; MESSAGE_MAX_BYTES];
    let reply: &[u8] = match clv::receive_message(NO_CHILD_HANDLE, &mut buffer) {
        Some(([], _)) => b"echo ready",
        Some((message, _)) => message,
        None => clv::exit(1),
    };
    let sent = clv::send_message(reply);
    clv::exit(usize::from(!sent));
}

#[panic_handler]
fn panic(_info: &PanicInfo<'_>) -> ! {
    clv::exit(1)
}
