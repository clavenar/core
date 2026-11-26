use clv_abi::audit::{EncodedReadRecord, ReadRecord};

use super::audit::AuditHistory;
use crate::fs::ramfs::RamFs;
use crate::scheduler::Scheduler;

pub(super) struct ReadBudget {
    limit: Option<usize>,
    spent: usize,
    audit: AuditHistory<EncodedReadRecord>,
}

impl ReadBudget {
    pub(super) fn new(limit: Option<usize>) -> Self {
        Self {
            limit,
            spent: 0,
            audit: AuditHistory::default(),
        }
    }

    fn deliver(
        &mut self,
        child: usize,
        requested: usize,
        bytes: &[u8],
        copy: impl FnOnce(&[u8]) -> bool,
    ) -> bool {
        let remaining_before = self.limit.map(|limit| limit - self.spent);
        let allowed = remaining_before.is_none_or(|remaining| bytes.len() <= remaining);
        if allowed {
            if !copy(bytes) {
                return false;
            }
            // A finite goal can spend at most its limit. Unlimited totals saturate.
            self.spent = self.spent.saturating_add(bytes.len());
        }
        self.audit.record(
            ReadRecord {
                child,
                requested,
                bytes: bytes.len(),
                remaining_before,
                delivered: allowed,
            }
            .encode(),
        );
        allowed
    }
}

impl Scheduler {
    pub(crate) fn read_file(
        &mut self,
        fs: &RamFs,
        fd: usize,
        requested: usize,
        copy: impl FnOnce(&[u8]) -> bool,
    ) -> Option<usize> {
        let task = &mut self.tasks[self.current];
        // RamFS resolves the handle and file slice, then advances the cursor only on delivery.
        task.fs.read(fs, fd, requested, |bytes| {
            if let Some(launch) = &task.launch
                && let Some(goal) = &mut self.goal
                && goal.live_member_mut(launch.handle).is_some()
            {
                goal.reads.deliver(launch.handle, requested, bytes, copy)
            } else {
                copy(bytes)
            }
        })
    }

    pub(crate) fn goal_read_audit(
        &self,
        handle: usize,
        copy: impl FnOnce(&[u8]) -> bool,
    ) -> Option<(usize, usize, usize)> {
        let reads = &self.shell_goal(handle)?.reads;
        let (count, overwritten) = reads.audit.read(copy)?;
        Some((count, overwritten, reads.spent))
    }
}
