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
    // Cancellation is terminal. Keep its final row separate so call 76 can still read
    // the last four explicit decisions and their original overwritten count.
    cancellation: Option<EncodedPublicationRecord>,
}

impl PublicationHistory {
    pub(super) fn record(
        &mut self,
        fs: &RamFs,
        change: &StagedRename,
        outcome: PublicationOutcome,
    ) {
        let row = change.publication_record(fs, outcome);
        if outcome == PublicationOutcome::DiscardedByCancel {
            self.cancellation = Some(row);
        } else {
            self.explicit.record(row);
        }
    }

    fn read_row(
        &self,
        index: usize,
        include_cancellation: bool,
        copy: impl FnOnce(&[u8]) -> bool,
    ) -> Option<(usize, usize)> {
        let cancellation = if include_cancellation {
            self.cancellation.as_slice()
        } else {
            &[]
        };
        let total = self.explicit.len + cancellation.len();
        let skipped = total.saturating_sub(AUDIT_CAPACITY);
        let count = total - skipped;
        let bytes = if index == 0 && count == 0 {
            &[][..]
        } else {
            self.explicit.entries[skipped..self.explicit.len]
                .iter()
                .chain(cancellation)
                .nth(index)?
                .as_flattened()
        };
        copy(bytes).then_some((count, self.explicit.overwritten.saturating_add(skipped)))
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
        include_cancellation: bool,
        copy: impl FnOnce(&[u8]) -> bool,
    ) -> Option<(usize, usize)> {
        self.shell_goal(handle)?
            .publication_audit
            .read_row(index, include_cancellation, copy)
    }
}
