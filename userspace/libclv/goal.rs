pub use clv_abi::budget::ReplyBudget;
pub use clv_abi::goal::{CancelResult, CloseResult, GoalStatus};
use clv_abi::{
    SYS_GOAL_BUDGET, SYS_GOAL_CANCEL, SYS_GOAL_CLOSE, SYS_GOAL_CREATE,
    SYS_GOAL_CREATE_WITH_OPTIONS, SYS_GOAL_CREATE_WITH_REPLY_BUDGET, SYS_GOAL_GRANT_FILE,
    SYS_GOAL_RUN, SYS_GOAL_RUN_WITH_REPLY_BUDGET, SYS_GOAL_STATUS,
};

use super::{ecall, syscall, syscall_result};

pub fn create() -> Option<usize> {
    syscall_result(syscall(SYS_GOAL_CREATE, 0, 0, 0))
}

pub fn create_with_reply_budget(bytes: usize) -> Option<usize> {
    syscall_result(syscall(SYS_GOAL_CREATE_WITH_REPLY_BUDGET, bytes, 0, 0))
}

/// Creates a goal with room for the given number of launches and a shared reply budget.
pub fn create_with_options(children: usize, reply_bytes: Option<usize>) -> Option<usize> {
    match (children, reply_bytes) {
        (1, None) => create(),
        (1, Some(bytes)) => create_with_reply_budget(bytes),
        _ => syscall_result(syscall(
            SYS_GOAL_CREATE_WITH_OPTIONS,
            children,
            usize::from(reply_bytes.is_some()),
            reply_bytes.unwrap_or(0),
        )),
    }
}

/// Attaches one file from the caller's handle table to a ready goal.
pub fn grant_file(goal: usize, fd: usize) -> bool {
    syscall(SYS_GOAL_GRANT_FILE, goal, fd, 0) == 0
}

pub fn budget(goal: usize) -> Option<ReplyBudget> {
    let (flags, available, reserved, spent) = ecall(SYS_GOAL_BUDGET, goal, 0, 0, 0, 0);
    ReplyBudget::from_registers(flags, available, reserved, spent)
}

/// Starts one ordinary child and returns its launch handle.
pub fn run(goal: usize, program: usize) -> Option<usize> {
    syscall_result(syscall(SYS_GOAL_RUN, goal, program, 0))
}

pub fn run_with_reply_budget(goal: usize, program: usize, bytes: usize) -> Option<usize> {
    syscall_result(syscall(
        SYS_GOAL_RUN_WITH_REPLY_BUDGET,
        goal,
        program,
        bytes,
    ))
}

pub fn status(goal: usize) -> Option<GoalStatus> {
    let (state, child, value, _) = ecall(SYS_GOAL_STATUS, goal, 0, 0, 0, 0);
    GoalStatus::from_registers(state, child, value)
}

pub fn close(goal: usize) -> Option<CloseResult> {
    match syscall(SYS_GOAL_CLOSE, goal, 0, 0) {
        n if n == CloseResult::Closed as usize => Some(CloseResult::Closed),
        n if n == CloseResult::Busy as usize => Some(CloseResult::Busy),
        _ => None,
    }
}

/// Cancels a ready goal or stops all its live children before returning.
pub fn cancel(goal: usize) -> Option<CancelResult> {
    match syscall(SYS_GOAL_CANCEL, goal, 0, 0) {
        n if n == CancelResult::Cancelled as usize => Some(CancelResult::Cancelled),
        n if n == CancelResult::AlreadyFinished as usize => Some(CancelResult::AlreadyFinished),
        _ => None,
    }
}
