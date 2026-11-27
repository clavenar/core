use clv_abi::audit::{PublicationOutcome, RenameDenial, RenameRecord};
use clv_abi::goal::{
    CommitResult, DiscardResult, GoalStatus, PublicationError, ReviewToken, TimedRenameReview,
    TimedReviewToken,
};
use clv_abi::{LaunchParent, PATH_MAX_BYTES};

use super::{Goal, Scheduler};
use crate::arch::monotonic_nanos;
use crate::fs::ramfs::{RamFs, RenamePolicy, StagedRename, TaskFs};

const REVIEW_LIFETIME_NS: u64 = 60 * 1_000_000_000;

fn review_is_live(review: TimedReviewToken, now_ns: u64) -> bool {
    let issued_at_ns = review.deadline_ns - REVIEW_LIFETIME_NS;
    (issued_at_ns..review.deadline_ns).contains(&now_ns)
}

pub(super) struct PendingRename {
    change: StagedRename,
    review_token: Option<TimedReviewToken>,
    from_file_grant: bool,
}

impl PendingRename {
    fn review(
        &mut self,
        fs: &RamFs,
        policy: &RenamePolicy,
        next_handle: &mut usize,
    ) -> Result<TimedReviewToken, PublicationError> {
        if let Err(reason) = self.change.check(fs, policy) {
            self.review_token = None;
            return Err(reason);
        }
        let now_ns = monotonic_nanos();
        if let Some(review) = self
            .review_token
            .filter(|&review| review_is_live(review, now_ns))
        {
            return Ok(review);
        }
        self.review_token = None;
        let deadline_ns = now_ns
            .checked_add(REVIEW_LIFETIME_NS)
            .ok_or(PublicationError::Unavailable)?;
        let token = ReviewToken::new(*next_handle).ok_or(PublicationError::Unavailable)?;
        *next_handle = token
            .get()
            .checked_add(1)
            .ok_or(PublicationError::Unavailable)?;
        let review = TimedReviewToken { token, deadline_ns };
        self.review_token = Some(review);
        Ok(review)
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
        let change = task_fs.stage_rename(
            fs,
            fd,
            name,
            from_file_grant.then_some(self.handle),
            &self.rename_policy,
        )?;
        self.rename = Some(PendingRename {
            change,
            review_token: None,
            from_file_grant,
        });
        Ok(())
    }

    fn discard_rename(
        &mut self,
        fs: &mut RamFs,
        outcome: Option<PublicationOutcome>,
    ) -> DiscardResult {
        let Some(rename) = self.rename.take() else {
            return DiscardResult::Empty;
        };
        if let Some(outcome) = outcome {
            self.publication_audit.record(fs, &rename.change, outcome);
        }
        rename.change.discard(fs);
        DiscardResult::Discarded
    }

    pub(super) fn discard_granted_rename(&mut self, fs: &mut RamFs) {
        if self
            .rename
            .as_ref()
            .is_some_and(|rename| rename.from_file_grant)
        {
            self.discard_rename(fs, None);
        }
    }

    pub(super) fn release_transaction(
        &mut self,
        fs: &mut RamFs,
        outcome: Option<PublicationOutcome>,
    ) -> DiscardResult {
        let discarded = self.discard_rename(fs, outcome);
        self.rename_policy.release_input(fs);
        discarded
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
        fs: &RamFs,
        deliver: impl FnOnce(&[u8]) -> bool,
    ) -> Option<(usize, usize, TimedRenameReview)> {
        let goal = self.shell_goal(handle)?;
        let can_review = review && matches!(goal.status(), GoalStatus::Completed { .. });
        let goal = self.goal.as_mut()?;
        let Some(pending) = &mut goal.rename else {
            return deliver(&[]).then_some((0, 0, TimedRenameReview::Preview));
        };
        let (names, source_len) = pending.change.diff();
        let count = names.len();
        if !deliver(names) {
            return None;
        }
        let result = if can_review {
            match pending.review(fs, &goal.rename_policy, &mut self.next_handle) {
                Ok(review) => TimedRenameReview::Ready(review),
                Err(reason) => TimedRenameReview::Blocked(reason),
            }
        } else {
            TimedRenameReview::Preview
        };
        Some((count, source_len, result))
    }

    pub(crate) fn discard_goal_rename(
        &mut self,
        handle: usize,
        fs: &mut RamFs,
    ) -> Option<DiscardResult> {
        self.shell_goal(handle)?;
        let goal = self.goal.as_mut()?;
        Some(goal.discard_rename(fs, Some(PublicationOutcome::Discarded)))
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
        let review = pending
            .review_token
            .take_if(|review| review.token == token)?;
        let result = if review_is_live(review, monotonic_nanos()) {
            match pending.change.publish(fs, &goal.rename_policy) {
                Ok(()) => Some(CommitResult::Committed),
                Err(PublicationError::Conflict) => Some(CommitResult::Conflict),
                Err(PublicationError::PostconditionFailed) => {
                    Some(CommitResult::PostconditionFailed)
                }
                Err(PublicationError::Unavailable) => None,
            }
        } else {
            Some(CommitResult::ReviewExpired)
        };
        let outcome = result.map_or(PublicationOutcome::Unavailable, PublicationOutcome::Commit);
        goal.publication_audit.record(fs, &pending.change, outcome);
        if result == Some(CommitResult::Committed) {
            goal.release_transaction(fs, None);
        }
        result
    }
}
