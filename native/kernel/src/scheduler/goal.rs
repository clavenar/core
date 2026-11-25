use clv_abi::TaskCompletion;
use clv_abi::goal::{CancelResult, CloseResult, GoalStatus};

use super::{SHELL_TASK_INDEX, Scheduler, TaskState};
use crate::fs::ramfs::RamFs;

pub(super) struct Goal {
    handle: usize,
    status: GoalStatus,
}

impl Goal {
    pub(super) fn start(&mut self, handle: usize, child: usize) -> Option<()> {
        if self.handle != handle || self.status != GoalStatus::Ready {
            return None;
        }
        self.status = GoalStatus::Running { child };
        Some(())
    }

    pub(super) fn finish_child(&mut self, child: usize, completion: TaskCompletion) {
        if !matches!(self.status, GoalStatus::Running { child: member } if member == child) {
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

    pub(super) fn is_running(&self) -> bool {
        matches!(self.status, GoalStatus::Running { .. })
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
            GoalStatus::Running { child } => {
                let index = self.launch_index(child)?;
                if self.tasks[index].state == TaskState::Stopped {
                    return None;
                }
                self.stop_task(index, fs, TaskCompletion::Cancelled);
            }
            GoalStatus::Cancelled { .. } => {}
            GoalStatus::Completed { .. } | GoalStatus::Failed { .. } => {
                return Some(CancelResult::AlreadyFinished);
            }
        }
        Some(CancelResult::Cancelled)
    }
}
