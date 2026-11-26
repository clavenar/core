use clv::goal;
use clv_abi::audit::DelegationDenial;

use super::{Children, DIGITS, decimal, hex, print, print_line};

pub(super) fn show(handle: usize, children: &Children) {
    let Some(snapshot) = goal::audit(handle) else {
        return print(b"cannot read goal audit\r\n");
    };
    let entries = snapshot.entries.iter().flatten();
    print_line(&[
        b"goal=",
        &hex(handle),
        b" entries=",
        decimal(entries.clone().count(), &mut [0; DIGITS]),
        b" overwritten=",
        decimal(snapshot.overwritten, &mut [0; DIGITS]),
    ]);
    for entry in entries {
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
            children
                .program_name(request.program)
                .unwrap_or(&hex(request.program)),
            b" bytes=",
            decimal(request.reply_bytes, &mut [0; DIGITS]),
            b" file=",
            if request.with_file { b"yes" } else { b"no" },
            decision,
            detail,
        ]);
    }
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
    }
}
