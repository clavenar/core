pub use clv_abi::net::{
    CRYPTO_SEED_BYTES, HTTPS_PROBE_BODY, HttpsRequest, HttpsStatus, MAX_FRAME_BYTES, crypto_seed,
    unix_nanos,
};

use clv_abi::SYS_NET_FRAME;
use clv_abi::net::{
    COMPLETE_HTTPS_PROBE, FrameOperation, NO_RECEIVE_DEADLINE, POLL_HTTPS_PROBE, READ_UNIX_TIME,
    SUBMIT_HTTPS_PROBE, TAKE_HTTPS_PROBE,
};

use super::{ecall, syscall, syscall_result};

/// Waits for an Ethernet frame or an optional absolute guest monotonic deadline.
/// The result includes current guest monotonic nanoseconds. An empty slice
/// indicates an expired deadline or a retry when the kernel cannot park the caller.
pub fn receive(
    buffer: &mut [u8; MAX_FRAME_BYTES],
    deadline: Option<u64>,
    request: usize,
) -> Option<(&[u8], u64)> {
    let (length, now, _, _) = ecall(
        SYS_NET_FRAME,
        FrameOperation::Receive as usize,
        buffer.as_mut_ptr().expose_provenance(),
        buffer.len(),
        deadline.unwrap_or(NO_RECEIVE_DEADLINE) as usize,
        request,
    );
    buffer.get(..length).map(|frame| (frame, now as u64))
}

/// Queues a frame from the service's stack. A zero result asks the caller to retry.
pub fn transmit(frame: &[u8], request: usize) -> Option<usize> {
    let (sent, ..) = ecall(
        SYS_NET_FRAME,
        FrameOperation::Transmit as usize,
        frame.as_ptr().expose_provenance(),
        frame.len(),
        0,
        request,
    );
    syscall_result(sent)
}

/// Reads the current Unix seconds for certificate verification.
pub fn unix_time() -> Option<u64> {
    syscall_result(syscall(SYS_NET_FRAME, READ_UNIX_TIME, 0, 0)).map(|time| time as u64)
}

/// Submit the goal's fixed HTTPS operation and collect its result.
pub fn request_https(kind: HttpsRequest) -> HttpsStatus {
    let (request, status, ..) = ecall(SYS_NET_FRAME, SUBMIT_HTTPS_PROBE, kind as usize, 0, 0, 0);
    let Some(request) = syscall_result(request) else {
        return HttpsStatus::from_raw(status).unwrap_or(HttpsStatus::Unavailable);
    };
    // Keep the agent runnable so the shell can wait while netd is parked on the NIC.
    loop {
        let result = syscall(SYS_NET_FRAME, POLL_HTTPS_PROBE, request, 0);
        if result != 0 {
            return HttpsStatus::from_raw(result).unwrap_or(HttpsStatus::Unavailable);
        }
    }
}

pub struct HttpsProbe {
    pub request: usize,
    pub kind: HttpsRequest,
    pub ipv4: [u8; 4],
    pub deadline: u64,
}

/// Claim a goal-bound HTTPS operation as the dedicated network service.
pub fn take_https_probe() -> Option<HttpsProbe> {
    let (request, ipv4, deadline, kind) = ecall(SYS_NET_FRAME, TAKE_HTTPS_PROBE, 0, 0, 0, 0);
    Some(HttpsProbe {
        request: syscall_result(request).filter(|&request| request != 0)?,
        kind: HttpsRequest::from_raw(kind)?,
        ipv4: (ipv4 as u32).to_le_bytes(),
        deadline: deadline as u64,
    })
}

pub fn complete_https_probe(request: usize, status: HttpsStatus) -> bool {
    syscall(
        SYS_NET_FRAME,
        COMPLETE_HTTPS_PROBE,
        request,
        status as usize,
    ) == 0
}
