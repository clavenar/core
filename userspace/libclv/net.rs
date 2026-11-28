pub use clv_abi::net::MAX_FRAME_BYTES;

use clv_abi::SYS_NET_FRAME;
use clv_abi::net::{FrameOperation, NO_RECEIVE_DEADLINE};

use super::{ecall, write_from};

/// Waits for an Ethernet frame or an optional absolute guest monotonic deadline.
/// The result includes current guest monotonic nanoseconds. An empty slice
/// indicates an expired deadline or a retry when the kernel cannot park the caller.
pub fn receive(buffer: &mut [u8; MAX_FRAME_BYTES], deadline: Option<u64>) -> Option<(&[u8], u64)> {
    let (length, now, _, _) = ecall(
        SYS_NET_FRAME,
        FrameOperation::Receive as usize,
        buffer.as_mut_ptr().expose_provenance(),
        buffer.len(),
        deadline.unwrap_or(NO_RECEIVE_DEADLINE) as usize,
        0,
    );
    buffer.get(..length).map(|frame| (frame, now as u64))
}

/// Queues a frame from the service's stack. A zero result asks the caller to retry.
pub fn transmit(frame: &[u8]) -> Option<usize> {
    write_from(SYS_NET_FRAME, FrameOperation::Transmit as usize, frame)
}
