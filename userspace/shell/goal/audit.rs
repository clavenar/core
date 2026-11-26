use crate::{ConsoleLine, help::Entry};
use clv::goal;
use clv_abi::audit::{
    AUDIT_CAPACITY, DelegationDenial, DelegationRecordWithTool, PublicationOutcome,
    PublicationRecord, ReadDecision, ReadLimitScope, RenameDenial, RenameRecord, ReplyRecord,
};
use clv_abi::tool::FILE_TOOLS;
use clv_abi::{LaunchParent, Program};

use super::{DIGITS, amount, decimal, hex, hex_digest, print, print_line};

enum View {
    Delegations,
    Replies,
    Reads,
    Renames,
    Publication,
}

pub(super) fn show(handle: Option<usize>, option: &[u8]) {
    let view = match option {
        b"" => View::Delegations,
        b"--replies" => View::Replies,
        b"--reads" => View::Reads,
        b"--renames" => View::Renames,
        b"--publication" => View::Publication,
        _ => return Entry::Audit.usage(),
    };
    let Some(handle) = handle else {
        return print(b"no current goal\r\n");
    };
    if show_rows(handle, view).is_none() {
        print(b"cannot read goal audit\r\n");
    }
}

fn show_rows(handle: usize, view: View) -> Option<()> {
    match view {
        View::Publication => {
            // Only this owner changes publication history. Reuse one row on its 4 KiB stack.
            let mut record = [[0; 8]; _];
            let (count, overwritten) = goal::publication_audit_row(handle, 0, &mut record)?;
            print_header(handle, count, overwritten);
            for index in 0..count {
                if index != 0 {
                    goal::publication_audit_row(handle, index, &mut record)?;
                }
                print_publication(PublicationRecord::decode(&record)?);
            }
            Some(())
        }
        View::Renames => {
            let mut records = [Default::default(); AUDIT_CAPACITY];
            let (rows, overwritten) = goal::rename_audit_rows(handle, &mut records)?;
            print_snapshot(
                handle,
                overwritten,
                rows.iter().map(RenameRecord::decode),
                print_rename,
            )
        }
        View::Reads => {
            let mut records = [Default::default(); AUDIT_CAPACITY];
            let (rows, overwritten, total) = goal::read_decision_rows(handle, &mut records)?;
            print_snapshot(
                handle,
                overwritten,
                rows.iter().map(ReadDecision::decode),
                print_read,
            )?;
            print_line(&[b"read_bytes=", decimal(total, &mut [0; DIGITS])]);
            Some(())
        }
        View::Replies => {
            let mut records = [Default::default(); AUDIT_CAPACITY];
            let (rows, overwritten) = goal::reply_audit_rows(handle, &mut records)?;
            print_snapshot(
                handle,
                overwritten,
                rows.iter().map(ReplyRecord::decode),
                print_reply,
            )
        }
        View::Delegations => {
            let mut records = [Default::default(); AUDIT_CAPACITY];
            let (rows, overwritten) = goal::audit_with_tool(handle, &mut records)?;
            print_snapshot(
                handle,
                overwritten,
                rows.iter().map(DelegationRecordWithTool::decode),
                print_delegation,
            )
        }
    }
}

fn print_snapshot<Record>(
    handle: usize,
    overwritten: usize,
    entries: impl ExactSizeIterator<Item = Option<Record>>,
    print_entry: impl Fn(Record),
) -> Option<()> {
    print_header(handle, entries.len(), overwritten);
    for entry in entries {
        print_entry(entry?);
    }
    Some(())
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

fn caller_fields(caller: LaunchParent, child: &mut [u8; 18]) -> (&'static [u8], &[u8]) {
    match caller {
        LaunchParent::Shell => (b" caller=shell", b""),
        LaunchParent::Agent(handle) => {
            *child = hex(handle);
            (b" caller=agent caller_child=", child)
        }
    }
}

fn print_delegation(record: DelegationRecordWithTool<'_>) {
    let entry = record.delegation;
    let child;
    let (decision, detail): (&[u8], &[u8]) = match entry.decision.outcome {
        Ok(handle) => {
            child = hex(handle);
            (b" accepted child=", &child)
        }
        Err(DelegationDenial::UnknownProgram) if record.tool.is_some() => {
            (b" denied=", b"unknown-tool")
        }
        Err(reason) => (b" denied=", denial_name(reason)),
    };
    let mut caller_child = [0; 18];
    let (caller, identity) = caller_fields(entry.caller, &mut caller_child);
    let request = entry.decision.request;
    let raw_id;
    let digest;
    let mut tool_name: &[u8] = b"";
    let mut image_hash: &[u8] = b"";
    if let Some(tool) = record.tool {
        raw_id = hex(tool.id.0 as usize);
        tool_name = FILE_TOOLS
            .iter()
            .find(|item| item.id == tool.id)
            .map_or(&raw_id[..], |item| item.name.as_bytes());
        if let Some(hash) = tool.image_sha256 {
            digest = hex_digest(hash);
            image_hash = &digest;
        }
    }
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
        caller,
        identity,
        if tool_name.is_empty() { b"" } else { b" tool=" },
        tool_name,
        if image_hash.is_empty() {
            b""
        } else {
            b" elf_sha256="
        },
        image_hash,
    ]);
}

