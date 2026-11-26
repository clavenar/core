pub(super) mod timer;
pub(crate) mod trap;

use core::arch::{asm, global_asm};

global_asm!(include_str!("boot.S"));

pub(crate) fn idle() -> ! {
    timer::cancel();
    loop {
        // SAFETY: WFI is available in supervisor mode on this target and
        // does not access memory or modify the stack.
        unsafe { asm!("wfi", options(nomem, nostack)) };
    }
}
