pub use clv_abi::goal::{CloseResult, GoalStatus};
use clv_abi::{SYS_GOAL_CLOSE, SYS_GOAL_CREATE, SYS_GOAL_RUN, SYS_GOAL_STATUS};

use super::{ecall, syscall, syscall_result};

pub fn create() -> Option<usize> {
    syscall_result(syscall(SYS_GOAL_CREATE, 0, 0, 0))
}

/// Starts one ordinary child and returns its launch handle.
pub fn run(goal: usize, program: usize) -> Option<usize> {
    syscall_result(syscall(SYS_GOAL_RUN, goal, program, 0))
}

pub fn status(goal: usize) -> Option<GoalStatus> {
    let (state, child, value) = ecall(SYS_GOAL_STATUS, goal, 0, 0, 0, 0);
    GoalStatus::from_registers(state, child, value)
}

pub fn close(goal: usize) -> Option<CloseResult> {
    match syscall(SYS_GOAL_CLOSE, goal, 0, 0) {
        n if n == CloseResult::Closed as usize => Some(CloseResult::Closed),
        n if n == CloseResult::Busy as usize => Some(CloseResult::Busy),
        _ => None,
    }
}
