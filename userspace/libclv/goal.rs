use clv_abi::audit::{
    AUDIT_CAPACITY, DelegationRecord, DelegationRecordWithCaller, DelegationRecordWithTool,
    EncodedPublicationObservation, EncodedPublicationRecord, EncodedReadDecision,
    EncodedReadDenialSnapshot, EncodedReadRecordWithLimit, EncodedRecordWithTool,
    EncodedRenameRecord, EncodedReplyRecord, PublicationAuditView, PublicationObservation,
    PublicationReviewer, ReadAuditView, ReadDenialSnapshot, ReadRecord, ReadRecordWithLimit,
    ReplyRecord,
};
pub use clv_abi::budget::ReplyBudget;
pub use clv_abi::goal::{
    CancelOutcome, CancelResult, CloseResult, CommitOutcome, CommitResult, CreateOptions,
    CreatePolicy, DiscardResult, EncodedGoalObservation, FileOptions, FileRecipient, GoalDeadline,
    GoalMonitorKind, GoalObservation, GoalStatus, PublicationError, ReadLimits, RenameBatchDiff,
    RenameOptions, RenameReviewReceipt, ReviewToken, TimedRenameReview, TimedReviewToken,
};
use clv_abi::goal::{
    DISARM_READ_WATCH, GoalCreateRequest, TAKE_READ_WATCH_SUPPRESSED, WAIT_READ_WATCH_UPDATE,
    decode_rename_batch,
};
use clv_abi::tool::ToolId;
use clv_abi::{
    MESSAGE_MAX_BYTES, Program, SYS_GOAL_AUDIT, SYS_GOAL_BUDGET, SYS_GOAL_CANCEL, SYS_GOAL_CLOSE,
    SYS_GOAL_COMMIT, SYS_GOAL_CREATE, SYS_GOAL_DISCARD, SYS_GOAL_GRANT_DELEGABLE_FILE,
    SYS_GOAL_GRANT_FILE, SYS_GOAL_GRANT_FILE_FOR_RENAME, SYS_GOAL_GRANT_FILE_TO,
    SYS_GOAL_GRANT_FILE_TO_WITH_ARG, SYS_GOAL_LAUNCH_OBSERVER, SYS_GOAL_LAUNCH_RENAME_REVIEWER,
    SYS_GOAL_MONITOR, SYS_GOAL_OBSERVE, SYS_GOAL_PUBLICATION_DECISIONS_WITH_REVIEWER,
    SYS_GOAL_READ_AUDIT, SYS_GOAL_RENAME_AUDIT, SYS_GOAL_RENAME_DIFF, SYS_GOAL_REPLY_AUDIT,
    SYS_GOAL_REVIEW_BATCH_GRANTED, SYS_GOAL_REVOKE_FILE, SYS_GOAL_RUN,
    SYS_GOAL_RUN_WITH_REPLY_BUDGET, SYS_GOAL_STAGE_RENAME, SYS_GOAL_STATUS,
};

use super::{ecall, syscall, syscall_result, with_stack_bytes};

pub fn create() -> Option<usize> {
    create_with_options(1, None)
}

pub fn create_with_reply_budget(bytes: usize) -> Option<usize> {
    create_with_options(1, Some(bytes))
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
    create_with_rename_options(options, limits, allowed_tool, RenameOptions::default())
}

/// A preserved descriptor binds the goal's rename to that file and its current bytes.
pub fn create_with_preserved_file(
    options: CreateOptions,
    limits: ReadLimits,
    allowed_tool: Option<ToolId>,
    preserve_file: Option<usize>,
) -> Option<usize> {
    create_with_rename_options(
        options,
        limits,
        allowed_tool,
        RenameOptions {
            directory: None,
            preserve_files: [preserve_file, None],
        },
    )
}

/// Binds a rename directory and an optional preserved file for a single-rename goal.
pub fn create_with_rename_options(
    options: CreateOptions,
    limits: ReadLimits,
    allowed_tool: Option<ToolId>,
    rename: RenameOptions,
) -> Option<usize> {
    create_with_file_options(
        options,
        limits,
        allowed_tool,
        FileOptions {
            read_directory: None,
            rename,
        },
    )
}

