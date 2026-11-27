use clv_abi::audit::{
    AUDIT_CAPACITY, EncodedPublicationRecord, PublicationAuditView, PublicationOutcome,
    PublicationReviewer,
};
use clv_abi::goal::MAX_GOAL_RENAMES;

use super::DelegationAudit;
use crate::fs::ramfs::{RamFs, StagedRename};
use crate::scheduler::Scheduler;

pub(super) struct AuditHistory<Record, const CAPACITY: usize = AUDIT_CAPACITY> {
    entries: [Record; CAPACITY],
    len: usize,
    overwritten: usize,
}

impl<const WORDS: usize, const CAPACITY: usize> Default
    for AuditHistory<[[u8; 8]; WORDS], CAPACITY>
{
    fn default() -> Self {
        Self {
            entries: [[[0; 8]; WORDS]; CAPACITY],
            len: 0,
            overwritten: 0,
        }
    }
}

impl<const WORDS: usize, const CAPACITY: usize> AuditHistory<[[u8; 8]; WORDS], CAPACITY> {
    pub(super) fn record(&mut self, entry: [[u8; 8]; WORDS]) {
        if self.len == self.entries.len() {
            self.entries.copy_within(1.., 0);
            self.overwritten = self.overwritten.saturating_add(1);
        } else {
            self.len += 1;
        }
        self.entries[self.len - 1] = entry;
    }

    pub(super) fn read(&self, copy: impl FnOnce(&[u8]) -> bool) -> Option<(usize, usize)> {
        let bytes = self.entries[..self.len].as_flattened().as_flattened();
        copy(bytes).then_some((self.len, self.overwritten))
    }

    /// Pack the legacy record for each entry before validating and copying the snapshot.
    pub(super) fn read_projected<const N: usize>(
        &self,
        project: impl FnMut(&[[u8; 8]; WORDS]) -> [[u8; 8]; N],
        copy: impl FnOnce(&[u8]) -> bool,
    ) -> Option<(usize, usize)> {
        let entries = self.entries.each_ref().map(project);
        let bytes = entries[..self.len].as_flattened().as_flattened();
        copy(bytes).then_some((self.len, self.overwritten))
    }
}

// Keep the publication row prefix intact for callers of every audit view.
type PublicationEntry = [[u8; 8]; size_of::<EncodedPublicationRecord>() / 8 + 1];

#[derive(Default)]
pub(super) struct PublicationHistory {
    explicit: AuditHistory<PublicationEntry>,
    // Revocations and explicit decisions can interleave. Each view keeps its own last four rows.
    with_revocations: AuditHistory<PublicationEntry>,
    // Cancellation or expiry can remove the final batch only once.
    terminal: AuditHistory<PublicationEntry, MAX_GOAL_RENAMES>,
    terminal_outcome: Option<PublicationOutcome>,
}

impl PublicationHistory {
    pub(super) fn record(
        &mut self,
        fs: &RamFs,
        change: &StagedRename,
        outcome: PublicationOutcome,
        reviewer: Option<PublicationReviewer>,
    ) {
        let mut row: PublicationEntry = [[0; 8]; _];
        let [record @ .., approval] = &mut row;
        *record = change.publication_record(fs, outcome);
        *approval = reviewer
            .map_or(0, PublicationReviewer::register)
            .to_le_bytes();
        match outcome {
            PublicationOutcome::DiscardedByCancel | PublicationOutcome::DiscardedByExpiry => {
                self.terminal.record(row);
                self.terminal_outcome = Some(outcome);
            }
            PublicationOutcome::DiscardedByRevoke => self.with_revocations.record(row),
            PublicationOutcome::Commit(_)
            | PublicationOutcome::Unavailable
            | PublicationOutcome::Discarded => {
                self.explicit.record(row);
                self.with_revocations.record(row);
            }
        }
    }

    fn read_row(
        &self,
        index: usize,
        view: PublicationAuditView,
        copy: impl FnOnce(&[u8]) -> bool,
    ) -> Option<(usize, usize, usize)> {
        let terminal = &self.terminal.entries[..self.terminal.len];
        let cancellation = if self.terminal_outcome == Some(PublicationOutcome::DiscardedByCancel) {
            terminal
        } else {
            &[]
        };
        let (history, tail) = match view {
            PublicationAuditView::Explicit => (&self.explicit, &[][..]),
            PublicationAuditView::WithCancellation => (&self.explicit, cancellation),
            PublicationAuditView::Decisions => (&self.with_revocations, cancellation),
            PublicationAuditView::WithExpiry => (&self.with_revocations, terminal),
        };
        let total = history.len + tail.len();
        let skipped = total.saturating_sub(AUDIT_CAPACITY);
        let count = total - skipped;
        let (bytes, reviewer) = if index == 0 && count == 0 {
            (&[][..], 0)
        } else {
            let [record @ .., approval] = history.entries[skipped..history.len]
                .iter()
                .chain(tail)
                .nth(index)?;
            (record.as_flattened(), usize::from_le_bytes(*approval))
        };
        copy(bytes).then_some((count, history.overwritten.saturating_add(skipped), reviewer))
    }
}

impl Scheduler {
    pub(crate) fn goal_delegation_audit(
        &self,
        handle: usize,
        view: DelegationAudit,
        copy: impl FnOnce(&[u8]) -> bool,
    ) -> Option<(usize, usize)> {
        let history = &self.shell_goal(handle)?.delegation_audit;
        match view {
            DelegationAudit::Decision => {
                history.read_projected(|row| *row.first_chunk::<6>().unwrap(), copy)
            }
            DelegationAudit::Caller => {
                history.read_projected(|row| *row.first_chunk::<7>().unwrap(), copy)
            }
            DelegationAudit::Tool => history.read(copy),
        }
    }

    pub(crate) fn goal_reply_audit(
        &self,
        handle: usize,
        copy: impl FnOnce(&[u8]) -> bool,
    ) -> Option<(usize, usize)> {
        self.shell_goal(handle)?.reply_audit.read(copy)
    }

    pub(crate) fn goal_rename_audit(
        &self,
        handle: usize,
        copy: impl FnOnce(&[u8]) -> bool,
    ) -> Option<(usize, usize)> {
        self.shell_goal(handle)?.rename_audit.read(copy)
    }

    pub(crate) fn goal_publication_audit(
        &self,
        handle: usize,
        index: usize,
        view: PublicationAuditView,
        copy: impl FnOnce(&[u8]) -> bool,
    ) -> Option<(usize, usize, usize)> {
        self.shell_goal(handle)?
            .publication_audit
            .read_row(index, view, copy)
    }
}
