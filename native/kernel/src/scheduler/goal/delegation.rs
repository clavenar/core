use clv_abi::agent::CURRENT_AGENT;
use clv_abi::audit::{
    DelegationDenial, DelegationRecord, DelegationRecordWithCaller, DelegationRequest,
};
use clv_abi::{LaunchParent, Program};

use super::{Goal, MemberRole};
use crate::fs::ramfs::{RamFs, TaskFs};
use crate::ipc::Message;
use crate::scheduler::{PreparedLaunch, Scheduler, Task, TaskState};

impl Scheduler {
    pub(crate) fn delegate(
        &mut self,
        fs: &mut RamFs,
        mut request: DelegationRequest,
        argument: Result<Option<Message>, DelegationDenial>,
    ) -> Option<usize> {
        let (goal, agent, owner) = if request.agent == CURRENT_AGENT {
            let child = self.tasks[self.current].launch.as_ref()?.handle;
            let goal = self.goal.as_ref()?;
            let member = goal
                .members()
                .find(|m| m.handle == child && m.completion.is_none())?;
            let MemberRole::Agent(handle) = member.role else {
                return None;
            };
            request.agent = handle;
            (goal, member, LaunchParent::Agent(child))
        } else {
            let (goal, member) = self.agent_member(request.agent)?;
            (goal, member, LaunchParent::Shell)
        };
        let parent = agent.handle;
        let launch = if agent.completion.is_some() {
            Err(DelegationDenial::AgentFinished)
        } else {
            goal.helper_program(request.program)
                .and_then(|program| self.prepare_launch(program, None, owner))
        };
        let goal = self.goal.as_mut()?;
        let admission = launch.and_then(|launch| {
            let argument = argument?;
            let child_fs = goal.admit_helper(fs, &self.tasks, parent, &launch, request)?;
            Ok((launch, child_fs, argument))
        });
        let outcome = match &admission {
            Ok((launch, ..)) => Ok(launch.handle),
            Err(reason) => Err(*reason),
        };
        goal.delegation_audit.record(
            DelegationRecordWithCaller {
                decision: DelegationRecord { request, outcome },
                caller: owner,
            }
            .encode(),
        );
        let (launch, child_fs, argument) = admission.ok()?;
        Some(self.install_child(launch, child_fs, argument, None, TaskState::Runnable))
    }
}

impl Goal {
    fn helper_program(&self, selector: usize) -> Result<Program, DelegationDenial> {
        let program = Program::from_raw(selector).ok_or(DelegationDenial::UnknownProgram)?;
        if self
            .options
            .allowed_helper
            .is_some_and(|allowed| allowed != program)
        {
            return Err(DelegationDenial::HelperNotAllowed);
        }
        Ok(program)
    }

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
