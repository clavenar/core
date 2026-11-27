mod audit;
mod budget;
mod checkpoint;
mod delegation;
mod monitor;
mod reads;
mod rename;

use self::audit::{AuditHistory, PublicationHistory};
use self::budget::ReplyReservation;
pub(super) use self::checkpoint::CheckpointJournal;
pub(super) use self::monitor::{MonitorReport, ReadDenialWatch};
use self::reads::GoalReads;
use self::rename::PendingRenames;
use clv_abi::agent::{
    AgentStatus, ControlResult, DefinitionPin, EncodedDefinitionPin, FIND_GREETING_V5,
    FIND_GREETING_V6, FIND_GREETING_V7, FIND_GREETING_V8, ResumeReceipt,
};
use clv_abi::audit::{
    EncodedRecordWithTool, EncodedRenameRecord, EncodedReplyRecord, PublicationObservation,
    PublicationOutcome, ReplyRecord,
};
use clv_abi::bridge::{BridgeFailure, HostRequest, SEARCH_GREETING, plan_action};
use clv_abi::budget::ReplyBudget;
use clv_abi::goal::{
    CancelOutcome, CancelResult, CloseResult, CreateOptions, CreatePolicy, DiscardResult,
    FileOptions, GoalDeadline, GoalObservation, GoalStatus, MAX_CHILDREN, MAX_GOAL_RENAMES,
};
use clv_abi::tool::{HOST_TEXT_SEARCH_V1, TEXT_SEARCH_V1, ToolId};
use clv_abi::{ReplyKind, RequestKind, TaskCompletion};

use super::{AgentBinding, LaunchGrant, SHELL_TASK_INDEX, Scheduler, TaskState};
use crate::arch::monotonic_nanos;
use crate::fs::ramfs::{
    DirectoryId, FileGrant, FileGrantMode, GrantArgument, RamFs, RenamePolicy, TaskFs,
};
use crate::ipc::{Message, MessageKind};
use crate::programs::GOAL_MONITOR_TASK_INDEX;

pub(crate) enum DelegationTarget {
    Program { selector: usize, with_file: bool },
    FileTool(ToolId),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum MemberRole {
    Child,
    Agent(AgentBinding),
    /// The original launch handle of the agent funding this helper.
    Helper {
        parent: usize,
    },
}

struct Member {
    handle: usize,
    role: MemberRole,
    replies: ReplyReservation,
    completion: Option<TaskCompletion>,
    host_step: HostStep,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum HostStep {
    Ready,
    AwaitingDecision,
    SearchApproved,
    ToolSubmitted,
}

impl Member {
    fn is_live(&self, handle: usize) -> bool {
        self.handle == handle && self.completion.is_none()
    }

    fn status(&self) -> GoalStatus {
        let child = self.handle;
        match self.completion {
            None => GoalStatus::Running { child },
            Some(TaskCompletion::Exited(0)) => GoalStatus::Completed { child },
            Some(TaskCompletion::Cancelled) => GoalStatus::Cancelled { child: Some(child) },
            Some(completion) => {
                let (kind, value) = completion.kind_and_value();
                GoalStatus::Failed { child, kind, value }
            }
        }
    }
}

pub(super) struct Goal {
    handle: usize,
    options: CreateOptions,
    deadline: GoalDeadline,
    allowed_tools: [Option<ToolId>; 2],
    reads: GoalReads,
    members: [Option<Member>; MAX_CHILDREN],
    checkpoint_child: Option<usize>,
    cancelled_before_launch: bool,
    file_grants: [Option<FileGrant>; MAX_GOAL_RENAMES],
    rename_policy: RenamePolicy,
    require_rename_review: bool,
    rename_limit: usize,
    rename: Option<PendingRenames>,
    publication_report: Option<PublicationObservation>,
    delegation_audit: AuditHistory<EncodedRecordWithTool>,
    reply_audit: AuditHistory<EncodedReplyRecord>,
    rename_audit: AuditHistory<EncodedRenameRecord>,
    publication_audit: PublicationHistory,
}

impl Goal {
    fn new(
        handle: usize,
        policy: CreatePolicy,
        deadline: GoalDeadline,
        reads: GoalReads,
        rename_policy: RenamePolicy,
    ) -> Self {
        Self {
            handle,
            options: policy.options,
            deadline,
            allowed_tools: policy.allowed_tools,
            reads,
            members: [const { None }; MAX_CHILDREN],
            checkpoint_child: None,
            cancelled_before_launch: false,
            file_grants: [const { None }; MAX_GOAL_RENAMES],
            rename_policy,
            require_rename_review: policy.require_rename_review || policy.durable,
            rename_limit: policy.rename_limit,
            rename: None,
            publication_report: None,
            delegation_audit: AuditHistory::default(),
            reply_audit: AuditHistory::default(),
            rename_audit: AuditHistory::default(),
            publication_audit: PublicationHistory::default(),
        }
    }

