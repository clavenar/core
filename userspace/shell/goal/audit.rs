use clv::goal;
use clv_abi::Program;
use clv_abi::audit::{DelegationDenial, DelegationRecord, ReadRecord, ReplyRecord};

use super::{DIGITS, amount, decimal, hex, print, print_line};

pub(super) fn show(handle: usize, option: &[u8]) {
    let shown = match option {
        b"--reads" => goal::read_audit(handle).map(|(snapshot, total)| {
            print_snapshot(handle, snapshot, print_read);
            print_line(&[b"read_bytes=", decimal(total, &mut [0; DIGITS])]);
        }),
        b"--replies" => {
            goal::reply_audit(handle).map(|snapshot| print_snapshot(handle, snapshot, print_reply))
        }
        _ => goal::audit(handle).map(|snapshot| print_snapshot(handle, snapshot, print_delegation)),
    };
    if shown.is_none() {
        print(b"cannot read goal audit\r\n");
    }
}

fn print_snapshot<Record>(
    handle: usize,
    snapshot: goal::AuditSnapshot<Record>,
    print_entry: impl Fn(&Record),
) {
    let entries = snapshot.entries.iter().flatten();
    print_header(handle, entries.clone().count(), snapshot.overwritten);
    for entry in entries {
        print_entry(entry);
    }
}

fn print_header(handle: usize, count: usize, overwritten: usize) {
    print_line(&[
        b"goal=",
        &hex(handle),
        b" entries=",
        decimal(count, &mut [0; DIGITS]),
        b" overwritten=",
        decimal(overwritten, &mut [0; DIGITS]),
    ]);
}

fn print_delegation(entry: &DelegationRecord) {
    let child;
    let (decision, detail): (&[u8], &[u8]) = match entry.outcome {
        Ok(handle) => {
            child = hex(handle);
            (b" accepted child=", &child)
        }
        Err(reason) => (b" denied=", denial_name(reason)),
    };
    let request = entry.request;
    print_line(&[
        b"agent=",
        &hex(request.agent),
        b" program=",
        Program::from_raw(request.program)
            .map(|program| program.name().as_bytes())
            .unwrap_or(&hex(request.program)),
        b" bytes=",
        decimal(request.reply_bytes, &mut [0; DIGITS]),
        b" file=",
        if request.with_file { b"yes" } else { b"no" },
        decision,
        detail,
    ]);
}

fn print_reply(entry: &ReplyRecord) {
    print_line(&[
        b"child=",
        &hex(entry.child),
        b" bytes=",
        decimal(entry.bytes, &mut [0; DIGITS]),
        b" remaining_before=",
        amount(entry.remaining_before, &mut [0; DIGITS]),
        if entry.sent {
            b" sent"
        } else {
            b" denied=insufficient-reply-bytes"
        },
    ]);
}

fn print_read(entry: &ReadRecord) {
    print_line(&[
        b"child=",
        &hex(entry.child),
        b" requested=",
        decimal(entry.requested, &mut [0; DIGITS]),
        b" bytes=",
        decimal(entry.bytes, &mut [0; DIGITS]),
        b" remaining_before=",
        amount(entry.remaining_before, &mut [0; DIGITS]),
        if entry.delivered {
            b" read"
        } else {
            b" denied=insufficient-read-bytes"
        },
    ]);
}

fn denial_name(reason: DelegationDenial) -> &'static [u8] {
    match reason {
        DelegationDenial::AgentFinished => b"agent-finished",
        DelegationDenial::UnknownProgram => b"unknown-program",
        DelegationDenial::ProgramBusy => b"program-busy",
        DelegationDenial::LaunchHandlesExhausted => b"launch-handles-exhausted",
        DelegationDenial::GoalFull => b"goal-full",
        DelegationDenial::InsufficientReplyBytes => b"insufficient-reply-bytes",
        DelegationDenial::NoDelegableFile => b"no-delegable-file",
        DelegationDenial::FileHandlesExhausted => b"file-handles-exhausted",
        DelegationDenial::FileRecipientNotAllowed => b"file-recipient-not-allowed",
        DelegationDenial::InvalidArgument => b"invalid-argument",
        DelegationDenial::HelperNotAllowed => b"helper-not-allowed",
    }
}
