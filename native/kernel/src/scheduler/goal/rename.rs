use clv_abi::goal::{CommitResult, GoalStatus, ReviewToken};

use super::{Goal, Scheduler};
use crate::fs::ramfs::{RamFs, StagedRename};

pub(super) struct PendingRename {
    change: StagedRename,
    token: Option<ReviewToken>,
}

impl PendingRename {
    fn review(&mut self, next_handle: &mut usize) -> Option<ReviewToken> {
        if self.token.is_none() {
            let token = ReviewToken::new(*next_handle)?;
            *next_handle = token.get().checked_add(1)?;
            self.token = Some(token);
        }
        self.token
    }
}

impl Goal {
    pub(super) fn discard_rename(&mut self, fs: &mut RamFs) {
        if let Some(rename) = self.rename.take() {
            rename.change.discard(fs);
        }
    }
}

impl Scheduler {
    pub(crate) fn stage_goal_rename(
        &mut self,
        handle: usize,
        fs: &mut RamFs,
        fd: usize,
        new_name: &[u8],
    ) -> Option<usize> {
        let goal = self.shell_goal(handle)?;
        if goal.rename.is_some() || !goal.is_active() {
            return None;
        }
        let slot = &mut self.goal.as_mut()?.rename;
        let change = self.tasks[self.current].fs.stage_rename(fs, fd, new_name)?;
        *slot = Some(PendingRename {
            change,
            token: None,
        });
        Some(0)
    }

    pub(crate) fn read_goal_diff(
        &mut self,
        handle: usize,
        review: bool,
        deliver: impl FnOnce(&[u8]) -> bool,
    ) -> Option<(usize, usize, Option<ReviewToken>)> {
        let goal = self.shell_goal(handle)?;
        let can_issue = review && matches!(goal.status(), GoalStatus::Completed { .. });
        let Some(pending) = &mut self.goal.as_mut()?.rename else {
            return deliver(&[]).then_some((0, 0, None));
        };
        let (names, source_len) = pending.change.diff();
        let count = names.len();
        if !deliver(names) {
            return None;
        }
        let token = if can_issue {
            pending.review(&mut self.next_handle)
        } else {
            None
        };
        Some((count, source_len, token))
    }

    pub(crate) fn commit_goal(
        &mut self,
        handle: usize,
        token: ReviewToken,
        fs: &mut RamFs,
    ) -> Option<CommitResult> {
        if !matches!(self.goal_status(handle)?, GoalStatus::Completed { .. }) {
            return None;
        }
        let goal = self.goal.as_mut()?;
        let pending = goal.rename.as_mut()?;
        if pending.token != Some(token) {
            return None;
        }
        pending.token = None;
        let result = pending.change.publish(fs)?;
        if result == CommitResult::Committed {
            goal.discard_rename(fs);
        }
        Some(result)
    }
}
