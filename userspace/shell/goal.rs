mod audit;
mod options;

use clv::goal::{self, CancelResult, CloseResult, FileRecipient, GoalStatus, ReplyBudget};
use clv::{agent, fs, print};
use clv_abi::{CompletionKind, OPEN_READ, Program};
pub(super) use options::{GoalCommand, GoalOptions};

use super::{
    children::Children, hex, print_handle, print_line, print_usage, split_literal, split_word,
};

const CREATE_HELP: &[u8] = b"goal create [--children 1|2|3] [--reply-bytes N] [--allow-helper <program>] [--read-bytes N] [--helper-read-bytes N]\r\n";
const GRANT_HELP: &[u8] =
    b"goal grant-file [--delegable | --delegate-to <program> [--argument <literal>]] <path>\r\n";

pub(super) fn help() {
    print(CREATE_HELP);
    print(b"goal run [--reply-bytes N] <program>\r\n");
    print(GRANT_HELP);
    print(b"goal revoke-file  goal status  goal budget  goal audit [--replies | --reads]  goal cancel  goal close\r\n");
}

pub(super) const AGENT_RUN_HELP: &[u8] = b"agent run [--suspended] [--reply-bytes N] <program> [request]\r\n\
      hello accepts --mock find-greeting | --delegate --with-file --reply-bytes N grep -F <pattern>\r\n";
pub(super) const AGENT_DELEGATE_HELP: &[u8] =
    b"agent delegate [--with-file] --reply-bytes N <program> [-F <pattern>]\r\n";

pub(super) fn agent_help() {
    print(AGENT_RUN_HELP);
    print(AGENT_DELEGATE_HELP);
    print(b"agent status  agent suspend  agent resume\r\n");
}

pub(super) struct CurrentGoal {
    handle: usize,
    agent: Option<usize>,
}

pub(super) fn command(
    arguments: &[u8],
    current: &mut Option<CurrentGoal>,
    children: &mut Children,
) {
    let handle = current.as_ref().map(|goal| goal.handle);
    match split_word(arguments) {
        (b"create", arguments) => create(arguments, current),
        (b"grant-file", arguments) => grant_file(handle, arguments),
        (b"revoke-file", b"") => match handle {
            Some(handle) if goal::revoke_file(handle) => print(b"file revoked\r\n"),
            Some(_) => print(b"cannot revoke file\r\n"),
            None => print(b"no current goal\r\n"),
        },
        (b"run", arguments) => children.run_in_goal(handle, arguments),
        (b"audit", option @ (b"" | b"--replies" | b"--reads")) => match handle {
            Some(handle) => audit::show(handle, option),
            None => print(b"no current goal\r\n"),
        },
        (b"audit", _) => print(b"usage: goal audit [--replies | --reads]\r\n"),
        (b"budget", b"") => match handle.and_then(goal::budget) {
            Some(budget) => print_budget(budget),
            None => print(b"no current goal\r\n"),
        },
        (b"status", b"") => match handle.and_then(goal::status) {
            Some(status) => print_status(status),
            None => print(b"no current goal\r\n"),
        },
        (b"cancel", b"") => match handle.and_then(goal::cancel) {
            Some(CancelResult::Cancelled) => print(b"goal cancelled\r\n"),
            Some(CancelResult::AlreadyFinished) => print(b"goal already finished\r\n"),
            None => print(b"no current goal\r\n"),
        },
        (b"close", b"") => match handle.and_then(goal::close) {
            Some(CloseResult::Closed) => {
                *current = None;
                print(b"goal closed\r\n");
            }
            Some(CloseResult::Busy) => print(b"goal busy\r\n"),
            None => print(b"no current goal\r\n"),
        },
        _ => {
            print(b"usage: ");
            help();
        }
    }
}

enum FileTarget<'a> {
    Recipient(Option<FileRecipient>),
    WithArgument(Program, &'a [u8]),
}

fn grant_options(arguments: &[u8]) -> Option<(FileTarget<'_>, &[u8])> {
    let (target, path) = match split_word(arguments) {
        (b"--delegable", path) => (FileTarget::Recipient(Some(FileRecipient::Any)), path),
        (b"--delegate-to", rest) => {
            let (name, path) = split_word(rest);
            let program = Program::named(name)?;
            if let (b"--argument", rest) = split_word(path) {
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
    (!path.is_empty() && split_word(path).0 != b"--argument").then_some((target, path))
}

fn grant_file(goal: Option<usize>, arguments: &[u8]) {
    let Some((target, path)) = grant_options(arguments) else {
        return print_usage(GRANT_HELP);
    };
    let Some(goal) = goal else {
        return print(b"no current goal\r\n");
    };
    let Some(fd) = fs::open(path, OPEN_READ) else {
        return print(b"cannot open path\r\n");
    };
    let granted = match target {
        FileTarget::Recipient(recipient) => goal::grant_file_with_recipient(goal, fd, recipient),
        FileTarget::WithArgument(program, literal) => {
            goal::grant_file_to_with_arg(goal, fd, program, literal)
        }
    };
    fs::close(fd);
    if granted {
        print(b"file granted\r\n");
    } else {
        print(b"cannot grant file\r\n");
    }
}

fn create(arguments: &[u8], current: &mut Option<CurrentGoal>) {
    let Some((options, b"")) = GoalOptions::parse(arguments, GoalCommand::Create) else {
        return print_usage(CREATE_HELP);
    };
    let result = goal::create_with_read_limits(
        goal::CreateOptions {
            child_limit: options.children.unwrap_or(1),
            reply_limit: options.reply_bytes,
            allowed_helper: options.allowed_helper,
        },
        options.read_limits,
    );
    match result {
        Some(handle) => {
            *current = Some(CurrentGoal {
                handle,
                agent: None,
            });
            print_handle(b"goal created ", handle);
        }
        None if current.is_some() => print(b"goal already open\r\n"),
        None => print(b"cannot create goal\r\n"),
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
    match split_word(arguments) {
        (b"delegate", arguments) => children.delegate(agent_handle, arguments),
        (b"run", arguments) => {
            let Some(goal) = current else {
                return print(b"no current goal\r\n");
            };
            if let Some(launch) = children.run_agent(goal.handle, arguments) {
                goal.agent = Some(launch.agent);
                print_line(&[
                    b"agent=",
                    &hex(launch.agent),
                    b" child=",
                    &hex(launch.child),
                ]);
            }
        }
        (b"status", b"") => {
            if let Some(handle) = agent_handle
                && let Some(snapshot) = agent::status(handle)
            {
                print_line(&[b"agent=", &hex(handle), b" goal=", &hex(snapshot.goal)]);
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
        (action @ (b"suspend" | b"resume"), b"") => {
            let suspending = action == b"suspend";
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
            print(b"usage: ");
            agent_help();
        }
    }
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
