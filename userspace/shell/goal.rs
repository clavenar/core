mod audit;
mod options;

use super::color;
use super::command::{GoalFlag, Subcommand};
use clv::goal::{
    self, CancelResult, CloseResult, CommitResult, DiscardResult, FileRecipient, GoalDeadline,
    GoalObservation, GoalStatus, PublicationError, RenameReviewReceipt, ReplyBudget, ReviewToken,
    TimedRenameReview,
};
use clv::{agent, print};
use clv_abi::agent::{NO_DEFINITION, SHA256_BYTES};
use clv_abi::audit::{PublicationObservation, ReadDenialEvent};
use clv_abi::{CompletionKind, OPEN_READ, Program};
pub(super) use options::{GoalCommand, GoalOptions};

use super::{
    ConsoleLine, HEX_DIGITS,
    children::Children,
    files::OpenHandle,
    help::{self, Entry},
    hex, print_handle, print_line, split_literal, split_word,
};

pub(super) struct CurrentGoal {
    handle: usize,
    agent: Option<usize>,
    review: Option<ReviewToken>,
    require_rename_review: bool,
}

const SUPERVISOR_REVIEW_REQUIRED: &[u8] =
    b"supervisor review required (use goal diff --via echo)\r\n";

fn with_current(current: &mut Option<CurrentGoal>, action: impl FnOnce(&mut CurrentGoal)) {
    match current {
        Some(goal) => action(goal),
        None => print(b"no current goal\r\n"),
    }
}

pub(super) fn command(
    arguments: &[u8],
    current: &mut Option<CurrentGoal>,
    children: &mut Children,
) {
    let handle = current.as_ref().map(|goal| goal.handle);
    let (name, args) = split_word(arguments);
    match (Subcommand::parse(name), args) {
        (Subcommand::Create, arguments) => create(arguments, current),
        (Subcommand::GrantFile, arguments) => grant_file(handle, arguments),
        (Subcommand::StageRename, arguments) => stage_rename(handle, arguments),
        (Subcommand::Diff, arguments) => {
            let reviewer = match split_word(arguments) {
                (b"", b"") => None,
                (b"--via", b"echo") => Some(children),
                _ => return Entry::GoalDiff.usage(),
            };
            with_current(current, |goal| show_diff(goal, reviewer));
        }
        (Subcommand::Commit, b"") => with_current(current, commit),
        (Subcommand::Discard, b"") => with_current(current, discard),
        (Subcommand::RevokeFile, b"") => with_current(current, revoke_file),
        (Subcommand::Cancel, b"") => with_current(current, cancel),
        (Subcommand::Run, arguments) => children.run_in_goal(handle, arguments),
        (Subcommand::Observe, arguments) => children.observe_goal(handle, arguments),
        (Subcommand::Audit, option) => audit::show(handle, option),
        (Subcommand::Budget, b"") => match handle.and_then(goal::budget) {
            Some(budget) => print_budget(budget),
            None => print(b"no current goal\r\n"),
        },
        (Subcommand::Status, b"") => match handle.and_then(goal::status_with_deadline) {
            Some((status, deadline)) => {
                print_status(status);
                if deadline != GoalDeadline::Unlimited {
                    print_deadline(deadline);
                }
            }
            None => print(b"no current goal\r\n"),
        },
        (Subcommand::Close, b"") => match handle.and_then(goal::close) {
            Some(CloseResult::Closed) => {
                *current = None;
                print(b"goal closed\r\n");
            }
            Some(CloseResult::Busy) => print(b"goal busy\r\n"),
            None => print(b"no current goal\r\n"),
        },
        _ => {
            print(b"usage: goal <subcommand>\r\n");
            help::goals();
        }
    }
}

enum FileTarget<'a> {
    Recipient(Option<FileRecipient>),
    WithArgument(Program, &'a [u8]),
    StageRename,
}

