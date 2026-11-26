use clv_abi::audit::{
    AUDIT_CAPACITY, DelegationDenial, DelegationRecord, DelegationRequest, EncodedRecord,
};

use super::{Goal, MemberRole};
use crate::fs::ramfs::{RamFs, TaskFs};
use crate::scheduler::{Scheduler, Task, TaskState};

#[derive(Default)]
pub(super) struct DelegationAudit {
    entries: [EncodedRecord; AUDIT_CAPACITY],
    len: usize,
    overwritten: usize,
}

impl DelegationAudit {
    fn record(&mut self, entry: DelegationRecord) {
        if self.len == self.entries.len() {
            self.entries.copy_within(1.., 0);
            self.overwritten = self.overwritten.saturating_add(1);
        } else {
            self.len += 1;
        }
        self.entries[self.len - 1] = entry.encode();
    }
}

impl Scheduler {
    pub(crate) fn goal_audit(&self, handle: usize) -> Option<(&[EncodedRecord], usize)> {
        let audit = &self.shell_goal(handle)?.audit;
        Some((&audit.entries[..audit.len], audit.overwritten))
    }

    pub(crate) fn delegate(&mut self, fs: &mut RamFs, request: DelegationRequest) -> Option<usize> {
        let (_, agent) = self.agent_member(request.agent)?;
        let parent = agent.handle;
        let launch = if agent.completion.is_some() {
            Err(DelegationDenial::AgentFinished)
        } else {
            self.prepare_launch(request.program, None)
        };
        let goal = self.goal.as_mut()?;
        let admission = launch.and_then(|launch| {
            let child_fs = goal.admit_helper(fs, &self.tasks, parent, launch.handle, request)?;
            Ok((launch, child_fs))
        });
        let outcome = match &admission {
            Ok((launch, _)) => Ok(launch.handle),
            Err(reason) => Err(*reason),
        };
        goal.audit.record(DelegationRecord { request, outcome });
        let (launch, child_fs) = admission.ok()?;
        Some(self.install_child(launch, child_fs, None, None, TaskState::Runnable))
    }
}

impl Goal {
    fn admit_helper(
        &mut self,
        fs: &mut RamFs,
        tasks: &[Task],
        parent: usize,
        child: usize,
        request: DelegationRequest,
    ) -> Result<TaskFs, DelegationDenial> {
        let slot = self.free_slot().ok_or(DelegationDenial::GoalFull)?;
        let agent = self
            .live_member_mut(parent)
            .ok_or(DelegationDenial::AgentFinished)?;
        let (retained, delegated) = agent
            .replies
            .split(request.reply_bytes)
            .ok_or(DelegationDenial::InsufficientReplyBytes)?;
        let child_fs = if request.with_file {
            let task = tasks
                .iter()
                .find(|task| task.has_launch(parent))
                .ok_or(DelegationDenial::AgentFinished)?;
            task.fs.delegate_file(fs)?
        } else {
            TaskFs::empty()
        };
        agent.replies = retained;
        self.admit(slot, child, MemberRole::Helper { parent }, delegated, fs);
        Ok(child_fs)
    }
}
