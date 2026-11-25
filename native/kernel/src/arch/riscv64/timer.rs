use core::arch::asm;
use core::sync::atomic::{AtomicBool, Ordering};

pub(crate) const INTERRUPT_CAUSE: usize = (1 << (usize::BITS - 1)) | 5;

const SBI_TIME: usize = 0x5449_4d45;
const SBI_SET_TIMER: usize = 0;
const SIE_STIE: usize = 1 << 5;
const QUANTUM_TICKS: u64 = 25_000;
static ARMED: AtomicBool = AtomicBool::new(false);

/// Starts a fresh quantum on the boot hart.
pub(crate) fn arm() -> bool {
    let now: u64;
    // SAFETY: OpenSBI exposes the time counter to the supervisor on QEMU virt.
    unsafe { asm!("rdtime {now}", now = out(reg) now, options(nomem, nostack)) };
    if !set_deadline(now.saturating_add(QUANTUM_TICKS)) {
        return false;
    }

    ARMED.store(true, Ordering::Relaxed);
    // SAFETY: The user trap frame is installed and global supervisor
    // interrupts stay disabled until execution returns to user mode.
    unsafe { asm!("csrs sie, {mask}", mask = in(reg) SIE_STIE, options(nostack)) };
    true
}

pub(crate) fn cancel() {
    if !ARMED.load(Ordering::Relaxed) {
        return;
    }
    // SAFETY: Only the supervisor timer source is masked.
    unsafe { asm!("csrc sie, {mask}", mask = in(reg) SIE_STIE, options(nostack)) };
    // The interrupt stays masked even if firmware cannot clear the deadline.
    let _ = set_deadline(u64::MAX);
    ARMED.store(false, Ordering::Relaxed);
}

fn set_deadline(deadline: u64) -> bool {
    let error: isize;
    // SAFETY: The SBI TIME call takes an absolute deadline in a0 and returns
    // a result in a0 and a1. The remaining registers are preserved by SBI.
    unsafe {
        asm!(
            "ecall",
            inlateout("a0") deadline => error,
            lateout("a1") _,
            in("a6") SBI_SET_TIMER,
            in("a7") SBI_TIME,
            options(nostack),
        );
    }
    error == 0
}
