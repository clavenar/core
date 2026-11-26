use clv_abi::audit::{AUDIT_CAPACITY, EncodedRecord, EncodedRecordWithCaller};

use crate::scheduler::Scheduler;

#[derive(Default)]
pub(super) struct AuditHistory<Record> {
    entries: [Record; AUDIT_CAPACITY],
    len: usize,
    overwritten: usize,
}

impl<const WORDS: usize> AuditHistory<[[u8; 8]; WORDS]> {
    pub(super) fn record(&mut self, entry: [[u8; 8]; WORDS]) {
        if self.len == self.entries.len() {
            self.entries.copy_within(1.., 0);
            self.overwritten = self.overwritten.saturating_add(1);
        } else {
            self.len += 1;
        }
        self.entries[self.len - 1] = entry;
    }

    pub(super) fn read(&self, copy: impl FnOnce(&[u8]) -> bool) -> Option<(usize, usize)> {
        let bytes = self.entries[..self.len].as_flattened().as_flattened();
        copy(bytes).then_some((self.len, self.overwritten))
    }
}

impl AuditHistory<EncodedRecordWithCaller> {
    fn read_without_caller(&self, copy: impl FnOnce(&[u8]) -> bool) -> Option<(usize, usize)> {
        // Call 52 packs six words per entry. Drop the caller from each retained record.
        let decisions: [EncodedRecord; AUDIT_CAPACITY] =
            self.entries.each_ref().map(|[decision @ .., _]| *decision);
        let bytes = decisions[..self.len].as_flattened().as_flattened();
        copy(bytes).then_some((self.len, self.overwritten))
    }
}

impl Scheduler {
    pub(crate) fn goal_delegation_audit(
        &self,
        handle: usize,
        with_caller: bool,
        copy: impl FnOnce(&[u8]) -> bool,
    ) -> Option<(usize, usize)> {
        let history = &self.shell_goal(handle)?.delegation_audit;
        if with_caller {
            history.read(copy)
        } else {
            history.read_without_caller(copy)
        }
    }

    pub(crate) fn goal_reply_audit(
        &self,
        handle: usize,
        copy: impl FnOnce(&[u8]) -> bool,
    ) -> Option<(usize, usize)> {
        self.shell_goal(handle)?.reply_audit.read(copy)
    }
}
