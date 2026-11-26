mod budget;

use self::budget::ReplyReservation;
use clv_abi::TaskCompletion;
use clv_abi::agent::{AgentStatus, ControlResult};
use clv_abi::budget::ReplyBudget;
use clv_abi::goal::{CancelResult, CloseResult, GoalStatus, MAX_CHILDREN};

use super::{SHELL_TASK_INDEX, Scheduler, TaskState};
use crate::fs::ramfs::{FileGrant, RamFs, TaskFs};

struct Member {
    handle: usize,
    agent: Option<usize>,
    replies: ReplyReservation,
    completion: Option<TaskCompletion>,
}

impl Member {
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
    child_limit: usize,
    reply_limit: Option<usize>,
    members: [Option<Member>; MAX_CHILDREN],
    cancelled_before_launch: bool,
    file: Option<FileGrant>,
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
            self.reply_limit,
            self.members().map(|member| &member.replies),
        )
    }

    pub(super) fn start(
        &mut self,
        handle: usize,
        child: usize,
        agent: Option<usize>,
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
        if agent.is_some() && self.members().any(|member| member.agent.is_some()) {
            return None;
        }
        let slot = self.members[..self.child_limit]
            .iter()
            .position(Option::is_none)?;
        let replies = ReplyReservation::new(self.budget().available, reply_bytes)?;
        let child_fs = match &self.file {
            Some(file) => file.open_for_child(fs)?,
            None => TaskFs::empty(),
        };
        self.members[slot] = Some(Member {
            handle: child,
            agent,
            replies,
            completion: None,
        });
        if slot + 1 == self.child_limit {
            self.release_file(fs);
        }
        Some(child_fs)
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
            .find(|member| member.handle == child && member.completion.is_none())
    }

    pub(super) fn charge_reply(&mut self, child: usize, bytes: usize) -> Option<()> {
        if let Some(member) = self.live_member_mut(child) {
            member.replies.charge(bytes)?;
        }
        Some(())
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
        member.replies.release();
        member.completion = Some(completion);
        if !self.members().any(|member| member.completion.is_none()) {
            self.release_file(fs);
        }
    }
}

impl Scheduler {
    pub(crate) fn create_goal(
        &mut self,
        child_limit: usize,
        reply_limit: Option<usize>,
    ) -> Option<usize> {
        if self.current != SHELL_TASK_INDEX
            || self.goal.is_some()
            || !(1..=MAX_CHILDREN).contains(&child_limit)
        {
            return None;
        }
        let handle = self.next_handle;
        self.next_handle = handle.checked_add(1)?;
        self.goal = Some(Goal {
            handle,
            child_limit,
            reply_limit,
            members: [const { None }; MAX_CHILDREN],
            cancelled_before_launch: false,
            file: None,
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
        fs: &mut RamFs,
    ) -> Option<()> {
        if self.goal_status(handle)? != GoalStatus::Ready {
            return None;
        }
        let goal = self.goal.as_mut()?;
        if goal.file.is_some() {
            return None;
        }
        goal.file = Some(self.tasks[self.current].fs.retain_file(fs, fd)?);
        Some(())
    }

    fn agent_member(&self, handle: usize) -> Option<(&Goal, &Member)> {
        if self.current != SHELL_TASK_INDEX {
            return None;
        }
        let goal = self.goal.as_ref()?;
        let member = goal.members().find(|member| member.agent == Some(handle))?;
        Some((goal, member))
    }

    pub(crate) fn agent_status(&self, handle: usize) -> Option<AgentStatus> {
        let (goal, member) = self.agent_member(handle)?;
        let suspended = member.completion.is_none()
            && self.tasks[self.launch_index(member.handle)?].state == TaskState::Suspended;
        Some(AgentStatus {
            goal: goal.handle,
            status: member.status(),
            suspended,
        })
    }

    pub(crate) fn set_agent_suspended(
        &mut self,
        handle: usize,
        suspended: bool,
    ) -> Option<ControlResult> {
        let (_, member) = self.agent_member(handle)?;
        if member.completion.is_some() {
            return Some(ControlResult::AlreadyFinished);
        }
        let index = self.launch_index(member.handle)?;
        let state = &mut self.tasks[index].state;
        if !matches!(state, TaskState::Runnable | TaskState::Suspended) {
            return None;
        }
        *state = TaskState::for_agent(suspended);
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
            GoalStatus::Running { .. } => self.cancel_running_children(fs)?,
            GoalStatus::Cancelled { .. } => {}
            GoalStatus::Completed { .. } | GoalStatus::Failed { .. } => {
                return Some(CancelResult::AlreadyFinished);
            }
        }
        Some(CancelResult::Cancelled)
    }

    pub(super) fn release_shell_goal(&mut self, fs: &mut RamFs) {
        let _ = self.cancel_running_children(fs);
        self.drop_goal(fs);
    }

    fn drop_goal(&mut self, fs: &mut RamFs) {
        if let Some(mut goal) = self.goal.take() {
            goal.release_file(fs);
        }
    }

    fn cancel_running_children(&mut self, fs: &mut RamFs) -> Option<()> {
        let goal = self.goal.as_ref()?;
        let mut targets = [None; MAX_CHILDREN];
        // Resolve every live launch before stopping any member.
        let live = goal.members().filter(|member| member.completion.is_none());
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