fn grant_options(arguments: &[u8]) -> Option<(FileTarget<'_>, &[u8])> {
    let (word, rest) = split_word(arguments);
    let (target, path) = match GoalFlag::parse(word) {
        GoalFlag::StageRename => {
            let (path, b"") = split_literal(rest)? else {
                return None;
            };
            (FileTarget::StageRename, path)
        }
        GoalFlag::Delegable => (FileTarget::Recipient(Some(FileRecipient::Any)), rest),
        GoalFlag::DelegateTo => {
            let (name, path) = split_word(rest);
            let program = Program::named(name)?;
            let (option, rest) = split_word(path);
            if matches!(GoalFlag::parse(option), GoalFlag::Argument) {
                let (argument, path) = split_literal(rest)?;
                (FileTarget::WithArgument(program, argument), path)
            } else {
                (
                    FileTarget::Recipient(Some(FileRecipient::Program(program))),
                    path,
                )
            }
        }
        _ => (FileTarget::Recipient(None), arguments),
    };
    (!path.is_empty() && !matches!(GoalFlag::parse(split_word(path).0), GoalFlag::Argument))
        .then_some((target, path))
}

fn grant_file(goal: Option<usize>, arguments: &[u8]) {
    let Some((target, path)) = grant_options(arguments) else {
        return Entry::GrantFile.usage();
    };
    let granted = with_goal_file(goal, path, |goal, fd| match target {
        FileTarget::Recipient(recipient) => goal::grant_file_with_recipient(goal, fd, recipient),
        FileTarget::StageRename => goal::grant_file_for_rename(goal, fd),
        FileTarget::WithArgument(program, literal) => {
            goal::grant_file_to_with_arg(goal, fd, program, literal)
        }
    });
    if let Some(granted) = granted {
        print(if granted {
            b"file granted\r\n"
        } else {
            b"cannot grant file\r\n"
        });
    }
}

fn with_goal_file(
    goal: Option<usize>,
    path: &[u8],
    operation: impl FnOnce(usize, usize) -> bool,
) -> Option<bool> {
    let Some(goal) = goal else {
        print(b"no current goal\r\n");
        return None;
    };
    let Some(file) = OpenHandle::open(path, OPEN_READ) else {
        print(b"cannot open path\r\n");
        return None;
    };
    Some(operation(goal, file.id()))
}

fn revoke_file(goal: &mut CurrentGoal) {
    let Some(rename_discarded) = goal::revoke_file_with_outcome(goal.handle) else {
        return print(b"cannot revoke file\r\n");
    };
    print(b"file revoked\r\n");
    if rename_discarded {
        goal.review = None;
        print(b"staged rename discarded\r\n");
    }
}

fn stage_rename(goal: Option<usize>, arguments: &[u8]) {
    let Some((path, rest)) = split_literal(arguments) else {
        return Entry::StageRename.usage();
    };
    let Some((name, b"")) = split_literal(rest) else {
        return Entry::StageRename.usage();
    };
    if let Some(staged) = with_goal_file(goal, path, |goal, fd| goal::stage_rename(goal, fd, name))
    {
        print(if staged {
            b"rename staged\r\n"
        } else {
            b"cannot stage rename\r\n"
        });
    }
}

fn show_diff(goal: &mut CurrentGoal, reviewer: Option<&mut Children>) {
    let mut buffer = [0; clv_abi::goal::RENAME_BATCH_DIFF_MAX_BYTES];
    if !goal.require_rename_review {
        goal.review = None;
    }
    let Some((changes, mut review)) =
        goal::rename_diff(goal.handle, reviewer.is_none(), &mut buffer)
    else {
        return print(b"cannot read goal diff\r\n");
    };
    if changes[0].is_none() {
        goal.review = None;
        return print(b"no staged changes\r\n");
    }
    for change in changes.into_iter().flatten() {
        print_line(&[b"rename ", change.source, b" -> ", change.new_name]);
    }
    if let Some(children) = reviewer {
        let Some(receipt) = children.review_rename(goal.handle) else {
            return print(b"cannot review rename via echo\r\n");
        };
        print_reviewer(&receipt);
        review = receipt.review;
    }
    goal.review = match review {
        TimedRenameReview::Ready(approved) => Some(approved.token),
        TimedRenameReview::SupervisorRequired if goal.review.is_some() => {
            return print(b"supervisor review required (previous receipt retained)\r\n");
        }
        _ => None,
    };
    print_review(review);
}

