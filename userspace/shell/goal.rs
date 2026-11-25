use clv::goal::{self, CloseResult, GoalStatus};
use clv::print;
use clv_abi::CompletionKind;

use super::{children::Children, hex, print_handle, print_line, split_word};

pub(super) fn command(arguments: &[u8], current: &mut Option<usize>, children: &mut Children) {
    match split_word(arguments) {
        (b"create", b"") => match goal::create() {
            Some(handle) => {
                *current = Some(handle);
                print_handle(b"goal created ", handle);
            }
            None if current.is_some() => print(b"goal already open\r\n"),
            None => print(b"cannot create goal\r\n"),
        },
        (b"run", name) => children.run_in_goal(*current, name),
        (b"status", b"") => match current.and_then(goal::status) {
            Some(status) => print_status(status),
            None => print(b"no current goal\r\n"),
        },
        (b"close", b"") => match current.and_then(goal::close) {
            Some(CloseResult::Closed) => {
                *current = None;
                print(b"goal closed\r\n");
            }
            Some(CloseResult::Busy) => print(b"goal busy\r\n"),
            None => print(b"no current goal\r\n"),
        },
        _ => print(b"usage: goal create | run <hello|echo> | status | close\r\n"),
    }
}

fn print_status(status: GoalStatus) {
    let (state, child, reason, value): (&[u8], _, &[u8], _) = match status {
        GoalStatus::Ready => return print(b"Ready\r\n"),
        GoalStatus::Running { child } => return print_handle(b"Running child=", child),
        GoalStatus::Completed { child } => (b"Completed child=", child, b" exit=", 0),
        GoalStatus::Failed { child, kind, value } => {
            let reason: &[u8] = match kind {
                CompletionKind::Exited => b" exit=",
                CompletionKind::Faulted => b" scause=",
            };
            (b"Failed child=", child, reason, value)
        }
    };
    print_line(&[state, &hex(child), reason, &hex(value)]);
}