/// Binds the owner's read and rename policies to the resolved objects at goal creation.
pub fn create_with_file_options(
    options: CreateOptions,
    limits: ReadLimits,
    allowed_tool: Option<ToolId>,
    files: FileOptions,
) -> Option<usize> {
    create_with_timeout(options, limits, allowed_tool, files, None)
}

/// An optional positive timeout in milliseconds applies to both execution and pending publication.
pub fn create_with_timeout(
    options: CreateOptions,
    limits: ReadLimits,
    allowed_tool: Option<ToolId>,
    files: FileOptions,
    timeout_ms: Option<usize>,
) -> Option<usize> {
    create_with_policy(
        CreatePolicy {
            options,
            read_limits: limits,
            allowed_tools: [allowed_tool, None],
            require_rename_review: false,
            rename_limit: 1,
            durable: false,
        },
        files,
        timeout_ms,
    )
}

/// Binds rename approval, resource limits and optional file scopes when the goal is created.
pub fn create_with_policy(
    policy: CreatePolicy,
    files: FileOptions,
    timeout_ms: Option<usize>,
) -> Option<usize> {
    let request = GoalCreateRequest {
        policy,
        files,
        timeout_ms,
    }
    .encode()?;
    let bytes = request.as_flattened();
    syscall_result(syscall(
        SYS_GOAL_CREATE,
        bytes.as_ptr().expose_provenance(),
        bytes.len(),
        0,
    ))
}

/// Attaches one file from the caller's handle table to a ready goal.
pub fn grant_file(goal: usize, fd: usize) -> bool {
    grant_file_with_recipient(goal, fd, None)
}

/// Gives the direct agent permission to stage a rename of this writable RamFS file.
/// On a two-rename goal, a second call adds another file in grant order.
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
    revoke_file_with_outcome(goal).is_some()
}

/// On success, reports whether this revocation discarded a grant-staged rename.
pub fn revoke_file_with_outcome(goal: usize) -> Option<bool> {
    match ecall(SYS_GOAL_REVOKE_FILE, goal, 0, 0, 0, 0) {
        (0, 0, ..) => Some(false),
        (0, 1, ..) => Some(true),
        _ => None,
    }
}

/// Retains the file behind the owner's descriptor and stages a same-directory rename.
pub fn stage_rename(goal: usize, fd: usize, new_name: &[u8]) -> bool {
    super::fs::submit_rename(new_name, |address, length| {
        ecall(SYS_GOAL_STAGE_RENAME, goal, fd, address, length, 0).0
    })
}

/// Copies the full staged diff. A requested review can approve the whole batch.
pub fn rename_diff(
    goal: usize,
    review: bool,
    buffer: &mut [u8],
) -> Option<(RenameBatchDiff<'_>, TimedRenameReview)> {
    let (length, entries, token, status) = ecall(
        SYS_GOAL_RENAME_DIFF,
        goal,
        buffer.as_mut_ptr().expose_provenance(),
        buffer.len(),
        usize::from(review),
        0,
    );
    Some((
        decode_rename_batch(buffer.get(..length)?, entries)?,
        TimedRenameReview::from_registers(token, status)?,
    ))
}

/// Gives one echo launch authority to review this completed goal's pending rename.
pub fn launch_rename_reviewer(goal: usize) -> Option<usize> {
    syscall_result(syscall(SYS_GOAL_LAUNCH_RENAME_REVIEWER, goal, 0, 0))
}

/// Delivers the whole batch and its verdict to the bound reviewer.
pub fn review_batch_granted(
    buffer: &mut [u8],
) -> Option<(RenameBatchDiff<'_>, Result<(), PublicationError>)> {
    let (length, entries, status, _) = ecall(
        SYS_GOAL_REVIEW_BATCH_GRANTED,
        buffer.as_mut_ptr().expose_provenance(),
        buffer.len(),
        0,
        0,
        0,
    );
    let changes = decode_rename_batch(buffer.get(..length)?, entries)?;
    let verdict = if status == 1 {
        Ok(())
    } else {
        Err(PublicationError::from_register(status)?)
    };
    Some((changes, verdict))
}

