//! Ethernet frame I/O for the dedicated network service.
//!
//! At boot, the service receives its MAC in the low 48 bits of a1 in
//! little-endian order and a 64-bit transport seed in a2. The kernel
//! supplies zero as the prototype seed when it cannot read the entropy device.
//! Register a3 holds the signed offset in nanoseconds from guest monotonic
//! time to Unix time, or NO_UNIX_TIME_OFFSET.

pub const ETHERNET_HEADER_BYTES: usize = 14;
pub const MAX_FRAME_BYTES: usize = ETHERNET_HEADER_BYTES + 1500;
pub const NO_RECEIVE_DEADLINE: u64 = 0;
pub const NO_UNIX_TIME_OFFSET: i64 = i64::MIN;

/// Applies the boot clock offset to a guest monotonic timestamp.
pub fn unix_nanos(monotonic_nanos: u64, offset: i64) -> Option<u64> {
    if offset == NO_UNIX_TIME_OFFSET {
        return None;
    }
    monotonic_nanos.checked_add_signed(offset)
}

#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(usize)]
pub enum FrameOperation {
    /// Pass an absolute deadline in guest monotonic nanoseconds in a3, or
    /// NO_RECEIVE_DEADLINE to wait for a frame. A successful receive returns the
    /// length in a0 (zero for timeout or retry) and current monotonic nanoseconds in a1.
    Receive = 0,
    Transmit = 1,
}

impl FrameOperation {
    pub const fn from_register(value: usize) -> Option<Self> {
        match value {
            0 => Some(Self::Receive),
            1 => Some(Self::Transmit),
            _ => None,
        }
    }
}