    pub(super) fn pins_directory(&self, directory: DirectoryId) -> bool {
        self.rename_policy.directory == directory || self.reads.pins_directory(directory)
    }

    fn members(&self) -> impl Iterator<Item = &Member> {
        self.members.iter().flatten()
    }

    fn status(&self) -> GoalStatus {
        // Ties report the first matching member in launch order.
        let representative = self.members().min_by_key(|member| match member.completion {
            None => 0,
            Some(TaskCompletion::Cancelled) => 1,
            Some(TaskCompletion::Exited(0)) => 3,
            Some(_) => 2,
        });
        match representative {
            Some(member) => member.status(),
            None if self.cancelled_before_launch => GoalStatus::Cancelled { child: None },
            None => GoalStatus::Ready,
        }
    }

    fn budget(&self) -> ReplyBudget {
        budget::balance(
            self.options.reply_limit,
            self.members().map(|member| &member.replies),
        )
    }

    fn observation(&self) -> GoalObservation {
        GoalObservation {
            status: self.status(),
            deadline: self.deadline,
            budget: self.budget(),
        }
    }

    fn is_active(&self) -> bool {
        matches!(
            self.status(),
            GoalStatus::Ready | GoalStatus::Running { .. }
        )
    }

    pub(super) fn start(
        &mut self,
        handle: usize,
        child: usize,
        agent: Option<AgentBinding>,
        reply_bytes: Option<usize>,
        request: Option<&Message>,
        fs: &mut RamFs,
    ) -> Option<TaskFs> {
        if self.handle != handle || !self.is_active() || self.checkpoint_child.is_some() {
            return None;
        }
        if agent.is_some()
            && self
                .members()
                .any(|member| matches!(member.role, MemberRole::Agent(_)))
        {
            return None;
        }
        let slot = self.free_slot()?;
        let replies = ReplyReservation::new(self.budget().available, reply_bytes)?;
        let rename_pair = request.is_some_and(|message| {
            message.kind == MessageKind::Request(RequestKind::StageRenamePair)
        });
        let child_fs = match (rename_pair, &self.file_grants) {
            (true, [Some(first), Some(second)]) if agent.is_some() => {
                first.open_for_child(fs, true, Some(second))?
            }
            (true, _) => return None,
            (false, [Some(file), _]) => file.open_for_child(fs, agent.is_some(), None)?,
            (false, [None, _]) => TaskFs::empty(),
        };
        let role = agent.map_or(MemberRole::Child, MemberRole::Agent);
        self.admit(slot, child, role, replies, fs);
        Some(child_fs)
    }

    fn free_slot(&self) -> Option<usize> {
        self.members[..self.options.child_limit]
            .iter()
            .position(Option::is_none)
    }

    fn admit(
        &mut self,
        slot: usize,
        child: usize,
        role: MemberRole,
        replies: ReplyReservation,
        fs: &mut RamFs,
    ) {
        self.members[slot] = Some(Member {
            handle: child,
            role,
            replies,
            completion: None,
            host_step: HostStep::Ready,
        });
        if slot + 1 == self.options.child_limit {
            // Launch slots are never reused. Issued handles keep their own file references.
            self.release_files(fs);
        }
    }

    fn release_files(&mut self, fs: &mut RamFs) {
        for file in self.file_grants.iter_mut().filter_map(Option::take) {
            file.release(fs);
        }
    }

    fn live_member_mut(&mut self, child: usize) -> Option<&mut Member> {
        self.members
            .iter_mut()
            .flatten()
            .find(|member| member.is_live(child))
    }

    pub(super) fn charge_reply(&mut self, child: usize, bytes: usize) -> Option<()> {
        let Some(member) = self.live_member_mut(child) else {
            return Some(());
        };
        let remaining_before = member.replies.remaining();
        let charged = member.replies.charge(bytes);
        let record = ReplyRecord {
            child,
            bytes,
            remaining_before,
            sent: charged.is_some(),
        };
        self.reply_audit.record(record.encode());
        charged
    }

