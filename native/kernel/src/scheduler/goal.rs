mod audit;
mod budget;
mod delegation;
mod reads;

use self::audit::AuditHistory;
use self::budget::ReplyReservation;
use self::reads::ReadBudget;
use clv_abi::TaskCompletion;
use clv_abi::agent::{AgentStatus, ControlResult, EncodedDefinitionPin};
use clv_abi::audit::{EncodedRecordWithCaller, EncodedReplyRecord, ReplyRecord};
use clv_abi::budget::ReplyBudget;
use clv_abi::goal::{
    CancelResult, CloseResult, CreateOptions, FileRecipient, GoalStatus, MAX_CHILDREN, ReadLimits,
};

use super::{AgentBinding, SHELL_TASK_INDEX, Scheduler, TaskState};
use crate::fs::ramfs::{FileGrant, GrantArgument, RamFs, TaskFs};

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
    reads: ReadBudget,
    members: [Option<Member>; MAX_CHILDREN],
    cancelled_before_launch: bool,
    file: Option<FileGrant>,
    delegation_audit: AuditHistory<EncodedRecordWithCaller>,
    reply_audit: AuditHistory<EncodedReplyRecord>,
}

impl Goal {
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

    pub(super) fn start(
        &mut self,
        handle: usize,
        child: usize,
        agent: Option<AgentBinding>,
        reply_bytes: Option<usize>,
        fs: &mut RamFs,
    ) -> Option<TaskFs> {
        if self.handle != handle
            || !matches!(
                self.status(),
                GoalStatus::Ready | GoalStatus::Running { .. }
            )
        {
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
        let child_fs = match &self.file {
            Some(file) => file.open_for_child(fs, agent.is_some())?,
            None => TaskFs::empty(),
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
        });
        if slot + 1 == self.options.child_limit {
            self.release_file(fs);
        }
    }

    fn release_file(&mut self, fs: &mut RamFs) {
        if let Some(file) = self.file.take() {
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
            self.release_file(fs);
        }
    }
}

impl Scheduler {
    pub(crate) fn create_goal(
        &mut self,
        options: CreateOptions,
        read_limits: ReadLimits,
    ) -> Option<usize> {
        if self.current != SHELL_TASK_INDEX
            || self.goal.is_some()
            || !(1..=MAX_CHILDREN).contains(&options.child_limit)
        {
            return None;
        }
        let handle = self.next_handle;
        self.next_handle = handle.checked_add(1)?;
        self.goal = Some(Goal {
            handle,
            options,
            reads: ReadBudget::new(read_limits),
            members: [const { None }; MAX_CHILDREN],
            cancelled_before_launch: false,
            file: None,
            delegation_audit: AuditHistory::default(),
            reply_audit: AuditHistory::default(),
        });
        Some(handle)
    }

    fn shell_goal(&self, handle: usize) -> Option<&Goal> {
        if self.current != SHELL_TASK_INDEX {
            return None;
        }
        self.goal.as_ref().filter(|goal| goal.handle == handle)
    }

    pub(crate) fn goal_status(&self, handle: usize) -> Option<GoalStatus> {
        Some(self.shell_goal(handle)?.status())
    }

    pub(crate) fn goal_budget(&self, handle: usize) -> Option<ReplyBudget> {
        Some(self.shell_goal(handle)?.budget())
    }

    pub(crate) fn grant_goal_file(
        &mut self,
        handle: usize,
        fd: usize,
        recipient: Option<FileRecipient>,
        argument: Option<GrantArgument>,
        fs: &mut RamFs,
    ) -> Option<()> {
        if self.goal_status(handle)? != GoalStatus::Ready {
            return None;
        }
        let goal = self.goal.as_mut()?;
        if goal.file.is_some() {
            return None;
        }
        goal.file = Some(
            self.tasks[self.current]
                .fs
                .retain_file(fs, handle, fd, recipient, argument)?,
        );
        Some(())
    }

    pub(crate) fn revoke_goal_file(&mut self, handle: usize, fs: &mut RamFs) -> Option<()> {
        self.shell_goal(handle)?;
        self.goal.as_mut()?.release_file(fs);
        // A full goal has already released its pin. Its issued handles still carry its ID.
        for task in &mut self.tasks {
            task.fs.revoke_goal_file(fs, handle);
        }
        Some(())
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
        handle: usize,
        suspended: bool,
    ) -> Option<ControlResult> {
        let (_, member, _) = self.agent_member(handle)?;
        if member.completion.is_some() {
            return Some(ControlResult::AlreadyFinished);
        }
        let index = self.launch_index(member.handle)?;
        let state = &mut self.tasks[index].state;
        if *state != TaskState::Suspended && !state.can_finish() {
            return None;
        }
        // A suspended blocking syscall retries at its saved PC after resume.
        if suspended || *state == TaskState::Suspended {
            *state = TaskState::for_agent(suspended);
        }
        Some(ControlResult::Applied)
    }

    pub(crate) fn close_goal(&mut self, handle: usize, fs: &mut RamFs) -> Option<CloseResult> {
        if matches!(self.goal_status(handle)?, GoalStatus::Running { .. }) {
            return Some(CloseResult::Busy);
        }
        self.drop_goal(fs);
        Some(CloseResult::Closed)
    }

    pub(crate) fn cancel_goal(&mut self, handle: usize, fs: &mut RamFs) -> Option<CancelResult> {
        match self.goal_status(handle)? {
            GoalStatus::Ready => {
                let goal = self.goal.as_mut()?;
                goal.release_file(fs);
                goal.cancelled_before_launch = true;
            }
            GoalStatus::Running { .. } => self.cancel_running_children(fs, |_| true)?,
            GoalStatus::Cancelled { .. } => {}
            GoalStatus::Completed { .. } | GoalStatus::Failed { .. } => {
                return Some(CancelResult::AlreadyFinished);
            }
        }
        Some(CancelResult::Cancelled)
    }

    pub(super) fn release_shell_goal(&mut self, fs: &mut RamFs) {
        let _ = self.cancel_running_children(fs, |_| true);
        self.drop_goal(fs);
    }

    fn drop_goal(&mut self, fs: &mut RamFs) {
        if let Some(mut goal) = self.goal.take() {
            goal.release_file(fs);
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