fn print_rename(entry: RenameRecord<'_>) {
    let decision: &[u8] = match entry.outcome {
        Ok(()) => b" staged",
        Err(RenameDenial::NoRenameGrant) => b" denied=no-rename-grant",
        Err(RenameDenial::InvalidBasename) => b" denied=invalid-basename",
        Err(RenameDenial::DestinationExists) => b" denied=destination-exists",
        Err(RenameDenial::AlreadyStaged) => b" denied=already-staged",
        Err(RenameDenial::InvalidSource) => b" denied=invalid-source",
        Err(RenameDenial::GoalInactive) => b" denied=goal-inactive",
        Err(RenameDenial::InvalidBuffer) => b" denied=invalid-buffer",
        Err(RenameDenial::OutsideRenameScope) => b" denied=outside-rename-scope",
    };
    let mut line = ConsoleLine::new();
    line.write(b"name=");
    if let Some(name) = entry.basename {
        line.quoted(name);
    } else {
        line.write(b"<unavailable>");
    }
    let mut child = [0; 18];
    let (caller, identity) = caller_fields(entry.caller, &mut child);
    line.finish(&[decision, caller, identity]);
}

fn print_publication(entry: PublicationRecord<'_>) {
    use goal::CommitResult;

    let outcome: &[u8] = match entry.outcome {
        PublicationOutcome::Commit(CommitResult::Committed) => b"published",
        PublicationOutcome::Commit(CommitResult::Conflict) => b"conflict",
        PublicationOutcome::Commit(CommitResult::PostconditionFailed) => b"postcondition-failed",
        PublicationOutcome::Commit(CommitResult::ReviewExpired) => b"review-expired",
        PublicationOutcome::Unavailable => b"unavailable",
        PublicationOutcome::Discarded => b"discarded",
    };
    let mut line = ConsoleLine::new();
    line.write(b"source=");
    line.quoted(entry.source);
    line.write(b" new_name=");
    line.quoted(entry.new_name);
    line.finish(&[
        b" base=",
        decimal(entry.base_revision as usize, &mut [0; DIGITS]),
        b" revision=",
        decimal(entry.revision as usize, &mut [0; DIGITS]),
        b" outcome=",
        outcome,
    ]);
}

fn print_reply(entry: ReplyRecord) {
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

fn print_read(decision: ReadDecision) {
    let entry = match decision {
        ReadDecision::Budget(entry) => entry,
        ReadDecision::OutsideScope { child, requested } => {
            return print_line(&[
                b"child=",
                &hex(child),
                b" requested=",
                decimal(requested, &mut [0; DIGITS]),
                b" bytes=0 denied=outside-read-scope",
            ]);
        }
    };
    let read = &entry.read;
    print_line(&[
        b"child=",
        &hex(read.child),
        b" requested=",
        decimal(read.requested, &mut [0; DIGITS]),
        b" bytes=",
        decimal(read.bytes, &mut [0; DIGITS]),
        b" remaining_before=",
        amount(read.remaining_before, &mut [0; DIGITS]),
        match (read.delivered, entry.limit) {
            (true, _) => b" read",
            (false, ReadLimitScope::Goal) => b" denied=insufficient-read-bytes limit=goal",
            (false, ReadLimitScope::Helper) => b" denied=insufficient-read-bytes limit=helper",
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
        DelegationDenial::ArgumentNotAllowed => b"argument-not-allowed",
        DelegationDenial::ToolNotAllowed => b"tool-not-allowed",
    }
}
