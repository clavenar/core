use clv_abi::audit::{
    AUDIT_CAPACITY, DelegationRecord, DelegationRecordWithCaller, ReadRecord, ReplyRecord,
};
pub use clv_abi::budget::ReplyBudget;
pub use clv_abi::goal::{CancelResult, CloseResult, CreateOptions, FileRecipient, GoalStatus};
use clv_abi::{
    SYS_GOAL_AUDIT, SYS_GOAL_AUDIT_WITH_CALLER, SYS_GOAL_BUDGET, SYS_GOAL_CANCEL, SYS_GOAL_CLOSE,
    SYS_GOAL_CREATE, SYS_GOAL_CREATE_WITH_OPTIONS, SYS_GOAL_CREATE_WITH_REPLY_BUDGET,
    SYS_GOAL_GRANT_DELEGABLE_FILE, SYS_GOAL_GRANT_FILE, SYS_GOAL_GRANT_FILE_TO,
    SYS_GOAL_READ_AUDIT, SYS_GOAL_REPLY_AUDIT, SYS_GOAL_RUN, SYS_GOAL_RUN_WITH_REPLY_BUDGET,
    SYS_GOAL_STATUS,
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
    create_configured(CreateOptions {
        child_limit: children,
        reply_limit: reply_bytes,
        allowed_helper: None,
    })
}

/// Creates a goal with its child limit, reply budget and optional helper restriction.
pub fn create_configured(options: CreateOptions) -> Option<usize> {
    create_with_read_limit(options, None)
}

/// All members of the goal share the optional cumulative RamFS read limit.
pub fn create_with_read_limit(options: CreateOptions, read_limit: Option<usize>) -> Option<usize> {
    match (
        options.child_limit,
        options.reply_limit,
        options.allowed_helper,
        read_limit,
    ) {
        (1, None, None, None) => create(),
        (1, Some(bytes), None, None) => create_with_reply_budget(bytes),
        _ => {
            let (children, flags, replies, helper, reads) =
                options.registers_with_read_limit(read_limit);
            let (handle, ..) = ecall(
                SYS_GOAL_CREATE_WITH_OPTIONS,
                children,
                flags,
                replies,
                helper,
                reads,
            );
            syscall_result(handle)
        }
    }
}

/// Attaches one file from the caller's handle table to a ready goal.
pub fn grant_file(goal: usize, fd: usize) -> bool {
    grant_file_with_recipient(goal, fd, None)
}

/// Also lets the goal's agent pass read access to its helper.
pub fn grant_delegable_file(goal: usize, fd: usize) -> bool {
    grant_file_with_recipient(goal, fd, Some(FileRecipient::Any))
}

/// Attaches a file and sets which helpers the direct agent may pass it to.
pub fn grant_file_with_recipient(goal: usize, fd: usize, recipient: Option<FileRecipient>) -> bool {
    let (call, selector) = match recipient {
        None => (SYS_GOAL_GRANT_FILE, 0),
        Some(FileRecipient::Any) => (SYS_GOAL_GRANT_DELEGABLE_FILE, 0),
        Some(FileRecipient::Program(program)) => (SYS_GOAL_GRANT_FILE_TO, program as usize),
    };
    syscall(call, goal, fd, selector) == 0
}

pub fn budget(goal: usize) -> Option<ReplyBudget> {
    let (flags, available, reserved, spent) = ecall(SYS_GOAL_BUDGET, goal, 0, 0, 0, 0);
    ReplyBudget::from_registers(flags, available, reserved, spent)
}

pub struct AuditSnapshot<Record = DelegationRecord> {
    pub entries: [Option<Record>; AUDIT_CAPACITY],
    pub overwritten: usize,
}

/// Reads the retained decisions in launch-attempt order without consuming them.
pub fn audit(goal: usize) -> Option<AuditSnapshot> {
    Some(snapshot(SYS_GOAL_AUDIT, goal, DelegationRecord::decode)?.0)
}

/// Reads the same retained decisions together with the caller authenticated at delegation.
pub fn audit_with_caller(goal: usize) -> Option<AuditSnapshot<DelegationRecordWithCaller>> {
    Some(
        snapshot(
            SYS_GOAL_AUDIT_WITH_CALLER,
            goal,
            DelegationRecordWithCaller::decode,
        )?
        .0,
    )
}

/// Reads the goal members' reply-budget decisions in send-attempt order.
pub fn reply_audit(goal: usize) -> Option<AuditSnapshot<ReplyRecord>> {
    Some(snapshot(SYS_GOAL_REPLY_AUDIT, goal, ReplyRecord::decode)?.0)
}

/// Returns the retained RamFS read decisions and the total bytes delivered over the goal's lifetime.
pub fn read_audit(goal: usize) -> Option<(AuditSnapshot<ReadRecord>, usize)> {
    snapshot(SYS_GOAL_READ_AUDIT, goal, ReadRecord::decode)
}

fn snapshot<Record: Copy, const WORDS: usize>(
    number: usize,
    goal: usize,
    decode: impl Fn(&[[u8; 8]; WORDS]) -> Option<Record>,
) -> Option<(AuditSnapshot<Record>, usize)> {
    let mut records = [[[0; 8]; WORDS]; AUDIT_CAPACITY];
    let (count, overwritten, total, _) = ecall(
        number,
        goal,
        records.as_mut_ptr().expose_provenance(),
        core::mem::size_of_val(&records),
        0,
        0,
    );
    let mut entries = [None; AUDIT_CAPACITY];
    for (entry, record) in entries.iter_mut().zip(records.get(..count)?) {
        *entry = Some(decode(record)?);
    }
    Some((
        AuditSnapshot {
            entries,
            overwritten,
        },
        total,
    ))
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