pub fn commit(goal: usize, token: ReviewToken) -> Option<CommitResult> {
    CommitResult::from_register(syscall(SYS_GOAL_COMMIT, goal, token.get(), 0))
}

pub fn commit_with_receipt(goal: usize, token: ReviewToken) -> Option<CommitOutcome> {
    let (result, _, generation, checksum) = ecall(SYS_GOAL_COMMIT, goal, token.get(), 0, 0, 0);
    CommitOutcome::from_registers(result, generation, checksum)
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
    let (records, _) = snapshot(SYS_GOAL_AUDIT, goal, 0, |record| {
        DelegationRecordWithTool::decode(record).map(|record| record.delegation.decision)
    })?;
    Some(records)
}

/// Reads the same retained decisions together with the caller authenticated at delegation.
pub fn audit_with_caller(goal: usize) -> Option<AuditSnapshot<DelegationRecordWithCaller>> {
    let (records, _) = snapshot(SYS_GOAL_AUDIT, goal, 0, |record| {
        DelegationRecordWithTool::decode(record).map(|record| record.delegation)
    })?;
    Some(records)
}

/// Reads the delegation history into the caller's buffer for decoding in place.
pub fn audit_with_tool(
    goal: usize,
    records: &mut [EncodedRecordWithTool; AUDIT_CAPACITY],
) -> Option<(&[EncodedRecordWithTool], usize)> {
    let (rows, overwritten, _) = audit_rows(SYS_GOAL_AUDIT, goal, 0, records)?;
    Some((rows, overwritten))
}

/// Reads the goal members' reply-budget decisions in send-attempt order.
pub fn reply_audit(goal: usize) -> Option<AuditSnapshot<ReplyRecord>> {
    Some(snapshot(SYS_GOAL_REPLY_AUDIT, goal, 0, ReplyRecord::decode)?.0)
}

pub fn reply_audit_rows(
    goal: usize,
    records: &mut [EncodedReplyRecord; AUDIT_CAPACITY],
) -> Option<(&[EncodedReplyRecord], usize)> {
    let (rows, overwritten, _) = audit_rows(SYS_GOAL_REPLY_AUDIT, goal, 0, records)?;
    Some((rows, overwritten))
}

/// Reads shell and agent rename staging decisions in attempt order.
pub fn rename_audit_rows(
    goal: usize,
    records: &mut [EncodedRenameRecord; AUDIT_CAPACITY],
) -> Option<(&[EncodedRenameRecord], usize)> {
    let (rows, overwritten, _) = audit_rows(SYS_GOAL_RENAME_AUDIT, goal, 0, records)?;
    Some((rows, overwritten))
}

/// Reads a commit or explicit discard (oldest first) and returns the retained and overwritten counts.
/// Index zero also succeeds for an empty history, leaving the buffer untouched.
pub fn publication_audit_row(
    goal: usize,
    index: usize,
    record: &mut EncodedPublicationRecord,
) -> Option<(usize, usize)> {
    read_publication_counts(goal, index, PublicationAuditView::Explicit, record)
}

/// Includes a rename discarded by cancellation among the last four publication decisions.
pub fn publication_audit_row_with_cancellation(
    goal: usize,
    index: usize,
    record: &mut EncodedPublicationRecord,
) -> Option<(usize, usize)> {
    read_publication_counts(goal, index, PublicationAuditView::WithCancellation, record)
}

/// Reads a publication decision, including a rename discarded by file grant revocation.
pub fn publication_decision_row(
    goal: usize,
    index: usize,
    record: &mut EncodedPublicationRecord,
) -> Option<(usize, usize)> {
    read_publication_counts(goal, index, PublicationAuditView::Decisions, record)
}

/// Includes the terminal discard of a rename when a timed goal expires.
pub fn publication_decision_row_with_expiry(
    goal: usize,
    index: usize,
    record: &mut EncodedPublicationRecord,
) -> Option<(usize, usize)> {
    read_publication_counts(goal, index, PublicationAuditView::WithExpiry, record)
}