pub(super) fn print_reviewer(receipt: &RenameReviewReceipt) {
    print_line(&[
        b"reviewed by supervisor=",
        &hex(receipt.reviewer),
        b" goal=",
        &hex(receipt.goal),
        b" base=",
        decimal(receipt.base_revision as usize, &mut [0; DIGITS]),
    ]);
}

pub(super) fn print_review(review: TimedRenameReview) {
    match review {
        TimedRenameReview::Preview => print(b"preview only\r\n"),
        TimedRenameReview::SupervisorRequired => print(SUPERVISOR_REVIEW_REQUIRED),
        TimedRenameReview::Ready(review) => print_review_deadline(review.deadline_ns),
        TimedRenameReview::Blocked(reason) => print(match reason {
            PublicationError::Conflict => b"conflict\r\n",
            PublicationError::PostconditionFailed => b"postcondition failed\r\n",
            PublicationError::Unavailable => b"review unavailable\r\n",
        }),
    }
}

fn print_review_deadline(deadline_ns: u64) {
    let deadline = core::time::Duration::from_nanos(deadline_ns);
    // decimal writes from the right. Keep all nine fractional digits.
    let mut fraction = [b'0'; DIGITS];
    decimal(deadline.subsec_nanos() as usize, &mut fraction);
    print_line(&[
        b"ready to commit (guest deadline ",
        decimal(deadline.as_secs() as usize, &mut [0; DIGITS]),
        b".",
        &fraction[DIGITS - 9..],
        b" s)",
    ]);
}

fn commit(goal: &mut CurrentGoal) {
    let token = goal.review.take();
    let outcome = token.and_then(|token| goal::commit_with_receipt(goal.handle, token));
    match outcome.map(|outcome| outcome.result) {
        Some(CommitResult::Committed) => color::print_line(color::GREEN, b"rename committed"),
        Some(CommitResult::Conflict) => color::print_line(color::RED, b"commit conflict"),
        Some(CommitResult::ReviewExpired) => color::print_line(color::YELLOW, b"review expired"),
        Some(CommitResult::PostconditionFailed) => print(b"postcondition failed\r\n"),
        Some(CommitResult::StorageFailed) => {
            color::print_line(color::RED, b"durable commit storage failed")
        }
        Some(CommitResult::StorageOutcomeUnknown) => {
            color::print_line(color::YELLOW, b"durable commit outcome unknown")
        }
        None => {
            if matches!(
                goal::status_with_deadline(goal.handle),
                Some((_, GoalDeadline::Expired))
            ) {
                color::print_line(color::YELLOW, b"goal deadline expired");
            } else if token.is_none() && goal.require_rename_review {
                print(SUPERVISOR_REVIEW_REQUIRED);
            } else if token.is_none() {
                print(b"goal commit requires a passing goal diff after completion\r\n");
            } else {
                print(b"cannot commit goal\r\n");
            }
        }
    }
    if let Some(receipt) = outcome.and_then(|outcome| outcome.receipt) {
        print_line(&[
            b"durable commit generation=",
            &hex(receipt.generation.get() as usize),
            b" checksum=",
            &hex(receipt.checksum as usize),
        ]);
    }
}

fn discard(goal: &mut CurrentGoal) {
    let Some(result) = goal::discard(goal.handle) else {
        return print(b"cannot discard goal changes\r\n");
    };
    goal.review = None;
    print(match result {
        DiscardResult::Discarded => b"rename discarded\r\n",
        DiscardResult::Empty => b"no staged changes\r\n",
    });
}

fn cancel(goal: &mut CurrentGoal) {
    let Some(outcome) = goal::cancel_with_outcome(goal.handle) else {
        return print(b"no current goal\r\n");
    };
    goal.review = None;
    print(match outcome.execution {
        CancelResult::Cancelled => b"goal cancelled\r\n",
        CancelResult::AlreadyFinished => b"goal already finished\r\n",
    });
    if outcome.rename_discarded {
        print(b"staged rename discarded\r\n");
    }
}

