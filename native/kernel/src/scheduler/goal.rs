use clv_abi::goal::{CloseResult, GoalStatus};

use super::{SHELL_TASK_INDEX, Scheduler, TaskCompletion};

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
}
