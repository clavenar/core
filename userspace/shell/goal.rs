use clv::goal::{self, CancelResult, CloseResult, GoalStatus};
use clv::{agent, print};
use clv_abi::CompletionKind;

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
        (b"create", b"") => match goal::create() {
            Some(handle) => {
                *current = Some(CurrentGoal {
                    handle,
                    agent: None,
                });
                print_handle(b"goal created ", handle);
            }
            None if current.is_some() => print(b"goal already open\r\n"),
            None => print(b"cannot create goal\r\n"),
        },
        (b"run", name) => children.run_in_goal(handle, name),
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
        _ => print(b"usage: goal create | run <hello|echo> | status | cancel | close\r\n"),
    }
}

pub(super) fn agent_command(
    arguments: &[u8],
    current: &mut Option<CurrentGoal>,
    children: &mut Children,
) {
    match split_word(arguments) {
        (b"run", name) => {
            let Some(goal) = current else {
                return print(b"no current goal\r\n");
            };
            if let Some(launch) = children.run_agent(goal.handle, name) {
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
            if let Some(handle) = current.as_ref().and_then(|goal| goal.agent)
                && let Some(snapshot) = agent::status(handle)
            {
                print_line(&[b"agent=", &hex(handle), b" goal=", &hex(snapshot.goal)]);
                print_status(snapshot.status);
            } else {
                print(b"no current agent\r\n");
            }
        }
        _ => print(b"usage: agent run <hello|echo> | status\r\n"),
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