    pub(super) fn finish_child(
        &mut self,
        child: usize,
        completion: TaskCompletion,
        fs: &mut RamFs,
    ) {
        let Some(member) = self.live_member_mut(child) else {
            return;
        };
        let unused = member.replies.release();
        member.completion = Some(completion);
        if let MemberRole::Helper { parent } = member.role
            && let Some(bytes) = unused
            && let Some(parent) = self.live_member_mut(parent)
        {
            parent.replies.refund(bytes);
        }
        if !self.members().any(|member| member.completion.is_none()) {
            self.release_files(fs);
        }
    }
}

impl Scheduler {
    pub(crate) fn create_goal(
        &mut self,
        policy: CreatePolicy,
        files: FileOptions,
        timeout_ms: Option<usize>,
        fs: &mut RamFs,
    ) -> Option<usize> {
        let durable_renames = policy.durable.then_some(policy.rename_limit);
        if self.current != SHELL_TASK_INDEX
            || self.goal.is_some()
            || !(1..=MAX_CHILDREN).contains(&policy.options.child_limit)
            || files.rename.preserve_files.iter().flatten().count() > policy.rename_limit
            || durable_renames.is_some_and(|count| {
                !self
                    .durable_disk
                    .as_ref()
                    .is_some_and(|disk| disk.supports_renames(count))
            })
        {
            return None;
        }
        for id in policy.allowed_tools.into_iter().flatten() {
            if id != HOST_TEXT_SEARCH_V1 {
                crate::programs::file_tool(id)?;
            }
        }
        let handle = self.next_handle;
        let next_handle = handle.checked_add(1)?;
        let caller = &self.tasks[self.current].fs;
        let read_directory = match files.read_directory {
            Some(fd) => Some(caller.directory_for_handle(fd)?),
            None => None,
        };
        let deadline = match timeout_ms {
            Some(ms) => GoalDeadline::after_millis(monotonic_nanos(), ms)?,
            None => GoalDeadline::Unlimited,
        };
        let rename_policy = RenamePolicy::capture(caller, fs, files.rename, durable_renames)?;
        let reads = GoalReads::new(policy.read_limits, read_directory);
        self.next_handle = next_handle;
        self.goal = Some(Goal::new(handle, policy, deadline, reads, rename_policy));
        Some(handle)
    }

    pub(super) fn shell_goal(&self, handle: usize) -> Option<&Goal> {
        if self.current != SHELL_TASK_INDEX {
            return None;
        }
        self.goal.as_ref().filter(|goal| goal.handle == handle)
    }

    pub(crate) fn goal_status(&self, handle: usize) -> Option<(GoalStatus, GoalDeadline)> {
        let goal = self.shell_goal(handle)?;
        Some((goal.status(), goal.deadline))
    }

    pub(crate) fn goal_budget(&self, handle: usize) -> Option<ReplyBudget> {
        Some(self.shell_goal(handle)?.budget())
    }

    pub(crate) fn grant_goal_file(
        &mut self,
        handle: usize,
        fd: usize,
        mode: FileGrantMode,
        argument: Option<GrantArgument>,
        fs: &mut RamFs,
    ) -> Option<()> {
        if self.shell_goal(handle)?.status() != GoalStatus::Ready {
            return None;
        }
        let goal = self.goal.as_mut()?;
        let slot = match &goal.file_grants {
            [None, None] => 0,
            [Some(first), None]
                if goal.rename_limit == MAX_GOAL_RENAMES
                    && first.is_for_rename()
                    && mode == FileGrantMode::StageRename =>
            {
                1
            }
            _ => return None,
        };
        let grant = self.tasks[self.current]
            .fs
            .retain_file(fs, handle, fd, mode, argument)?;
        if goal.file_grants[0]
            .as_ref()
            .is_some_and(|first| first.same_file(&grant))
        {
            grant.release(fs);
            return None;
        }
        goal.file_grants[slot] = Some(grant);
        Some(())
    }

    pub(crate) fn revoke_goal_file(&mut self, handle: usize, fs: &mut RamFs) -> Option<bool> {
        self.shell_goal(handle)?;
        let goal = self.goal.as_mut()?;
        goal.release_files(fs);
        let rename_discarded = goal.discard_granted_rename(fs);
        // A full goal has released its pins. Issued handles still carry the goal ID.
        for task in &mut self.tasks {
            task.fs.revoke_goal_file(fs, handle);
        }
        if rename_discarded {
            self.wake_goal_monitor();
        }
        Some(rename_discarded)
    }

