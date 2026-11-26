use clv_abi::audit::{DelegationDenial, DelegationRecord, DelegationRequest};

use super::{Goal, MemberRole};
use crate::fs::ramfs::{RamFs, TaskFs};
use crate::scheduler::{PreparedLaunch, Scheduler, Task, TaskState};

impl Scheduler {
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
            let child_fs = goal.admit_helper(fs, &self.tasks, parent, &launch, request)?;
            Ok((launch, child_fs))
        });
        let outcome = match &admission {
            Ok((launch, _)) => Ok(launch.handle),
            Err(reason) => Err(*reason),
        };
        goal.delegation_audit
            .record(DelegationRecord { request, outcome }.encode());
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
        launch: &PreparedLaunch,
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
            task.fs.delegate_file(fs, launch.program)?
        } else {
            TaskFs::empty()
        };
        agent.replies = retained;
        self.admit(
            slot,
            launch.handle,
            MemberRole::Helper { parent },
            delegated,
            fs,
        );
        Ok(child_fs)
    }
}