fn create(arguments: &[u8], current: &mut Option<CurrentGoal>) {
    let Some((options, b"")) = GoalOptions::parse(arguments, GoalCommand::Create) else {
        return Entry::GoalCreate.usage();
    };
    match create_goal(&options) {
        Some(handle) => {
            *current = Some(CurrentGoal {
                handle,
                agent: None,
                review: None,
                require_rename_review: options.require_rename_review || options.durable,
            });
            print_handle(b"goal created ", handle);
        }
        None if current.is_some() => print(b"goal already open\r\n"),
        None if options.durable => print(b"cannot create durable goal\r\n"),
        None => print(b"cannot create goal\r\n"),
    }
}

fn create_goal(options: &GoalOptions<'_>) -> Option<usize> {
    let policy = goal::CreatePolicy {
        options: goal::CreateOptions {
            child_limit: options.children.unwrap_or(1),
            reply_limit: options.reply_bytes,
            allowed_helper: options.allowed_helper,
        },
        read_limits: options.read_limits,
        allowed_tools: options.allowed_tools,
        require_rename_review: options.require_rename_review,
        rename_limit: options.rename_limit.unwrap_or(1),
        durable: options.durable,
        allow_https_probe: options.allow_https_probe,
        allow_mcp_init: options.allow_mcp_init,
        allow_mcp_list: options.allow_mcp_list,
        allow_mcp_call: options.allow_mcp_call,
    };
    // Keep the policy descriptors open through goal creation.
    let read_directory = open_policy_handle(options.read_under)?;
    let rename_directory = open_policy_handle(options.rename_under)?;
    let [first_file, second_file] = options.preserve_files;
    let preserve_files = [
        open_policy_handle(first_file)?,
        open_policy_handle(second_file)?,
    ];
    goal::create_with_policy(
        policy,
        goal::FileOptions {
            read_directory: read_directory.as_ref().map(OpenHandle::id),
            rename: goal::RenameOptions {
                directory: rename_directory.as_ref().map(OpenHandle::id),
                preserve_files: preserve_files
                    .each_ref()
                    .map(|file| file.as_ref().map(OpenHandle::id)),
            },
        },
        options.timeout_ms,
    )
}

// An omitted path leaves its policy unset. An unreadable supplied path fails goal creation.
fn open_policy_handle(path: Option<&[u8]>) -> Option<Option<OpenHandle>> {
    match path {
        Some(path) => OpenHandle::open(path, OPEN_READ).map(Some),
        None => Some(None),
    }
}

const DIGITS: usize = usize::MAX.ilog10() as usize + 1;

fn decimal(mut value: usize, buf: &mut [u8; DIGITS]) -> &[u8] {
    let mut start = buf.len();
    loop {
        start -= 1;
        buf[start] = b'0' + (value % 10) as u8;
        value /= 10;
        if value == 0 {
            return &buf[start..];
        }
    }
}

pub(super) fn recv_checkpoint() {
    let mut buffer = [0; clv_abi::MESSAGE_MAX_BYTES];
    let Some(reply) = agent::receive_checkpoint_reply(&mut buffer) else {
        return print(b"no completed checkpoint reply\r\n");
    };
    super::children::display_reply(0, reply.kind, reply.bytes);
    let mut calls_buffer = [0; DIGITS];
    let (host_label, calls): (&[u8], &[u8]) = reply.host_calls.map_or((&[], &[]), |calls| {
        (b" host_calls=", decimal(calls, &mut calls_buffer))
    });
    print_line(&[
        b"read_bytes=",
        decimal(reply.read_spent, &mut [0; DIGITS]),
        b" reply_bytes=",
        decimal(reply.reply_spent, &mut [0; DIGITS]),
        host_label,
        calls,
    ]);
}

fn amount(value: Option<usize>, buf: &mut [u8; DIGITS]) -> &[u8] {
    match value {
        Some(value) => decimal(value, buf),
        None => b"unlimited",
    }
}

fn print_budget(budget: ReplyBudget) {
    let mut line = ConsoleLine::new();
    write_budget(&mut line, budget);
    line.finish(&[]);
}