    fn agent_member(&self, handle: usize) -> Option<(&Goal, &Member, AgentBinding)> {
        if self.current != SHELL_TASK_INDEX {
            return None;
        }
        let goal = self.goal.as_ref()?;
        goal.members().find_map(|member| match member.role {
            MemberRole::Agent(binding) if binding.handle == handle => Some((goal, member, binding)),
            _ => None,
        })
    }

    fn current_agent_member(&self) -> Option<(&Goal, &Member, AgentBinding)> {
        let child = self.tasks[self.current].launch.as_ref()?.handle;
        self.live_agent_member(child)
    }

    pub(crate) fn submit_host_request(&mut self, tool: bool) -> Result<usize, BridgeFailure> {
        let (goal, member, binding) = self
            .current_agent_member()
            .ok_or(BridgeFailure::Unavailable)?;
        let pin = binding
            .definition
            .and_then(|definition| DefinitionPin::decode(definition))
            .ok_or(BridgeFailure::Unavailable)?;
        let launch = member.handle;
        let combined = pin.id == FIND_GREETING_V8;
        let host_granted = goal.allowed_tools.contains(&Some(HOST_TEXT_SEARCH_V1));
        let (request, timeout_ns) = match (pin.id, tool) {
            (FIND_GREETING_V5, false) => (HostRequest::MockGreeting, 2_000_000_000),
            (FIND_GREETING_V6, false) => (HostRequest::ModelGreeting, 30_000_000_000),
            (FIND_GREETING_V7, _) if host_granted => (
                HostRequest::SearchFixture {
                    goal: goal.handle as u64,
                },
                10_000_000_000,
            ),
            (FIND_GREETING_V7, _) => return Err(BridgeFailure::ToolNotAllowed),
            (FIND_GREETING_V8, _) => {
                if !host_granted || !goal.allowed_tools.contains(&Some(TEXT_SEARCH_V1.id)) {
                    return Err(BridgeFailure::ToolNotAllowed);
                }
                let (required_step, timeout) = if tool {
                    (HostStep::SearchApproved, 10_000_000_000)
                } else {
                    (HostStep::Ready, 30_000_000_000)
                };
                if member.host_step != required_step {
                    return Err(BridgeFailure::Unavailable);
                }
                (
                    HostRequest::Combined {
                        goal: goal.handle as u64,
                        launch: launch as u64,
                        tool,
                    },
                    timeout,
                )
            }
            _ => return Err(BridgeFailure::Unavailable),
        };
        let limit = monotonic_nanos().saturating_add(timeout_ns);
        let deadline = match goal.deadline {
            GoalDeadline::Unlimited => limit,
            GoalDeadline::At(at) => limit.min(at),
            GoalDeadline::Expired => return Err(BridgeFailure::TimedOut),
        };
        // A separate handle for each combined request lets the bridge reject stale replies.
        let (request_id, next_handle) = if combined {
            (
                self.next_handle,
                self.next_handle
                    .checked_add(1)
                    .ok_or(BridgeFailure::Unavailable)?,
            )
        } else {
            (launch, self.next_handle)
        };
        self.host_bridge
            .as_mut()
            .ok_or(BridgeFailure::Unavailable)?
            .submit(launch, request_id, request, deadline)?;
        self.next_handle = next_handle;
        if combined {
            *self.host_step_mut(launch) = if tool {
                HostStep::ToolSubmitted
            } else {
                HostStep::AwaitingDecision
            };
        }
        Ok(request_id)
    }

    pub(crate) fn poll_host_reply(
        &mut self,
        copy: impl FnOnce(&[u8]) -> bool,
    ) -> Result<usize, BridgeFailure> {
        let (_, member, _) = self
            .current_agent_member()
            .ok_or(BridgeFailure::Unavailable)?;
        let launch = member.handle;
        let awaiting_decision = member.host_step == HostStep::AwaitingDecision;
        let mut search_approved = false;
        let length = self
            .host_bridge
            .as_mut()
            .ok_or(BridgeFailure::Unavailable)?
            .poll(launch, |id, bytes| {
                let copied = copy(bytes);
                search_approved =
                    copied && awaiting_decision && plan_action(bytes, id) == Some(SEARCH_GREETING);
                copied
            })?;
        if search_approved {
            *self.host_step_mut(launch) = HostStep::SearchApproved;
        }
        Ok(length)
    }

