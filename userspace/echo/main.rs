#![no_std]
#![no_main]

use clv_abi::{MESSAGE_MAX_BYTES, NO_CHILD_HANDLE, SYS_EXIT, SYS_MESSAGE_RECV, SYS_MESSAGE_SEND};
use core::arch::asm;
use core::panic::PanicInfo;

// SAFETY: The kernel enters this unique symbol with an aligned user stack.
#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
extern "C" fn _start() -> ! {
    let mut buffer = [0; MESSAGE_MAX_BYTES];
    let address = buffer.as_mut_ptr().expose_provenance();
    let length = match syscall(SYS_MESSAGE_RECV, address, buffer.len(), NO_CHILD_HANDLE) {
        0 => {
            let ready = b"echo ready";
            buffer[..ready.len()].copy_from_slice(ready);
            ready.len()
        }
        length if length <= buffer.len() => length,
        _ => exit(1),
    };
    let count = syscall(SYS_MESSAGE_SEND, address, length, 0);
    exit(usize::from(count != length));
}

fn exit(status: usize) -> ! {
    syscall(SYS_EXIT, status, 0, 0);
    loop {
        core::hint::spin_loop();
    }
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
    exit(1)
}
