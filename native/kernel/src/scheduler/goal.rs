use clv_abi::TaskCompletion;
use clv_abi::agent::AgentStatus;
use clv_abi::goal::{CancelResult, CloseResult, GoalStatus};

use super::{SHELL_TASK_INDEX, Scheduler, TaskState};
use crate::fs::ramfs::RamFs;

pub(super) struct Goal {
    handle: usize,
    agent: Option<usize>,
    status: GoalStatus,
}

impl Goal {
    pub(super) fn start(
        &mut self,
        handle: usize,
        child: usize,
        agent: Option<usize>,
    ) -> Option<()> {
        if self.handle != handle || self.status != GoalStatus::Ready {
            return None;
        }
        self.agent = agent;
        self.status = GoalStatus::Running { child };
        Some(())
    }

    pub(super) fn finish_child(&mut self, child: usize, completion: TaskCompletion) {
        if self.running_child() != Some(child) {
            return;
        }
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
    pub(crate) fn create_goal(&mut self) -> Option<usize> {
        if self.current != SHELL_TASK_INDEX || self.goal.is_some() {
            return None;
        }
        let handle = self.next_handle;
        self.next_handle = handle.checked_add(1)?;
        self.goal = Some(Goal {
            handle,
            agent: None,
            status: GoalStatus::Ready,
        });
        Some(handle)
    }

    pub(crate) fn goal_status(&self, handle: usize) -> Option<GoalStatus> {
        if self.current != SHELL_TASK_INDEX {
            return None;
        }
        self.goal
            .as_ref()
            .filter(|goal| goal.handle == handle)
            .map(|goal| goal.status)
    }

    pub(crate) fn agent_status(&self, handle: usize) -> Option<AgentStatus> {
        let goal = self.goal.as_ref()?;
        if self.current != SHELL_TASK_INDEX || goal.agent != Some(handle) {
            return None;
        }
        Some(AgentStatus {
            goal: goal.handle,
            status: goal.status,
        })
    }

    pub(crate) fn close_goal(&mut self, handle: usize) -> Option<CloseResult> {
        if matches!(self.goal_status(handle)?, GoalStatus::Running { .. }) {
            return Some(CloseResult::Busy);
        }
        self.goal = None;
        Some(CloseResult::Closed)
    }

    pub(crate) fn cancel_goal(&mut self, handle: usize, fs: &mut RamFs) -> Option<CancelResult> {
        match self.goal_status(handle)? {
            GoalStatus::Ready => self.goal.as_mut()?.status = GoalStatus::Cancelled { child: None },
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
        self.goal = None;
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