    fn host_step_mut(&mut self, launch: usize) -> &mut HostStep {
        &mut self
            .goal
            .as_mut()
            .and_then(|goal| goal.live_member_mut(launch))
            .expect("a host exchange belongs to a live goal member")
            .host_step
    }

    /// The boot-unique child handle identifies both the original goal and its agent.
    fn live_agent_member(&self, child: usize) -> Option<(&Goal, &Member, AgentBinding)> {
        let goal = self.goal.as_ref()?;
        let member = goal.members().find(|member| member.is_live(child))?;
        let MemberRole::Agent(binding) = member.role else {
            return None;
        };
        Some((goal, member, binding))
    }

    pub(crate) fn agent_status(
        &self,
        handle: usize,
    ) -> Option<(AgentStatus, Option<&'static EncodedDefinitionPin>)> {
        let (goal, member, binding) = self.agent_member(handle)?;
        let suspended = member.completion.is_none()
            && self.tasks[self.launch_index(member.handle)?].state == TaskState::Suspended;
        Some((
            AgentStatus {
                goal: goal.handle,
                status: member.status(),
                suspended,
            },
            binding.definition,
        ))
    }

    pub(crate) fn set_agent_suspended(
        &mut self,
        fs: &mut RamFs,
        handle: usize,
        suspended: bool,
    ) -> Option<ControlResult> {
        let (_, member, _) = self.agent_member(handle)?;
        if member.completion.is_some() {
            return Some(ControlResult::AlreadyFinished);
        }
        let child = member.handle;
        let index = self.launch_index(child)?;
        let state = self.tasks[index].state;
        if state != TaskState::Suspended && !state.can_finish(child) {
            return None;
        }
        if suspended {
            self.tasks[index].state = TaskState::Suspended;
        } else if state == TaskState::Suspended {
            if self.has_live_checkpoint(child) {
                self.complete_live_checkpoint(index, fs)?;
            } else {
                self.resume_suspended_child(index, child);
            }
        }
        Some(ControlResult::Applied)
    }

    pub(super) fn agent_resume_grant(&self, agent: usize) -> Option<LaunchGrant> {
        let (_, member, _) = self.agent_member(agent)?;
        let child = member.handle;
        let index = self.launch_index(child)?;
        (member.completion.is_none()
            && self.tasks[index].state == TaskState::Suspended
            && !self.has_live_checkpoint(child))
        .then_some(LaunchGrant::ResumeAgent(child))
    }

    pub(crate) fn resume_granted_agent(&mut self) -> Option<()> {
        let supervisor = self.tasks[self.current].launch.as_ref()?;
        let LaunchGrant::ResumeAgent(child) = supervisor.grant? else {
            return None;
        };
        let (goal, _, binding) = self.live_agent_member(child)?;
        let index = self.launch_index(child)?;
        if self.tasks[index].state != TaskState::Suspended || self.has_live_checkpoint(child) {
            return None;
        }
        let receipt = ResumeReceipt {
            supervisor: supervisor.handle,
            goal: goal.handle,
            agent: binding.handle,
            child,
        }
        .encode();
        let bytes = receipt.as_flattened();
        // Queue the receipt before changing state so a full mailbox permits a retry.
        // Both operations finish in this trap before the parent or agent can run.
        self.send_reply(Some(ReplyKind::AgentResumed), bytes.len(), |reply| {
            reply.copy_from_slice(bytes);
            true
        })?;
        self.resume_suspended_child(index, child);
        Some(())
    }

    fn resume_suspended_child(&mut self, index: usize, child: usize) {
        // A suspended blocking syscall retries at its saved PC after resume.
        self.tasks[index].state = TaskState::Runnable;
        // A later suspension of the same live child must require a fresh grant.
        for launch in self
            .tasks
            .iter_mut()
            .filter_map(|task| task.launch.as_mut())
        {
            if launch.grant == Some(LaunchGrant::ResumeAgent(child)) {
                launch.grant = None;
            }
        }
    }

    pub(crate) fn close_goal(&mut self, handle: usize, fs: &mut RamFs) -> Option<CloseResult> {
        if matches!(
            self.shell_goal(handle)?.status(),
            GoalStatus::Running { .. }
        ) {
            return Some(CloseResult::Busy);
        }
        self.drop_goal(fs);
        Some(CloseResult::Closed)
    }

