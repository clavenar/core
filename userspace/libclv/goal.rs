use clv_abi::audit::{
    AUDIT_CAPACITY, DelegationRecord, DelegationRecordWithCaller, EncodedPublicationRecord,
    EncodedReadRecordWithLimit, EncodedRecordWithTool, EncodedRenameRecord, EncodedReplyRecord,
    ReadRecord, ReadRecordWithLimit, ReplyRecord,
};
pub use clv_abi::budget::ReplyBudget;
pub use clv_abi::goal::{
    CancelResult, CloseResult, CommitResult, CreateOptions, DiscardResult, FileRecipient, GoalDiff,
    GoalStatus, PublicationError, ReadLimits, RenameReview, ReviewToken, TimedRenameReview,
    TimedReviewToken,
};
use clv_abi::tool::ToolId;
use clv_abi::{
    MESSAGE_MAX_BYTES, Program, SYS_GOAL_AUDIT, SYS_GOAL_AUDIT_WITH_CALLER,
    SYS_GOAL_AUDIT_WITH_TOOL, SYS_GOAL_BUDGET, SYS_GOAL_CANCEL, SYS_GOAL_CLOSE, SYS_GOAL_COMMIT,
    SYS_GOAL_CREATE, SYS_GOAL_CREATE_PRESERVING_FILE, SYS_GOAL_CREATE_WITH_OPTIONS,
    SYS_GOAL_CREATE_WITH_REPLY_BUDGET, SYS_GOAL_DIFF, SYS_GOAL_DISCARD,
    SYS_GOAL_GRANT_DELEGABLE_FILE, SYS_GOAL_GRANT_FILE, SYS_GOAL_GRANT_FILE_FOR_RENAME,
    SYS_GOAL_GRANT_FILE_TO, SYS_GOAL_GRANT_FILE_TO_WITH_ARG, SYS_GOAL_PUBLICATION_AUDIT,
    SYS_GOAL_READ_AUDIT, SYS_GOAL_READ_AUDIT_WITH_LIMIT, SYS_GOAL_RENAME_AUDIT,
    SYS_GOAL_REPLY_AUDIT, SYS_GOAL_REVIEW, SYS_GOAL_REVIEW_WITH_DEADLINE, SYS_GOAL_REVOKE_FILE,
    SYS_GOAL_RUN, SYS_GOAL_RUN_WITH_REPLY_BUDGET, SYS_GOAL_STAGE_RENAME, SYS_GOAL_STATUS,
};

use super::{ecall, ecall6, syscall, syscall_result, with_stack_bytes};

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
    create_with_read_limits(
        options,
        ReadLimits {
            goal: read_limit,
            helpers: None,
        },
    )
}

/// Sets independent ceilings for all goal reads and for the subset made by delegated helpers.
pub fn create_with_read_limits(options: CreateOptions, limits: ReadLimits) -> Option<usize> {
    create_with_allowed_tool(options, limits, None)
}

/// Creates a goal with an optional exact tool revision for helper delegations.
pub fn create_with_allowed_tool(
    options: CreateOptions,
    limits: ReadLimits,
    allowed_tool: Option<ToolId>,
) -> Option<usize> {
    if options.child_limit == 1
        && options.allowed_helper.is_none()
        && allowed_tool.is_none()
        && limits.goal.is_none()
        && limits.helpers.is_none()
    {
        return options
            .reply_limit
            .map_or_else(create, create_with_reply_budget);
    }
    create_with_preserved_file(options, limits, allowed_tool, None)
}

/// A preserved descriptor binds the goal's rename to that file and its current bytes.
pub fn create_with_preserved_file(
    options: CreateOptions,
    limits: ReadLimits,
    allowed_tool: Option<ToolId>,
    preserve_file: Option<usize>,
) -> Option<usize> {
    let words = options.registers_with_allowed_tool(limits, allowed_tool)?;
    if let Some(fd) = preserve_file {
        let encoded = words.map(usize::to_le_bytes);
        let bytes = encoded.as_flattened();
        return syscall_result(syscall(
            SYS_GOAL_CREATE_PRESERVING_FILE,
            fd,
            bytes.as_ptr().expose_provenance(),
            bytes.len(),
        ));
    }
    let [children, flags, replies, helper, reads, helper_reads] = words;
    let (handle, ..) = ecall6(
        SYS_GOAL_CREATE_WITH_OPTIONS,
        children,
        flags,
        replies,
        helper,
        reads,
        helper_reads,
    );
    syscall_result(handle)
}

