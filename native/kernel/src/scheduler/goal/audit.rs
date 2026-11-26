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

    fn read(&self, copy: impl FnOnce(&[u8]) -> bool) -> Option<(usize, usize)> {
        let bytes = self.entries[..self.len].as_flattened().as_flattened();
        copy(bytes).then_some((self.len, self.overwritten))
    }
}

impl Scheduler {
    pub(crate) fn read_goal_audit(
        &self,
        handle: usize,
        replies: bool,
        copy: impl FnOnce(&[u8]) -> bool,
    ) -> Option<(usize, usize)> {
        let goal = self.shell_goal(handle)?;
        if replies {
            goal.reply_audit.read(copy)
        } else {
            goal.delegation_audit.read(copy)
        }
    }
}
