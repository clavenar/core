pub use clv_abi::net::MAX_FRAME_BYTES;

use clv_abi::SYS_NET_FRAME;
use clv_abi::net::FrameOperation;

use super::{read_into, write_from};

/// Receives into a full-sized Ethernet buffer on the service's stack.
/// An empty slice means the caller can retry when the kernel cannot park it.
pub fn receive(buffer: &mut [u8; MAX_FRAME_BYTES]) -> Option<&[u8]> {
    read_into(SYS_NET_FRAME, FrameOperation::Receive as usize, buffer)
}

/// Queues a frame from the service's stack. A zero result asks the caller to retry.
pub fn transmit(frame: &[u8]) -> Option<usize> {
    write_from(SYS_NET_FRAME, FrameOperation::Transmit as usize, frame)
}