fn write_budget(line: &mut ConsoleLine, budget: ReplyBudget) {
    let mut digits = [0; DIGITS];
    line.write(b"available=");
    line.write(amount(budget.available, &mut digits));
    line.write(b" reserved=");
    line.write(amount(budget.reserved, &mut digits));
    line.write(b" spent=");
    line.write(decimal(budget.spent, &mut digits));
}

fn print_deadline(deadline: GoalDeadline) {
    match deadline {
        GoalDeadline::Unlimited => print(b"deadline=unlimited\r\n"),
        GoalDeadline::At(ns) => {
            print_line(&[b"deadline_ns=", decimal(ns as usize, &mut [0; DIGITS])])
        }
        GoalDeadline::Expired => print(b"deadline=expired\r\n"),
    }
}

pub(super) fn print_observation(bytes: &[u8]) {
    if let Some(snapshot) = GoalObservation::decode(bytes) {
        print_status(snapshot.status);
        print_deadline(snapshot.deadline);
        print_budget(snapshot.budget);
    } else {
        print(b"invalid goal observation\r\n");
    }
}

// Keep the publication report's formatting buffer out of other reply paths.
#[inline(never)]
pub(super) fn print_publication_observation(bytes: &[u8]) {
    let Some(snapshot) = PublicationObservation::decode(bytes) else {
        return print(b"invalid publication observation\r\n");
    };
    let mut reviewer_handle = [0; 18];
    let reviewer = match snapshot.reviewer {
        Some(reviewer) => audit::publication_reviewer_label(reviewer, &mut reviewer_handle),
        None => b"none",
    };
    let mut line = ConsoleLine::new();
    line.write(b"publication goal=");
    line.write(&hex(snapshot.goal));
    line.write(b" outcome=");
    line.write(audit::publication_outcome_label(snapshot.outcome));
    line.write(b" renames=");
    line.write(decimal(snapshot.renames, &mut [0; DIGITS]));
    line.write(b" reviewer=");
    line.write(reviewer);
    line.write(b"\r\n");
    write_budget(&mut line, snapshot.budget);
    line.finish(&[]);
}

#[inline(never)]
pub(super) fn print_read_denial_alert(handle: usize, bytes: &[u8]) {
    let (Some(event), Some(suppressed)) = (
        ReadDenialEvent::decode(bytes),
        goal::take_read_denial_suppressed(handle),
    ) else {
        return print(b"read-denial alert unavailable\r\n");
    };
    print_line(&[
        b"read-denial alert goal=",
        &hex(event.goal),
        b" child=",
        &hex(event.child),
        b" requested=",
        decimal(event.requested, &mut [0; DIGITS]),
        b" denied=outside-read-scope suppressed=",
        decimal(suppressed, &mut [0; DIGITS]),
    ]);
}

pub(super) fn print_resume_receipt(bytes: &[u8]) {
    if let Some(receipt) = agent::ResumeReceipt::decode(bytes) {
        print_handles(
            b"resumed by supervisor=",
            receipt.supervisor,
            b" goal=",
            receipt.goal,
        );
        print_handles(b"agent=", receipt.agent, b" child=", receipt.child);
    } else {
        print(b"cannot resume agent via echo\r\n");
    }
}

