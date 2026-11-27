use clv_abi::audit::{
    EncodedReadDecision, ReadAuditView, ReadDecision, ReadDenialEvent, ReadLimitScope, ReadRecord,
    ReadRecordWithLimit,
};
use clv_abi::goal::ReadLimits;

use super::MemberRole;
use super::audit::AuditHistory;
use crate::fs::ramfs::{DirectoryId, RamFs};
use crate::scheduler::Scheduler;

struct ReadScope {
    directory: DirectoryId,
    // Scope denials can evict rows here while the budget history retains them.
    audit: AuditHistory<EncodedReadDecision>,
}

impl ReadScope {
    fn permits(
        &mut self,
        fs: &RamFs,
        parent: Option<DirectoryId>,
        child: usize,
        requested: usize,
    ) -> bool {
        if parent.is_some_and(|dir| self.directory.contains(fs, dir)) {
            return true;
        }
        self.audit
            .record(ReadDecision::OutsideScope { child, requested }.encode());
        false
    }
}

pub(super) struct GoalReads {
    goal_limit: Option<usize>,
    helper_remaining: Option<usize>,
    spent: usize,
    budget_audit: AuditHistory<EncodedReadDecision>,
    scope: Option<ReadScope>,
}

impl GoalReads {
    pub(super) fn new(limits: ReadLimits, directory: Option<DirectoryId>) -> Self {
        Self {
            goal_limit: limits.goal,
            helper_remaining: limits.helpers,
            spent: 0,
            budget_audit: AuditHistory::default(),
            scope: directory.map(|directory| ReadScope {
                directory,
                audit: AuditHistory::default(),
            }),
        }
    }

    pub(super) fn pins_directory(&self, directory: DirectoryId) -> bool {
        self.scope
            .as_ref()
            .is_some_and(|scope| scope.directory == directory)
    }

    fn deliver(
        &mut self,
        child: usize,
        is_helper: bool,
        requested: usize,
        bytes: &[u8],
        copy: impl FnOnce(&[u8]) -> bool,
    ) -> bool {
        let goal_remaining = self.goal_limit.map(|limit| limit - self.spent);
        let (limit, remaining_before) = match self.helper_remaining {
            Some(helper) if is_helper && goal_remaining.is_none_or(|goal| helper < goal) => {
                (ReadLimitScope::Helper, Some(helper))
            }
            _ => (ReadLimitScope::Goal, goal_remaining),
        };
        let allowed = remaining_before.is_none_or(|remaining| bytes.len() <= remaining);
        if allowed {
            if !copy(bytes) {
                return false;
            }
            // A finite goal can spend at most its limit. Unlimited totals saturate.
            self.spent = self.spent.saturating_add(bytes.len());
            if is_helper && let Some(remaining) = &mut self.helper_remaining {
                *remaining -= bytes.len();
            }
        }
        let record = ReadRecordWithLimit {
            read: ReadRecord {
                child,
                requested,
                bytes: bytes.len(),
                remaining_before,
                delivered: allowed,
            },
            limit,
        }
        .encode();
        self.budget_audit.record(record);
        if let Some(scope) = &mut self.scope {
            scope.audit.record(record);
        }
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
        let mut denial = None;
        // RamFS resolves the handle and file slice, then advances the cursor only on delivery.
        let result = task.fs.read(fs, fd, requested, |parent, bytes| {
            if let Some(launch) = &task.launch
                && let Some(goal) = &mut self.goal
                && let Some(member) = goal.live_member_mut(launch.handle)
            {
                let is_helper = matches!(member.role, MemberRole::Helper { .. });
                if let Some(scope) = &mut goal.reads.scope
                    && !scope.permits(fs, parent, launch.handle, requested)
                {
                    denial = Some(ReadDenialEvent {
                        goal: goal.handle,
                        child: launch.handle,
                        requested,
                    });
                    return false;
                }
                goal.reads
                    .deliver(launch.handle, is_helper, requested, bytes, copy)
            } else {
                copy(bytes)
            }
        });
        if let Some(event) = denial {
            self.record_read_denial(event);
        }
        result
    }

    pub(crate) fn goal_read_audit(
        &self,
        handle: usize,
        view: ReadAuditView,
        copy: impl FnOnce(&[u8]) -> bool,
    ) -> Option<(usize, usize, usize)> {
        let reads = &self.shell_goal(handle)?.reads;
        let history = match (view, &reads.scope) {
            (ReadAuditView::Decisions, Some(scope)) => &scope.audit,
            (ReadAuditView::Budget, _) | (ReadAuditView::Decisions, None) => &reads.budget_audit,
        };
        let (count, overwritten) = history.read(copy)?;
        Some((count, overwritten, reads.spent))
    }
}