    pub(crate) fn cancel_goal(&mut self, handle: usize, fs: &mut RamFs) -> Option<CancelOutcome> {
        if self
            .shell_goal(handle)?
            .checkpoint_child
            .is_some_and(|child| self.has_live_checkpoint(child))
        {
            self.consume_checkpoint()?;
        }
        self.stop_goal_work(fs, PublicationOutcome::DiscardedByCancel)
    }

    /// Check on trap entry, before admitting a syscall or scheduling more user work.
    /// Returns true when the interrupted task was itself stopped by the deadline.
    pub(crate) fn expire_goal_if_due(&mut self, fs: &mut RamFs) -> bool {
        let Some(goal) = &mut self.goal else {
            return false;
        };
        let GoalDeadline::At(at) = goal.deadline else {
            return false;
        };
        if monotonic_nanos() < at {
            return false;
        }
        goal.deadline = GoalDeadline::Expired;
        self.stop_goal_work(fs, PublicationOutcome::DiscardedByExpiry)
            .expect("every live goal member has its original task slot");
        self.tasks[self.current].state == TaskState::Stopped
    }

    fn stop_goal_work(
        &mut self,
        fs: &mut RamFs,
        outcome: PublicationOutcome,
    ) -> Option<CancelOutcome> {
        let goal = self.goal.as_mut()?;
        let execution = match goal.status() {
            GoalStatus::Ready => {
                goal.cancelled_before_launch = true;
                CancelResult::Cancelled
            }
            GoalStatus::Running { .. } => {
                self.cancel_running_children(fs, |_| true)?;
                CancelResult::Cancelled
            }
            GoalStatus::Cancelled { .. } => CancelResult::Cancelled,
            GoalStatus::Completed { .. } | GoalStatus::Failed { .. } => {
                CancelResult::AlreadyFinished
            }
        };
        let goal = self.goal.as_mut()?;
        // Stopping the members closed their descriptors. A ready goal can still hold its pins.
        goal.release_files(fs);
        let discarded = goal.release_transaction(fs, Some(outcome));
        self.revoke_read_denial_watch(fs);
        self.wake_goal_monitor();
        Some(CancelOutcome {
            execution,
            rename_discarded: discarded == DiscardResult::Discarded,
        })
    }

    pub(super) fn release_shell_goal(&mut self, fs: &mut RamFs) {
        if self.tasks[GOAL_MONITOR_TASK_INDEX].state != TaskState::Stopped {
            self.stop_task(GOAL_MONITOR_TASK_INDEX, fs, TaskCompletion::Cancelled);
        }
        self.read_denial_watch = None;
        let _ = self.cancel_running_children(fs, |_| true);
        self.drop_goal(fs);
    }

    fn drop_goal(&mut self, fs: &mut RamFs) {
        let Some(goal) = &mut self.goal else {
            return;
        };
        let handle = goal.handle;
        goal.release_files(fs);
        goal.close_transaction(fs);
        self.revoke_read_denial_watch(fs);
        self.wake_goal_monitor();
        self.goal = None;
        if self.tasks[GOAL_MONITOR_TASK_INDEX].state == TaskState::WaitingForGoal(handle) {
            self.stop_task(GOAL_MONITOR_TASK_INDEX, fs, TaskCompletion::Cancelled);
        }
    }

    pub(super) fn cancel_delegated_helpers(&mut self, parent: usize, fs: &mut RamFs) {
        let _ = self.cancel_running_children(fs, |role| role == MemberRole::Helper { parent });
    }

    fn cancel_running_children(
        &mut self,
        fs: &mut RamFs,
        include: impl Fn(MemberRole) -> bool,
    ) -> Option<()> {
        let goal = self.goal.as_ref()?;
        let mut targets = [None; MAX_CHILDREN];
        // Resolve the original launch handles before stopping any of the selected members.
        let live = goal
            .members()
            .filter(|member| member.completion.is_none() && include(member.role));
        for (target, member) in targets.iter_mut().zip(live) {
            let index = self.launch_index(member.handle)?;
            if self.tasks[index].state == TaskState::Stopped {
                return None;
            }
            *target = Some(index);
        }
        for index in targets.into_iter().flatten() {
            self.stop_task(index, fs, TaskCompletion::Cancelled);
        }
        Some(())
    }
}
