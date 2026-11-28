use clv_abi::net::NO_UNIX_TIME_OFFSET;
use core::arch::asm;
use core::ptr::without_provenance;

use crate::arch::monotonic_nanos;

// QEMU RV64 virt places the Goldfish RTC at this physical address.
const TIME_LOW: usize = 0x0010_1000;
const TIME_HIGH: usize = TIME_LOW + 4;

/// # Safety
/// The caller must have access to QEMU virt's physical RTC registers.
pub(crate) unsafe fn monotonic_offset() -> i64 {
    let before = monotonic_nanos();
    // SAFETY: The aligned low register latches the high half of the Unix timestamp.
    let (low, high) = unsafe {
        let low = without_provenance::<u32>(TIME_LOW).read_volatile();
        asm!("fence iorw, iorw", options(nostack));
        let high = without_provenance::<u32>(TIME_HIGH).read_volatile();
        (u32::from_le(low), u32::from_le(high))
    };
    let after = monotonic_nanos();
    let unix_nanos = ((u64::from(high) << 32) | u64::from(low)) as i64;
    let offset = i128::from(unix_nanos) - i128::from(before.midpoint(after));
    i64::try_from(offset).unwrap_or(NO_UNIX_TIME_OFFSET)
}