/// Reads an indexed publication decision and the reviewer whose approval it used.
pub fn publication_decision_row_with_reviewer(
    goal: usize,
    index: usize,
    include_expiry: bool,
    record: &mut EncodedPublicationRecord,
) -> Option<(usize, usize, Option<PublicationReviewer>)> {
    let view = if include_expiry {
        PublicationAuditView::WithExpiry
    } else {
        PublicationAuditView::Decisions
    };
    read_publication_row(goal, index, view, record)
}

fn read_publication_counts(
    goal: usize,
    index: usize,
    view: PublicationAuditView,
    record: &mut EncodedPublicationRecord,
) -> Option<(usize, usize)> {
    let (count, overwritten, _) = read_publication_row(goal, index, view, record)?;
    Some((count, overwritten))
}

fn read_publication_row(
    goal: usize,
    index: usize,
    view: PublicationAuditView,
    record: &mut EncodedPublicationRecord,
) -> Option<(usize, usize, Option<PublicationReviewer>)> {
    let (count, overwritten, reviewer, audited_goal) = ecall(
        SYS_GOAL_PUBLICATION_DECISIONS_WITH_REVIEWER,
        goal,
        record.as_mut_ptr().expose_provenance(),
        size_of_val(record),
        index,
        view as usize,
    );
    (count <= AUDIT_CAPACITY && audited_goal == goal).then(|| {
        (
            count,
            overwritten,
            PublicationReviewer::from_register(reviewer),
        )
    })
}

/// Returns the retained RamFS budget decisions and the total bytes delivered over the goal's lifetime.
pub fn read_audit(goal: usize) -> Option<(AuditSnapshot<ReadRecord>, usize)> {
    snapshot(
        SYS_GOAL_READ_AUDIT,
        goal,
        ReadAuditView::Budget as usize,
        |record| ReadRecordWithLimit::decode(record).map(|entry| entry.read),
    )
}

/// Includes the scope of each read's effective allowance in the same snapshot.
pub fn read_audit_with_limit(goal: usize) -> Option<(AuditSnapshot<ReadRecordWithLimit>, usize)> {
    snapshot(
        SYS_GOAL_READ_AUDIT,
        goal,
        ReadAuditView::Budget as usize,
        ReadRecordWithLimit::decode,
    )
}

pub fn read_audit_rows_with_limit(
    goal: usize,
    records: &mut [EncodedReadRecordWithLimit; AUDIT_CAPACITY],
) -> Option<(&[EncodedReadRecordWithLimit], usize, usize)> {
    audit_rows(
        SYS_GOAL_READ_AUDIT,
        goal,
        ReadAuditView::Budget as usize,
        records,
    )
}

/// Includes directory-scope denials alongside byte-budget decisions in read-attempt order.
pub fn read_decision_rows(
    goal: usize,
    records: &mut [EncodedReadDecision; AUDIT_CAPACITY],
) -> Option<(&[EncodedReadDecision], usize, usize)> {
    audit_rows(
        SYS_GOAL_READ_AUDIT,
        goal,
        ReadAuditView::Decisions as usize,
        records,
    )
}