/// Attaches one file from the caller's handle table to a ready goal.
pub fn grant_file(goal: usize, fd: usize) -> bool {
    grant_file_with_recipient(goal, fd, None)
}

/// Gives the direct agent permission to stage a rename of this writable RamFS file.
pub fn grant_file_for_rename(goal: usize, fd: usize) -> bool {
    syscall(SYS_GOAL_GRANT_FILE_FOR_RENAME, goal, fd, 0) == 0
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

/// Attaches a file for delegation to one program with this exact launch argument.
pub fn grant_file_to_with_arg(goal: usize, fd: usize, program: Program, argument: &[u8]) -> bool {
    with_stack_bytes::<MESSAGE_MAX_BYTES>(argument, |address, length| {
        ecall(
            SYS_GOAL_GRANT_FILE_TO_WITH_ARG,
            goal,
            fd,
            program as usize,
            address,
            length,
        )
        .0
    }) == Some(0)
}

/// Closes all derived handles and discards any pending rename staged through the grant.
pub fn revoke_file(goal: usize) -> bool {
    syscall(SYS_GOAL_REVOKE_FILE, goal, 0, 0) == 0
}

/// Retains the file behind the owner's descriptor and stages a same-directory rename.
pub fn stage_rename(goal: usize, fd: usize, new_name: &[u8]) -> bool {
    super::fs::submit_rename(new_name, |address, length| {
        ecall(SYS_GOAL_STAGE_RENAME, goal, fd, address, length, 0).0
    })
}

/// Reads the pending change into the owner's stack buffer without consuming it.
pub fn diff(goal: usize, buffer: &mut [u8]) -> Option<GoalDiff<'_>> {
    read_diff(SYS_GOAL_DIFF, goal, buffer).map(|(diff, _, _)| diff)
}

/// A passing review of a completed goal returns a single-use commit token.
pub fn review(goal: usize, buffer: &mut [u8]) -> Option<(GoalDiff<'_>, Option<ReviewToken>)> {
    let (diff, token, _) = read_diff(SYS_GOAL_REVIEW, goal, buffer)?;
    Some((diff, ReviewToken::new(token)))
}

/// Returns the frozen diff alongside the kernel's decision on issuing a commit token.
pub fn review_with_status(goal: usize, buffer: &mut [u8]) -> Option<(GoalDiff<'_>, RenameReview)> {
    let (diff, token, status) = read_diff(SYS_GOAL_REVIEW, goal, buffer)?;
    Some((diff, RenameReview::from_registers(token, status)?))
}

/// Returns the same review with the absolute guest deadline of a passing token.
pub fn review_with_deadline(
    goal: usize,
    buffer: &mut [u8],
) -> Option<(GoalDiff<'_>, TimedRenameReview)> {
    let (diff, token, detail) = read_diff(SYS_GOAL_REVIEW_WITH_DEADLINE, goal, buffer)?;
    Some((diff, TimedRenameReview::from_registers(token, detail)?))
}

fn read_diff(
    number: usize,
    goal: usize,
    buffer: &mut [u8],
) -> Option<(GoalDiff<'_>, usize, usize)> {
    let (count, source_len, token, status) = ecall(
        number,
        goal,
        buffer.as_mut_ptr().expose_provenance(),
        buffer.len(),
        0,
        0,
    );
    let names = buffer.get(..count)?;
    let diff = match names.split_at_checked(source_len)? {
        ([], []) => GoalDiff::Empty,
        ([], _) | (_, []) => return None,
        (source, new_name) => GoalDiff::Rename { source, new_name },
    };
    Some((diff, token, status))
}

pub fn commit(goal: usize, token: ReviewToken) -> Option<CommitResult> {
    CommitResult::from_register(syscall(SYS_GOAL_COMMIT, goal, token.get(), 0))
}

