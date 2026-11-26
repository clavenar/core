use clv_abi::audit::AUDIT_CAPACITY;

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

    /// Pack the legacy record for each entry before validating and copying the snapshot.
    pub(super) fn read_projected<const N: usize>(
        &self,
        project: impl FnMut(&[[u8; 8]; WORDS]) -> [[u8; 8]; N],
        copy: impl FnOnce(&[u8]) -> bool,
    ) -> Option<(usize, usize)> {
        let entries = self.entries.each_ref().map(project);
        let bytes = entries[..self.len].as_flattened().as_flattened();
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
            history.read_projected(|[decision @ .., _]| *decision, copy)
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