fn snapshot<Record: Copy, const WORDS: usize>(
    number: usize,
    goal: usize,
    view: usize,
    decode: impl Fn(&[[u8; 8]; WORDS]) -> Option<Record>,
) -> Option<(AuditSnapshot<Record>, usize)> {
    let mut records = [[[0; 8]; WORDS]; AUDIT_CAPACITY];
    let (rows, overwritten, total) = audit_rows(number, goal, view, &mut records)?;
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
    view: usize,
    records: &mut [Record; AUDIT_CAPACITY],
) -> Option<(&[Record], usize, usize)> {
    let (count, overwritten, total, _) = ecall(
        number,
        goal,
        records.as_mut_ptr().expose_provenance(),
        core::mem::size_of_val(records),
        view,
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
    status_with_deadline(goal).map(|(status, _)| status)
}

/// Starts a shell-owned child with a one-use observation grant for this open goal.
pub fn launch_observer(goal: usize, program: Program) -> Option<usize> {
    syscall_result(syscall(SYS_GOAL_LAUNCH_OBSERVER, goal, program as usize, 0))
}

/// Starts an isolated echo monitor for one terminal snapshot of this goal.
pub fn launch_finish_monitor(goal: usize) -> Option<usize> {
    launch_monitor(goal, GoalMonitorKind::Finish)
}

/// Starts an isolated echo monitor for the disposition of a rename transaction.
pub fn launch_publication_monitor(goal: usize) -> Option<usize> {
    launch_monitor(goal, GoalMonitorKind::Publication)
}

/// Arms the monitor for out-of-scope reads by goal members.
pub fn launch_read_denial_watch(goal: usize) -> Option<usize> {
    launch_monitor(goal, GoalMonitorKind::ReadDenial)
}

pub fn launch_read_denial_watch_after(goal: usize, threshold: usize) -> Option<usize> {
    if threshold == 1 {
        return launch_read_denial_watch(goal);
    }
    syscall_result(
        ecall(
            SYS_GOAL_MONITOR,
            goal,
            GoalMonitorKind::ReadDenial as usize,
            0,
            threshold,
            0,
        )
        .0,
    )
}

fn launch_monitor(goal: usize, kind: GoalMonitorKind) -> Option<usize> {
    syscall_result(syscall(SYS_GOAL_MONITOR, goal, kind as usize, 0))
}

pub fn disarm_read_denial_watch(handle: usize) -> bool {
    syscall(SYS_GOAL_MONITOR, handle, 0, DISARM_READ_WATCH) == 0
}

/// Ends the watch after collecting both the alert and the monitor exit status.
pub fn take_read_denial_suppressed(handle: usize) -> Option<usize> {
    syscall_result(syscall(
        SYS_GOAL_MONITOR,
        handle,
        0,
        TAKE_READ_WATCH_SUPPRESSED,
    ))
}

/// Waits for an authenticated count newer than the monitor's last sample.
pub fn wait_read_denial_snapshot() -> Option<ReadDenialSnapshot> {
    let mut bytes = [0; size_of::<EncodedReadDenialSnapshot>()];
    let length = syscall(
        SYS_GOAL_MONITOR,
        bytes.as_mut_ptr().expose_provenance(),
        bytes.len(),
        WAIT_READ_WATCH_UPDATE,
    );
    ReadDenialSnapshot::decode(bytes.get(..length)?)
}

/// Takes one snapshot and queues the same kernel-generated record for the parent.
pub fn observe() -> Option<GoalObservation> {
    let mut bytes = [0; size_of::<EncodedGoalObservation>()];
    GoalObservation::decode(observe_record(&mut bytes)?)
}

/// Collects the publication snapshot and queues the same record for the parent.
pub fn observe_publication() -> Option<PublicationObservation> {
    let mut bytes = [0; size_of::<EncodedPublicationObservation>()];
    PublicationObservation::decode(observe_record(&mut bytes)?)
}

fn observe_record(bytes: &mut [u8]) -> Option<&[u8]> {
    let count = syscall(
        SYS_GOAL_OBSERVE,
        bytes.as_mut_ptr().expose_provenance(),
        bytes.len(),
        0,
    );
    bytes.get(..count)
}

/// Reads member completion and the independent goal deadline in a single syscall.
pub fn status_with_deadline(goal: usize) -> Option<(GoalStatus, GoalDeadline)> {
    let (state, child, value, deadline) = ecall(SYS_GOAL_STATUS, goal, 0, 0, 0, 0);
    Some((
        GoalStatus::from_registers(state, child, value)?,
        GoalDeadline::from_register(deadline),
    ))
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
    cancel_with_outcome(goal).map(|outcome| outcome.execution)
}

/// Reports the execution status and whether this cancellation removed a staged rename.
pub fn cancel_with_outcome(goal: usize) -> Option<CancelOutcome> {
    let (execution, rename_discarded, ..) = ecall(SYS_GOAL_CANCEL, goal, 0, 0, 0, 0);
    CancelOutcome::from_registers(execution, rename_discarded)
}
