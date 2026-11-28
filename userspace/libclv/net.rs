pub use clv_abi::net::MAX_FRAME_BYTES;

use clv_abi::SYS_NET_FRAME;
use clv_abi::net::FrameOperation;

use super::{ecall, write_from};

/// Receives into a full-sized Ethernet buffer on the service's stack.
/// A nonempty frame comes with its receive time in guest monotonic nanoseconds.
/// An empty slice means the caller can retry when the kernel cannot park it.
pub fn receive(buffer: &mut [u8; MAX_FRAME_BYTES]) -> Option<(&[u8], u64)> {
    let (length, received_at, _, _) = ecall(
        SYS_NET_FRAME,
        FrameOperation::Receive as usize,
        buffer.as_mut_ptr().expose_provenance(),
        buffer.len(),
        0,
        0,
    );
    buffer
        .get(..length)
        .map(|frame| (frame, received_at as u64))
}

/// Queues a frame from the service's stack. A zero result asks the caller to retry.
pub fn transmit(frame: &[u8]) -> Option<usize> {
    write_from(SYS_NET_FRAME, FrameOperation::Transmit as usize, frame)
}
