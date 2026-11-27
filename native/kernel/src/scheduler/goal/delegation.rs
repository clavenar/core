use clv_abi::agent::CURRENT_AGENT;
use clv_abi::audit::{
    DelegationDenial, DelegationRecord, DelegationRecordWithCaller, DelegationRecordWithTool,
    DelegationRequest, RequestedTool, UNRESOLVED_TOOL_PROGRAM,
};
use clv_abi::tool::ToolId;
use clv_abi::{LaunchParent, Program, RequestKind};

use super::{DelegationTarget, Goal, MemberRole};
use crate::fs::ramfs::{RamFs, TaskFs};
use crate::ipc::{Message, MessageKind};
use crate::scheduler::{LaunchPipes, PreparedLaunch, Scheduler, Task, TaskState};

impl Scheduler {
    pub(crate) fn delegate(
        &mut self,
        fs: &mut RamFs,
        agent_handle: usize,
        target: DelegationTarget,
        reply_bytes: usize,
        argument: Result<Option<Message>, DelegationDenial>,
    ) -> Option<usize> {
        let (goal, agent, owner, agent_handle) = if agent_handle == CURRENT_AGENT {
            let (goal, member, binding) = self.current_agent_member()?;
            (
                goal,
                member,
                LaunchParent::Agent(member.handle),
                binding.handle,
            )
        } else {
            let (goal, member, _) = self.agent_member(agent_handle)?;
            (goal, member, LaunchParent::Shell, agent_handle)
        };
        let (request, tool) = target.resolve(agent_handle, reply_bytes);
        let parent = agent.handle;
        let launch = if agent.completion.is_some() {
            Err(DelegationDenial::AgentFinished)
        } else {
            goal.helper_program(request.program, tool.as_ref().map(|requested| requested.id))
                .and_then(|program| self.prepare_launch(program, None, owner, None))
        };
        let goal = self.goal.as_mut()?;
        let admission = launch.and_then(|launch| {
            let argument = argument?;
            let child_fs = goal.admit_helper(
                fs,
                &self.tasks,
                parent,
                &launch,
                request,
                argument.as_ref().map(|message| message.bytes(false)),
            )?;
            Ok((launch, child_fs, argument))
        });
        let outcome = match &admission {
            Ok((launch, ..)) => Ok(launch.handle),
            Err(reason) => Err(*reason),
        };
        goal.delegation_audit.record(
            DelegationRecordWithTool {
                delegation: DelegationRecordWithCaller {
                    decision: DelegationRecord { request, outcome },
                    caller: owner,
                },
                tool,
            }
            .encode(),
        );
        let (launch, child_fs, argument) = admission.ok()?;
        let pipes = if argument.as_ref().is_some_and(|message| {
            matches!(
                message.kind,
                MessageKind::Request(RequestKind::FileToolStdio | RequestKind::FileToolMcp)
            )
        }) {
            LaunchPipes::stdio()
        } else {
            LaunchPipes::default()
        };
        Some(self.install_child(launch, child_fs, argument, pipes, TaskState::Runnable))
    }
}

impl DelegationTarget {
    fn resolve(
        self,
        agent: usize,
        reply_bytes: usize,
    ) -> (DelegationRequest, Option<RequestedTool<'static>>) {
        let (program, with_file, tool) = match self {
            Self::Program {
                selector,
                with_file,
            } => (selector, with_file, None),
            Self::FileTool(id) => {
                let binding = crate::programs::file_tool(id);
                let tool = RequestedTool {
                    id,
                    image_sha256: binding.map(|(_, hash)| hash),
                };
                let selector =
                    binding.map_or(UNRESOLVED_TOOL_PROGRAM, |(program, _)| program as usize);
                (selector, true, Some(tool))
            }
        };
        (
            DelegationRequest {
                agent,
                program,
                reply_bytes,
                with_file,
            },
            tool,
        )
    }
}

impl Goal {
    fn helper_program(
        &self,
        selector: usize,
        requested_tool: Option<ToolId>,
    ) -> Result<Program, DelegationDenial> {
        let program = Program::from_raw(selector).ok_or(DelegationDenial::UnknownProgram)?;
        if self
            .allowed_tool
            .is_some_and(|allowed| Some(allowed) != requested_tool)
        {
            return Err(DelegationDenial::ToolNotAllowed);
        }
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
        argument: Option<&[u8]>,
    ) -> Result<TaskFs, DelegationDenial> {
        let slot = self.free_slot().ok_or(DelegationDenial::GoalFull)?;
        let agent = self
            .members
            .iter_mut()
            .flatten()
            .find(|member| member.is_live(parent))
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
            let grant = self.file_grants[0]
                .as_ref()
                .ok_or(DelegationDenial::NoDelegableFile)?;
            grant.delegate(fs, &task.fs, launch.program, argument)?
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
