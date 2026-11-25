use core::arch::asm;
use core::sync::atomic::{AtomicU8, Ordering};

pub(crate) const INTERRUPT_CAUSE: usize = (1 << (usize::BITS - 1)) | 5;

const SBI_TIME: usize = 0x5449_4d45;
const SBI_SET_TIMER: usize = 0;
const SIE_STIE: usize = 1 << 5;
const DELAY_TICKS: u64 = 25_000;

const UNARMED: u8 = 0;
const ARMED: u8 = 1;
const FIRED: u8 = 2;
static STATE: AtomicU8 = AtomicU8::new(UNARMED);

/// Arms the boot hart's timer once from the user trap handler.
pub(crate) fn arm_once() -> bool {
    if STATE.load(Ordering::Relaxed) != UNARMED {
        return true;
    }

    let now: u64;
    // SAFETY: OpenSBI exposes the time counter to the supervisor on QEMU virt.
    unsafe { asm!("rdtime {now}", now = out(reg) now, options(nomem, nostack)) };
    if !set_deadline(now.saturating_add(DELAY_TICKS)) {
        return false;
    }

    STATE.store(ARMED, Ordering::Relaxed);
    // SAFETY: The user trap frame is installed and global supervisor
    // interrupts stay disabled until execution returns to user mode.
    unsafe { asm!("csrs sie, {mask}", mask = in(reg) SIE_STIE, options(nostack)) };
    true
}

pub(crate) fn complete() {
    cancel();
    STATE.store(FIRED, Ordering::Relaxed);
}

pub(crate) fn cancel() {
    if STATE.load(Ordering::Relaxed) != ARMED {
        return;
    }
    // SAFETY: Only the supervisor timer source is masked.
    unsafe { asm!("csrc sie, {mask}", mask = in(reg) SIE_STIE, options(nostack)) };
    // The interrupt stays masked even if firmware cannot clear the deadline.
    let _ = set_deadline(u64::MAX);
    STATE.store(UNARMED, Ordering::Relaxed);
}

pub(crate) fn fired() -> bool {
    STATE.load(Ordering::Relaxed) == FIRED
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
