use core::num::NonZeroUsize;

use crate::PATH_MAX_BYTES;
use crate::budget::ReplyBudget;
use crate::goal::{CommitResult, MAX_GOAL_RENAMES, RENAME_DIFF_MAX_BYTES};

/// Decision, base and observed revisions, source and basename lengths, then the frozen diff.
pub type EncodedPublicationRecord = [[u8; 8]; 5 + RENAME_DIFF_MAX_BYTES.div_ceil(8)];

/// Selects the `goal_publication_decisions_with_reviewer` history in `a4`.
#[derive(Clone, Copy)]
#[repr(usize)]
pub enum PublicationAuditView {
    Decisions = 0,
    WithExpiry = 1,
    Explicit = 2,
    WithCancellation = 3,
}

impl PublicationAuditView {
    pub const fn from_register(value: usize) -> Option<Self> {
        Some(match value {
            0 => Self::Decisions,
            1 => Self::WithExpiry,
            2 => Self::Explicit,
            3 => Self::WithCancellation,
            _ => return None,
        })
    }
}

/// The reviewer whose approval was consumed by a publication attempt.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PublicationReviewer {
    Owner,
    Supervisor(NonZeroUsize),
}

impl PublicationReviewer {
    /// The publication audit reports zero for rows without a review. The all-ones value denotes
    /// the owner and is reserved from launch handles by the kernel's handle allocator.
    pub const fn register(self) -> usize {
        match self {
            Self::Owner => usize::MAX,
            Self::Supervisor(launch) => launch.get(),
        }
    }

    pub fn from_register(value: usize) -> Option<Self> {
        if value == usize::MAX {
            return Some(Self::Owner);
        }
        Some(Self::Supervisor(NonZeroUsize::new(value)?))
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PublicationOutcome {
    Commit(CommitResult),
    Unavailable,
    Discarded,
    DiscardedByCancel,
    DiscardedByRevoke,
    DiscardedByExpiry,
}

impl PublicationOutcome {
    fn code(self) -> u64 {
        match self {
            Self::Commit(result) => result as u64,
            Self::Unavailable => 4,
            Self::Discarded => 5,
            Self::DiscardedByCancel => 6,
            Self::DiscardedByRevoke => 7,
            Self::DiscardedByExpiry => 8,
        }
    }

    fn from_code(code: u64) -> Option<Self> {
        Some(match code {
            4 => Self::Unavailable,
            5 => Self::Discarded,
            6 => Self::DiscardedByCancel,
            7 => Self::DiscardedByRevoke,
            8 => Self::DiscardedByExpiry,
            _ => Self::Commit(CommitResult::from_register(code.try_into().ok()?)?),
        })
    }
}

pub type EncodedPublicationObservation = [[u8; 8]; 8];

/// Snapshot taken when a whole rename transaction is published or discarded.
#[derive(Clone, Copy)]
pub struct PublicationObservation {
    pub goal: usize,
    pub outcome: PublicationOutcome,
    pub reviewer: Option<PublicationReviewer>,
    pub renames: usize,
    pub budget: ReplyBudget,
}

impl PublicationObservation {
    pub fn encode(self) -> EncodedPublicationObservation {
        let (flags, available, reserved, spent) = self.budget.registers();
        [
            self.goal as u64,
            self.outcome.code(),
            self.reviewer.map_or(0, PublicationReviewer::register) as u64,
            self.renames as u64,
            flags as u64,
            available as u64,
            reserved as u64,
            spent as u64,
        ]
        .map(u64::to_le_bytes)
    }

    pub fn decode(bytes: &[u8]) -> Option<Self> {
        let (words, []) = bytes.as_chunks::<8>() else {
            return None;
        };
        let words: &EncodedPublicationObservation = words.try_into().ok()?;
        let [
            goal,
            outcome,
            reviewer,
            renames,
            flags,
            available,
            reserved,
            spent,
        ] = words.map(u64::from_le_bytes);
        let outcome = PublicationOutcome::from_code(outcome)?;
        if goal == 0
            || !(1..=MAX_GOAL_RENAMES as u64).contains(&renames)
            || !matches!(
                outcome,
                PublicationOutcome::Commit(CommitResult::Committed)
                    | PublicationOutcome::Discarded
                    | PublicationOutcome::DiscardedByCancel
                    | PublicationOutcome::DiscardedByRevoke
                    | PublicationOutcome::DiscardedByExpiry
            )
        {
            return None;
        }
        Some(Self {
            goal: goal as usize,
            outcome,
            reviewer: PublicationReviewer::from_register(reviewer as usize),
            renames: renames as usize,
            budget: ReplyBudget::from_registers(
                flags as usize,
                available as usize,
                reserved as usize,
                spent as usize,
            )?,
        })
    }
}

pub struct PublicationRecord<'a> {
    pub outcome: PublicationOutcome,
    pub base_revision: u64,
    /// RamFS revision immediately after the decision, including the new version on success.
    pub revision: u64,
    pub source: &'a [u8],
    pub new_name: &'a [u8],
}

impl<'a> PublicationRecord<'a> {
    fn valid(&self) -> bool {
        self.source.starts_with(b"/")
            && (1..=PATH_MAX_BYTES).contains(&self.new_name.len())
            && self.revision >= self.base_revision
            && (self.outcome != PublicationOutcome::Commit(CommitResult::Committed)
                || self.base_revision.checked_add(1) == Some(self.revision))
    }

    pub fn encode(&self) -> Option<EncodedPublicationRecord> {
        if !self.valid() {
            return None;
        }
        let mut record: EncodedPublicationRecord = [[0; 8]; _];
        let [code, base, revision, source_len, name_len, names @ ..] = &mut record;
        let (source, rest) = names
            .as_flattened_mut()
            .split_at_mut_checked(self.source.len())?;
        rest.get_mut(..self.new_name.len())?
            .copy_from_slice(self.new_name);
        source.copy_from_slice(self.source);
        *code = self.outcome.code().to_le_bytes();
        *base = self.base_revision.to_le_bytes();
        *revision = self.revision.to_le_bytes();
        *source_len = (source.len() as u64).to_le_bytes();
        *name_len = (self.new_name.len() as u64).to_le_bytes();
        Some(record)
    }

    pub fn decode(record: &'a EncodedPublicationRecord) -> Option<Self> {
        let [code, base, revision, source_len, name_len, names @ ..] = record;
        let source_len = usize::try_from(u64::from_le_bytes(*source_len)).ok()?;
        let name_len = usize::try_from(u64::from_le_bytes(*name_len)).ok()?;
        let (source, rest) = names.as_flattened().split_at_checked(source_len)?;
        let row = Self {
            outcome: PublicationOutcome::from_code(u64::from_le_bytes(*code))?,
            base_revision: u64::from_le_bytes(*base),
            revision: u64::from_le_bytes(*revision),
            source,
            new_name: rest.get(..name_len)?,
        };
        row.valid().then_some(row)
    }
}
