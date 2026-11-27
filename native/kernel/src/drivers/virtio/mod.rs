pub(crate) mod block;
pub(crate) mod console;
mod mmio;
pub(crate) mod net;
mod queue;

use core::arch::asm;

fn order_device_accesses() {
    // SAFETY: This orders coherent DMA memory and MMIO accesses on the RV64 hart.
    unsafe { asm!("fence iorw, iorw", options(nostack)) };
}
