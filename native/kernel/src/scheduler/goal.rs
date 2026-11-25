mod budget;

use self::budget::ReplyAccount;
use clv_abi::TaskCompletion;
use clv_abi::agent::{AgentStatus, ControlResult};
use clv_abi::budget::ReplyBudget;
use clv_abi::goal::{CancelResult, CloseResult, GoalStatus};

use super::{SHELL_TASK_INDEX, Scheduler, TaskState};
use crate::fs::ramfs::{FileGrant, RamFs, TaskFs};

pub(super) struct Goal {
    handle: usize,
    agent: Option<usize>,
    status: GoalStatus,
    replies: ReplyAccount,
    file: Option<FileGrant>,
}

impl Goal {
    pub(super) fn start(
        &mut self,
        handle: usize,
        child: usize,
        agent: Option<usize>,
        reply_bytes: Option<usize>,
        fs: &mut RamFs,
    ) -> Option<TaskFs> {
        if self.handle != handle || self.status != GoalStatus::Ready {
            return None;
        }
        let replies = self.replies.with_reservation(reply_bytes)?;
        let child_fs = match &self.file {
            Some(file) => file.open_for_child(fs)?,
            None => TaskFs::empty(),
        };
        // The child now owns its handle. All fallible launch checks have passed.
        self.release_file(fs);
        self.replies = replies;
        self.agent = agent;
        self.status = GoalStatus::Running { child };
        Some(child_fs)
    }

    fn release_file(&mut self, fs: &mut RamFs) {
        if let Some(file) = self.file.take() {
            file.release(fs);
        }
    }

    pub(super) fn charge_reply(&mut self, child: usize, bytes: usize) -> Option<()> {
        if self.running_child() == Some(child) {
            self.replies.charge(bytes)?;
        }
        Some(())
    }

    pub(super) fn finish_child(&mut self, child: usize, completion: TaskCompletion) {
        if self.running_child() != Some(child) {
            return;
        }
        self.replies.release();
        self.status = match completion {
            TaskCompletion::Exited(0) => GoalStatus::Completed { child },
            TaskCompletion::Cancelled => GoalStatus::Cancelled { child: Some(child) },
            _ => {
                let (kind, value) = completion.kind_and_value();
                GoalStatus::Failed { child, kind, value }
            }
        };
    }

    fn running_child(&self) -> Option<usize> {
        match self.status {
            GoalStatus::Running { child } => Some(child),
            _ => None,
        }
    }
}

impl Scheduler {
    pub(crate) fn create_goal(&mut self, reply_bytes: Option<usize>) -> Option<usize> {
        if self.current != SHELL_TASK_INDEX || self.goal.is_some() {
            return None;
        }
        let handle = self.next_handle;
        self.next_handle = handle.checked_add(1)?;
        self.goal = Some(Goal {
            handle,
            agent: None,
            status: GoalStatus::Ready,
            replies: ReplyAccount::new(reply_bytes),
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
        Some(self.shell_goal(handle)?.status)
    }

    pub(crate) fn goal_budget(&self, handle: usize) -> Option<ReplyBudget> {
        Some(self.shell_goal(handle)?.replies.snapshot())
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

    fn goal_for_agent(&self, handle: usize) -> Option<&Goal> {
        let goal = self.goal.as_ref()?;
        (self.current == SHELL_TASK_INDEX && goal.agent == Some(handle)).then_some(goal)
    }

    pub(crate) fn agent_status(&self, handle: usize) -> Option<AgentStatus> {
        let goal = self.goal_for_agent(handle)?;
        let suspended = match goal.running_child() {
            Some(child) => self.tasks[self.launch_index(child)?].state == TaskState::Suspended,
            None => false,
        };
        Some(AgentStatus {
            goal: goal.handle,
            status: goal.status,
            suspended,
        })
    }

    pub(crate) fn set_agent_suspended(
        &mut self,
        handle: usize,
        suspended: bool,
    ) -> Option<ControlResult> {
        let Some(child) = self.goal_for_agent(handle)?.running_child() else {
            return Some(ControlResult::AlreadyFinished);
        };
        let index = self.launch_index(child)?;
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
                goal.status = GoalStatus::Cancelled { child: None };
            }
            GoalStatus::Running { child } => self.cancel_child_launch(child, fs)?,
            GoalStatus::Cancelled { .. } => {}
            GoalStatus::Completed { .. } | GoalStatus::Failed { .. } => {
                return Some(CancelResult::AlreadyFinished);
            }
        }
        Some(CancelResult::Cancelled)
    }

    pub(super) fn release_shell_goal(&mut self, fs: &mut RamFs) {
        if let Some(child) = self.goal.as_ref().and_then(Goal::running_child) {
            self.cancel_child_launch(child, fs);
        }
        self.drop_goal(fs);
    }

    fn drop_goal(&mut self, fs: &mut RamFs) {
        if let Some(mut goal) = self.goal.take() {
            goal.release_file(fs);
        }
    }

    fn cancel_child_launch(&mut self, child: usize, fs: &mut RamFs) -> Option<()> {
        let index = self.launch_index(child)?;
        if self.tasks[index].state == TaskState::Stopped {
            return None;
        }
        self.stop_task(index, fs, TaskCompletion::Cancelled);
        Some(())
    }
}
