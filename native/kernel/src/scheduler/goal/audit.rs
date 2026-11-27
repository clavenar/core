use clv_abi::audit::{AUDIT_CAPACITY, EncodedPublicationRecord, PublicationOutcome};

use super::DelegationAudit;
use crate::fs::ramfs::{RamFs, StagedRename};
use crate::scheduler::Scheduler;

pub(super) struct AuditHistory<Record> {
    entries: [Record; AUDIT_CAPACITY],
    len: usize,
    overwritten: usize,
}

impl<const WORDS: usize> Default for AuditHistory<[[u8; 8]; WORDS]> {
    fn default() -> Self {
        Self {
            entries: [[[0; 8]; WORDS]; AUDIT_CAPACITY],
            len: 0,
            overwritten: 0,
        }
    }
}

impl<const WORDS: usize> AuditHistory<[[u8; 8]; WORDS]> {
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

#[derive(Default)]
pub(super) struct PublicationHistory {
    explicit: AuditHistory<EncodedPublicationRecord>,
    // A revocation can precede later owner decisions. Older readers retain their own four rows.
    with_revocations: AuditHistory<EncodedPublicationRecord>,
    // Cancellation is terminal, so both views that include it can share its final row.
    cancellation: Option<EncodedPublicationRecord>,
}

pub(crate) enum PublicationAudit {
    Explicit,
    WithCancellation,
    Decisions,
}

impl PublicationHistory {
    pub(super) fn record(
        &mut self,
        fs: &RamFs,
        change: &StagedRename,
        outcome: PublicationOutcome,
    ) {
        let row = change.publication_record(fs, outcome);
        match outcome {
            PublicationOutcome::DiscardedByCancel => self.cancellation = Some(row),
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
        view: PublicationAudit,
        copy: impl FnOnce(&[u8]) -> bool,
    ) -> Option<(usize, usize)> {
        let (history, cancellation) = match view {
            PublicationAudit::Explicit => (&self.explicit, &[][..]),
            PublicationAudit::WithCancellation => (&self.explicit, self.cancellation.as_slice()),
            PublicationAudit::Decisions => (&self.with_revocations, self.cancellation.as_slice()),
        };
        let total = history.len + cancellation.len();
        let skipped = total.saturating_sub(AUDIT_CAPACITY);
        let count = total - skipped;
        let bytes = if index == 0 && count == 0 {
            &[][..]
        } else {
            history.entries[skipped..history.len]
                .iter()
                .chain(cancellation)
                .nth(index)?
                .as_flattened()
        };
        copy(bytes).then_some((count, history.overwritten.saturating_add(skipped)))
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
        view: PublicationAudit,
        copy: impl FnOnce(&[u8]) -> bool,
    ) -> Option<(usize, usize)> {
        self.shell_goal(handle)?
            .publication_audit
            .read_row(index, view, copy)
    }
}
