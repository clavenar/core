use clv_abi::audit::{RenameDenial, RenameRecord};
use clv_abi::goal::{CommitResult, GoalStatus, ReviewToken};
use clv_abi::{LaunchParent, PATH_MAX_BYTES};

use super::{Goal, Scheduler};
use crate::fs::ramfs::{RamFs, StagedRename, TaskFs};

pub(super) struct PendingRename {
    change: StagedRename,
    token: Option<ReviewToken>,
    from_file_grant: bool,
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
    fn stage_rename(
        &mut self,
        fs: &mut RamFs,
        task_fs: &TaskFs,
        fd: usize,
        name: &[u8],
        caller: LaunchParent,
    ) -> Result<(), RenameDenial> {
        if !self.is_active() {
            return Err(RenameDenial::GoalInactive);
        }
        if self.rename.is_some() {
            return Err(RenameDenial::AlreadyStaged);
        }
        let from_file_grant = matches!(caller, LaunchParent::Agent(_));
        let change = task_fs.stage_rename(fs, fd, name, from_file_grant.then_some(self.handle))?;
        self.rename = Some(PendingRename {
            change,
            token: None,
            from_file_grant,
        });
        Ok(())
    }

    pub(super) fn discard_granted_rename(&mut self, fs: &mut RamFs) {
        if let Some(rename) = self.rename.take_if(|rename| rename.from_file_grant) {
            rename.change.discard(fs);
        }
    }

    pub(super) fn release_transaction(&mut self, fs: &mut RamFs) {
        if let Some(rename) = self.rename.take() {
            rename.change.discard(fs);
        }
        if let Some(input) = self.preserved_file.take() {
            input.release(fs);
        }
    }
}

impl Scheduler {
    pub(crate) fn stage_goal_rename(
        &mut self,
        handle: usize,
        fs: &mut RamFs,
        fd: usize,
        length: usize,
        copy: impl FnOnce(&mut [u8]) -> bool,
    ) -> Option<usize> {
        self.shell_goal(handle)?;
        self.stage_rename(fs, fd, LaunchParent::Shell, length, copy)
    }

    pub(crate) fn stage_agent_rename(
        &mut self,
        fs: &mut RamFs,
        fd: usize,
        length: usize,
        copy: impl FnOnce(&mut [u8]) -> bool,
    ) -> Option<usize> {
        let (_, member, _) = self.current_agent_member()?;
        self.stage_rename(fs, fd, LaunchParent::Agent(member.handle), length, copy)
    }

    fn stage_rename(
        &mut self,
        fs: &mut RamFs,
        fd: usize,
        caller: LaunchParent,
        length: usize,
        copy: impl FnOnce(&mut [u8]) -> bool,
    ) -> Option<usize> {
        let goal = self.goal.as_mut()?;
        let mut buffer = [0; PATH_MAX_BYTES];
        let name = match buffer.get_mut(..length) {
            Some(name) => copy(name)
                .then_some(&*name)
                .ok_or(RenameDenial::InvalidBuffer),
            None => Err(RenameDenial::InvalidBasename),
        };
        let outcome = name
            .and_then(|name| goal.stage_rename(fs, &self.tasks[self.current].fs, fd, name, caller));
        goal.rename_audit.record(
            RenameRecord {
                caller,
                basename: name.ok(),
                outcome,
            }
            .encode(),
        );
        outcome.ok().map(|()| 0)
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
        let result = pending.change.publish(fs, goal.preserved_file.as_ref())?;
        if result == CommitResult::Committed {
            goal.release_transaction(fs);
        }
        Some(result)
    }
}
