mod options;

use clv::goal::{self, CancelResult, CloseResult, GoalStatus, ReplyBudget};
use clv::{agent, print};
use clv_abi::CompletionKind;
pub(super) use options::GoalOptions;

use super::{children::Children, hex, print_handle, print_line, split_word};

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
        (b"run", arguments) => children.run_in_goal(handle, arguments),
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
        _ => print(b"usage: goal create [--reply-bytes N] | run [--reply-bytes N] <hello|echo> | budget | status | cancel | close\r\n"),
    }
}

fn create(arguments: &[u8], current: &mut Option<CurrentGoal>) {
    let Some((options, b"")) = GoalOptions::parse(arguments, false) else {
        return print(b"usage: goal create [--reply-bytes N]\r\n");
    };
    let result = match options.reply_bytes {
        Some(bytes) => goal::create_with_reply_budget(bytes),
        None => goal::create(),
    };
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

fn print_budget(budget: ReplyBudget) {
    const DIGITS: usize = usize::MAX.ilog10() as usize + 1;
    fn amount(value: Option<usize>, buf: &mut [u8; DIGITS]) -> &[u8] {
        let Some(mut value) = value else {
            return b"unlimited";
        };
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
    print_line(&[
        b"available=",
        amount(budget.available, &mut [0; DIGITS]),
        b" reserved=",
        amount(budget.reserved, &mut [0; DIGITS]),
        b" spent=",
        amount(Some(budget.spent), &mut [0; DIGITS]),
    ]);
}

pub(super) fn agent_command(
    arguments: &[u8],
    current: &mut Option<CurrentGoal>,
    children: &mut Children,
) {
    let agent_handle = current.as_ref().and_then(|goal| goal.agent);
    match split_word(arguments) {
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
        _ => print(b"usage: agent run [--suspended] [--reply-bytes N] <hello|echo> | status | suspend | resume\r\n"),
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