/// Lets the owner discard the goal's pending rename and invalidate its review token.
pub fn discard(goal: usize) -> Option<DiscardResult> {
    match syscall(SYS_GOAL_DISCARD, goal, 0, 0) {
        n if n == DiscardResult::Discarded as usize => Some(DiscardResult::Discarded),
        n if n == DiscardResult::Empty as usize => Some(DiscardResult::Empty),
        _ => None,
    }
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

/// Borrows the extended wire records so each decision can be decoded in place.
pub fn audit_with_tool(
    goal: usize,
    records: &mut [EncodedRecordWithTool; AUDIT_CAPACITY],
) -> Option<(&[EncodedRecordWithTool], usize)> {
    let (rows, overwritten, _) = audit_rows(SYS_GOAL_AUDIT_WITH_TOOL, goal, records)?;
    Some((rows, overwritten))
}

/// Reads the goal members' reply-budget decisions in send-attempt order.
pub fn reply_audit(goal: usize) -> Option<AuditSnapshot<ReplyRecord>> {
    Some(snapshot(SYS_GOAL_REPLY_AUDIT, goal, ReplyRecord::decode)?.0)
}

pub fn reply_audit_rows(
    goal: usize,
    records: &mut [EncodedReplyRecord; AUDIT_CAPACITY],
) -> Option<(&[EncodedReplyRecord], usize)> {
    let (rows, overwritten, _) = audit_rows(SYS_GOAL_REPLY_AUDIT, goal, records)?;
    Some((rows, overwritten))
}

/// Reads shell and agent rename staging decisions in attempt order.
pub fn rename_audit_rows(
    goal: usize,
    records: &mut [EncodedRenameRecord; AUDIT_CAPACITY],
) -> Option<(&[EncodedRenameRecord], usize)> {
    let (rows, overwritten, _) = audit_rows(SYS_GOAL_RENAME_AUDIT, goal, records)?;
    Some((rows, overwritten))
}

/// Copies one row (index zero is oldest) and returns the retained and overwritten counts.
/// Index zero also succeeds for an empty history, leaving the buffer untouched.
pub fn publication_audit_row(
    goal: usize,
    index: usize,
    record: &mut EncodedPublicationRecord,
) -> Option<(usize, usize)> {
    let (count, overwritten, _, _) = ecall(
        SYS_GOAL_PUBLICATION_AUDIT,
        goal,
        record.as_mut_ptr().expose_provenance(),
        size_of_val(record),
        index,
        0,
    );
    (count <= AUDIT_CAPACITY).then_some((count, overwritten))
}

/// Returns the retained RamFS read decisions and the total bytes delivered over the goal's lifetime.
pub fn read_audit(goal: usize) -> Option<(AuditSnapshot<ReadRecord>, usize)> {
    snapshot(SYS_GOAL_READ_AUDIT, goal, ReadRecord::decode)
}

/// Includes the scope of each read's effective allowance in the same snapshot.
pub fn read_audit_with_limit(goal: usize) -> Option<(AuditSnapshot<ReadRecordWithLimit>, usize)> {
    snapshot(
        SYS_GOAL_READ_AUDIT_WITH_LIMIT,
        goal,
        ReadRecordWithLimit::decode,
    )
}

pub fn read_audit_rows_with_limit(
    goal: usize,
    records: &mut [EncodedReadRecordWithLimit; AUDIT_CAPACITY],
) -> Option<(&[EncodedReadRecordWithLimit], usize, usize)> {
    audit_rows(SYS_GOAL_READ_AUDIT_WITH_LIMIT, goal, records)
}

fn snapshot<Record: Copy, const WORDS: usize>(
    number: usize,
    goal: usize,
    decode: impl Fn(&[[u8; 8]; WORDS]) -> Option<Record>,
) -> Option<(AuditSnapshot<Record>, usize)> {
    let mut records = [[[0; 8]; WORDS]; AUDIT_CAPACITY];
    let (rows, overwritten, total) = audit_rows(number, goal, &mut records)?;
    let mut entries = [None; AUDIT_CAPACITY];
    for (entry, record) in entries.iter_mut().zip(rows) {
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

fn audit_rows<Record>(
    number: usize,
    goal: usize,
    records: &mut [Record; AUDIT_CAPACITY],
) -> Option<(&[Record], usize, usize)> {
    let (count, overwritten, total, _) = ecall(
        number,
        goal,
        records.as_mut_ptr().expose_provenance(),
        core::mem::size_of_val(records),
        0,
        0,
    );
    Some((records.get(..count)?, overwritten, total))
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
