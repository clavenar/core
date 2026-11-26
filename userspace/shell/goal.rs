mod audit;
mod options;

use super::color;
use super::command::{GoalFlag, Subcommand};
use clv::goal::{
    self, CancelResult, CloseResult, CommitResult, DiscardResult, FileRecipient, GoalStatus,
    PublicationError, ReplyBudget, ReviewToken, TimedRenameReview,
};
use clv::{agent, fs, print};
use clv_abi::agent::{NO_DEFINITION, SHA256_BYTES};
use clv_abi::{CompletionKind, OPEN_READ, Program};
pub(super) use options::{GoalCommand, GoalOptions};

use super::{
    HEX_DIGITS,
    children::Children,
    help::{self, Entry},
    hex, print_handle, print_line, split_literal, split_word,
};

pub(super) struct CurrentGoal {
    handle: usize,
    agent: Option<usize>,
    review: Option<ReviewToken>,
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
        (action @ (Subcommand::Diff | Subcommand::Commit | Subcommand::Discard), b"") => {
            match (action, current) {
                (Subcommand::Commit, Some(goal)) => commit(goal),
                (Subcommand::Discard, Some(goal)) => discard(goal),
                (_, Some(goal)) => show_diff(goal),
                (_, None) => print(b"no current goal\r\n"),
            }
        }
        (Subcommand::RevokeFile, b"") => match handle {
            Some(handle) if goal::revoke_file(handle) => print(b"file revoked\r\n"),
            Some(_) => print(b"cannot revoke file\r\n"),
            None => print(b"no current goal\r\n"),
        },
        (Subcommand::Run, arguments) => children.run_in_goal(handle, arguments),
        (Subcommand::Audit, option) => audit::show(handle, option),
        (Subcommand::Budget, b"") => match handle.and_then(goal::budget) {
            Some(budget) => print_budget(budget),
            None => print(b"no current goal\r\n"),
        },
        (Subcommand::Status, b"") => match handle.and_then(goal::status) {
            Some(status) => print_status(status),
            None => print(b"no current goal\r\n"),
        },
        (Subcommand::Cancel, b"") => match current.as_mut().and_then(|goal| {
            let result = goal::cancel(goal.handle)?;
            goal.review = None;
            Some(result)
        }) {
            Some(CancelResult::Cancelled) => print(b"goal cancelled\r\n"),
            Some(CancelResult::AlreadyFinished) => print(b"goal already finished\r\n"),
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
    let Some(fd) = fs::open(path, OPEN_READ) else {
        print(b"cannot open path\r\n");
        return None;
    };
    let result = operation(goal, fd);
    fs::close(fd);
    Some(result)
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

fn show_diff(goal: &mut CurrentGoal) {
    let mut buffer = [0; clv_abi::goal::RENAME_DIFF_MAX_BYTES];
    goal.review = None;
    match goal::review_with_deadline(goal.handle, &mut buffer) {
        Some((goal::GoalDiff::Empty, _)) => print(b"no staged changes\r\n"),
        Some((goal::GoalDiff::Rename { source, new_name }, review)) => {
            print_line(&[b"rename ", source, b" -> ", new_name]);
            match review {
                TimedRenameReview::Preview => print(b"preview only\r\n"),
                TimedRenameReview::Ready(review) => {
                    goal.review = Some(review.token);
                    print_review_deadline(review.deadline_ns);
                }
                TimedRenameReview::Blocked(reason) => print(match reason {
                    PublicationError::Conflict => b"conflict\r\n",
                    PublicationError::PostconditionFailed => b"postcondition failed\r\n",
                    PublicationError::Unavailable => b"review unavailable\r\n",
                }),
            }
        }
        None => print(b"cannot read goal diff\r\n"),
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
    let Some(token) = goal.review.take() else {
        return print(b"goal commit requires a passing goal diff after completion\r\n");
    };
    match goal::commit(goal.handle, token) {
        Some(CommitResult::Committed) => color::print_line(color::GREEN, b"rename committed"),
        Some(CommitResult::Conflict) => color::print_line(color::RED, b"commit conflict"),
        Some(CommitResult::ReviewExpired) => color::print_line(color::YELLOW, b"review expired"),
        Some(CommitResult::PostconditionFailed) => print(b"postcondition failed\r\n"),
        None => print(b"cannot commit goal\r\n"),
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

fn create(arguments: &[u8], current: &mut Option<CurrentGoal>) {
    let Some((options, b"")) = GoalOptions::parse(arguments, GoalCommand::Create) else {
        return Entry::GoalCreate.usage();
    };
    match create_goal(options) {
        Some(handle) => {
            *current = Some(CurrentGoal {
                handle,
                agent: None,
                review: None,
            });
            print_handle(b"goal created ", handle);
        }
        None if current.is_some() => print(b"goal already open\r\n"),
        None => print(b"cannot create goal\r\n"),
    }
}

fn create_goal(options: GoalOptions<'_>) -> Option<usize> {
    with_optional_handle(options.rename_under, |directory| {
        with_optional_handle(options.preserve_file, |preserve_file| {
            goal::create_with_rename_options(
                goal::CreateOptions {
                    child_limit: options.children.unwrap_or(1),
                    reply_limit: options.reply_bytes,
                    allowed_helper: options.allowed_helper,
                },
                options.read_limits,
                options.allowed_tool,
                goal::RenameOptions {
                    directory,
                    preserve_file,
                },
            )
        })
    })
}

fn with_optional_handle(
    path: Option<&[u8]>,
    operation: impl FnOnce(Option<usize>) -> Option<usize>,
) -> Option<usize> {
    let Some(path) = path else {
        return operation(None);
    };
    let fd = fs::open(path, OPEN_READ)?;
    let result = operation(Some(fd));
    fs::close(fd);
    result
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

fn amount(value: Option<usize>, buf: &mut [u8; DIGITS]) -> &[u8] {
    match value {
        Some(value) => decimal(value, buf),
        None => b"unlimited",
    }
}

fn print_budget(budget: ReplyBudget) {
    print_line(&[
        b"available=",
        amount(budget.available, &mut [0; DIGITS]),
        b" reserved=",
        amount(budget.reserved, &mut [0; DIGITS]),
        b" spent=",
        decimal(budget.spent, &mut [0; DIGITS]),
    ]);
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
                print_agent_handles(launch.agent, b" child=", launch.child);
            }
        }
        (Subcommand::Status, b"") => print_agent_status(agent_handle),
        (action @ (Subcommand::Suspend | Subcommand::Resume), b"") => {
            let suspending = matches!(action, Subcommand::Suspend);
            let control = if suspending {
                agent::suspend
            } else {
                agent::resume
            };
            match agent_handle.and_then(control) {
                Some(agent::ControlResult::Applied) if suspending => print(b"agent suspended\r\n"),
                Some(agent::ControlResult::Applied) => print(b"agent resumed\r\n"),
                Some(agent::ControlResult::AlreadyFinished) => print(b"agent already finished\r\n"),
                None => print(b"no current agent\r\n"),
            }
        }
        _ => {
            print(b"usage: agent <subcommand>\r\n");
            help::agents();
        }
    }
}

fn print_agent_handles(agent: usize, label: &[u8], related: usize) {
    print_line(&[b"agent=", &hex(agent), label, &hex(related)]);
}

fn print_agent_status(agent_handle: Option<usize>) {
    let mut definition = NO_DEFINITION;
    if let Some(handle) = agent_handle
        && let Some((snapshot, pin)) = agent::status_with_definition(handle, &mut definition)
    {
        print_agent_handles(handle, b" goal=", snapshot.goal);
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
