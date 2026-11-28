//! Ethernet frame I/O for the dedicated network service.

pub const ETHERNET_HEADER_BYTES: usize = 14;
pub const MAX_FRAME_BYTES: usize = ETHERNET_HEADER_BYTES + 1500;

#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(usize)]
pub enum FrameOperation {
    /// A nonempty receive returns the frame length in a0 and guest monotonic nanoseconds in a1.
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