pub(super) fn agent_command(
    arguments: &[u8],
    current: &mut Option<CurrentGoal>,
    children: &mut Children,
) {
    let agent_handle = current.as_ref().and_then(|goal| goal.agent);
    let (name, args) = split_word(arguments);
    match (Subcommand::parse(name), args) {
        (Subcommand::Delegate, arguments) => children.delegate(agent_handle, arguments),
        (Subcommand::Run, arguments) => {
            let Some(goal) = current else {
                return print(b"no current goal\r\n");
            };
            if let Some(launch) = children.run_agent(goal.handle, arguments) {
                goal.agent = Some(launch.agent);
                print_handles(b"agent=", launch.agent, b" child=", launch.child);
            }
        }
        (Subcommand::Status, b"") => print_agent_status(agent_handle),
        (Subcommand::Resume, args) if split_word(args) == (b"--via", b"echo") => match agent_handle
        {
            Some(agent) => children.resume_via_echo(agent),
            None => print(b"no current agent\r\n"),
        },
        (Subcommand::Resume, b"") if current.is_none() => {
            let (handle, launch) = match children.resume_checkpoint() {
                Ok(resumed) => resumed,
                Err(agent::CheckpointResumeError::OutcomeUnknown) => {
                    return print(b"host checkpoint outcome unknown\r\n");
                }
                Err(_) => return print(b"no current agent\r\n"),
            };
            *current = Some(CurrentGoal {
                handle,
                agent: Some(launch.agent),
                review: None,
                require_rename_review: false,
            });
            print(b"agent checkpoint resumed\r\n");
            print_handles(b"agent=", launch.agent, b" child=", launch.child);
        }
        (action @ (Subcommand::Suspend | Subcommand::Resume), b"") => {
            let suspending = matches!(action, Subcommand::Suspend);
            let Some(handle) = agent_handle else {
                return print(b"no current agent\r\n");
            };
            let result = if suspending {
                agent::suspend(handle).ok_or(agent::CheckpointResumeError::Unavailable)
            } else {
                agent::resume(handle)
            };
            match result {
                Ok(agent::ControlResult::Applied) if suspending => print(b"agent suspended\r\n"),
                Ok(agent::ControlResult::Applied) => print(b"agent resumed\r\n"),
                Ok(agent::ControlResult::AlreadyFinished) => print(b"agent already finished\r\n"),
                Err(agent::CheckpointResumeError::OutcomeUnknown) => {
                    print(b"host checkpoint outcome unknown\r\n")
                }
                Err(_) => print(b"no current agent\r\n"),
            }
        }
        _ => {
            print(b"usage: agent <subcommand>\r\n");
            help::agents();
        }
    }
}

fn print_handles(label: &[u8], handle: usize, related_label: &[u8], related: usize) {
    print_line(&[label, &hex(handle), related_label, &hex(related)]);
}

fn print_agent_status(agent_handle: Option<usize>) {
    let mut definition = NO_DEFINITION;
    if let Some(handle) = agent_handle
        && let Some((snapshot, pin)) = agent::status_with_definition(handle, &mut definition)
    {
        print_handles(b"agent=", handle, b" goal=", snapshot.goal);
        if let Some(pin) = pin {
            print_line(&[
                b"definition=",
                pin.id.intent.name().as_bytes(),
                b"@",
                decimal(usize::from(pin.id.revision), &mut [0; DIGITS]),
                b" sha256=",
                &hex_digest(pin.sha256),
            ]);
        }
        match snapshot.status {
            GoalStatus::Running { child } if snapshot.suspended => {
                print_handle(b"Suspended child=", child)
            }
            status => print_status(status),
        }
    } else {
        print(b"no current agent\r\n");
    }
}

fn hex_digest(hash: &[u8; SHA256_BYTES]) -> [u8; 2 * SHA256_BYTES] {
    let mut text = [0; 2 * SHA256_BYTES];
    for (&byte, pair) in hash.iter().zip(text.chunks_exact_mut(2)) {
        pair[0] = HEX_DIGITS[usize::from(byte >> 4)];
        pair[1] = HEX_DIGITS[usize::from(byte & 15)];
    }
    text
}

fn print_status(status: GoalStatus) {
    let (state, child, reason, value): (&[u8], _, &[u8], _) = match status {
        GoalStatus::Ready => return print(b"Ready\r\n"),
        GoalStatus::Running { child } => return print_handle(b"Running child=", child),
        GoalStatus::Completed { child } => (b"Completed child=", child, b" exit=", 0),
        GoalStatus::Failed {
            child,
            kind: CompletionKind::Exited,
            value,
        } => (b"Failed child=", child, b" exit=", value),
        GoalStatus::Failed {
            child,
            kind: CompletionKind::Faulted,
            value,
        } => (b"Failed child=", child, b" scause=", value),
        GoalStatus::Cancelled { child: Some(child) }
        | GoalStatus::Failed {
            child,
            kind: CompletionKind::Cancelled,
            ..
        } => {
            return print_handle(b"Cancelled child=", child);
        }
        GoalStatus::Cancelled { child: None } => return print(b"Cancelled\r\n"),
    };
    print_line(&[state, &hex(child), reason, &hex(value)]);
}
