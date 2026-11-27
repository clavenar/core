pub use clv_abi::bridge::{
    BUDGET_EXHAUSTED, MODEL_UNAVAILABLE, PlanFailure, SEARCH_GREETING, SKIP,
};
use clv_abi::{
    SYS_HOST_BRIDGE,
    bridge::{MAX_FRAME_BYTES, POLL_PLAN, SUBMIT_PLAN},
};

use super::{ecall, syscall_result};

/// Requests one plan for the active pinned agent launch.
pub fn plan_action() -> Result<u8, PlanFailure> {
    let id = call(SUBMIT_PLAN, 0, 0)? as u64;
    let mut frame = [0; MAX_FRAME_BYTES];
    loop {
        let count = call(
            POLL_PLAN,
            frame.as_mut_ptr().expose_provenance(),
            frame.len(),
        )?;
        if count != 0 {
            return frame
                .get(..count)
                .and_then(|bytes| clv_abi::bridge::plan_action(bytes, id))
                .ok_or(PlanFailure::InvalidReply);
        }
        core::hint::spin_loop();
    }
}

fn call(operation: usize, address: usize, capacity: usize) -> Result<usize, PlanFailure> {
    let (value, reason, ..) = ecall(SYS_HOST_BRIDGE, operation, address, capacity, 0, 0);
    syscall_result(value)
        .ok_or_else(|| PlanFailure::from_raw(reason).unwrap_or(PlanFailure::Unavailable))
}
