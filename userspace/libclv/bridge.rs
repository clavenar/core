pub use clv_abi::bridge::{
    BUDGET_EXHAUSTED, BridgeFailure, MAX_FRAME_BYTES, MODEL_UNAVAILABLE, SEARCH_GREETING, SKIP,
};
use clv_abi::{
    SYS_HOST_BRIDGE,
    bridge::{POLL, SUBMIT, SUBMIT_TOOL},
};

use super::{ecall, syscall_result};

/// Requests one plan for the active pinned agent launch.
pub fn plan_action() -> Result<u8, BridgeFailure> {
    let mut frame = [0; MAX_FRAME_BYTES];
    let (bytes, id) = exchange(SUBMIT, &mut frame)?;
    clv_abi::bridge::plan_action(bytes, id).ok_or(BridgeFailure::InvalidReply)
}

/// Returns the bounded text of the active pinned host tool call.
pub fn tool_text(frame: &mut [u8; MAX_FRAME_BYTES]) -> Result<&[u8], BridgeFailure> {
    let (bytes, id) = exchange(SUBMIT_TOOL, frame)?;
    clv_abi::bridge::tool_text(bytes, id)
}

fn exchange(
    submit: usize,
    frame: &mut [u8; MAX_FRAME_BYTES],
) -> Result<(&[u8], u64), BridgeFailure> {
    let id = call(submit, 0, 0)? as u64;
    loop {
        let count = call(POLL, frame.as_mut_ptr().expose_provenance(), frame.len())?;
        if count != 0 {
            let bytes = frame.get(..count).ok_or(BridgeFailure::InvalidReply)?;
            return Ok((bytes, id));
        }
        core::hint::spin_loop();
    }
}

fn call(operation: usize, address: usize, capacity: usize) -> Result<usize, BridgeFailure> {
    let (value, reason, ..) = ecall(SYS_HOST_BRIDGE, operation, address, capacity, 0, 0);
    syscall_result(value)
        .ok_or_else(|| BridgeFailure::from_raw(reason).unwrap_or(BridgeFailure::Unavailable))
}
