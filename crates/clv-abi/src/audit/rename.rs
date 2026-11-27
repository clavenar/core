use super::{decode_caller, encode_caller};
use crate::{LaunchParent, PATH_MAX_BYTES};

/// Caller, decision, copied name length, then the zero-padded basename.
pub type EncodedRenameRecord = [[u8; 8]; 3 + PATH_MAX_BYTES.div_ceil(8)];
const NO_NAME: u64 = u64::MAX;

#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum RenameDenial {
    NoRenameGrant = 1,
    InvalidBasename = 2,
    DestinationExists = 3,
    AlreadyStaged = 4,
    InvalidSource = 5,
    GoalInactive = 6,
    InvalidBuffer = 7,
    OutsideRenameScope = 8,
    BaseChanged = 9,
    SourceAlreadyStaged = 10,
    DestinationAlreadyStaged = 11,
}

impl RenameDenial {
    fn from_code(code: u64) -> Option<Self> {
        Some(match code {
            1 => Self::NoRenameGrant,
            2 => Self::InvalidBasename,
            3 => Self::DestinationExists,
            4 => Self::AlreadyStaged,
            5 => Self::InvalidSource,
            6 => Self::GoalInactive,
            7 => Self::InvalidBuffer,
            8 => Self::OutsideRenameScope,
            9 => Self::BaseChanged,
            10 => Self::SourceAlreadyStaged,
            11 => Self::DestinationAlreadyStaged,
            _ => return None,
        })
    }
}

pub struct RenameRecord<'a> {
    pub caller: LaunchParent,
    /// Absent when the submitted length or buffer prevented copying the name.
    pub basename: Option<&'a [u8]>,
    pub outcome: Result<(), RenameDenial>,
}

impl<'a> RenameRecord<'a> {
    pub fn encode(&self) -> EncodedRenameRecord {
        let mut record = EncodedRenameRecord::default();
        let [caller, decision, length, storage @ ..] = &mut record;
        *caller = encode_caller(self.caller);
        let code = self.outcome.err().map_or(0, |reason| reason as u64);
        *decision = code.to_le_bytes();
        let copied = match self.basename {
            Some(name) if name.len() <= PATH_MAX_BYTES => {
                storage.as_flattened_mut()[..name.len()].copy_from_slice(name);
                name.len() as u64
            }
            _ => NO_NAME,
        };
        *length = copied.to_le_bytes();
        record
    }

    pub fn decode(record: &'a EncodedRenameRecord) -> Option<Self> {
        let [caller, code, length, storage @ ..] = record;
        let outcome = match u64::from_le_bytes(*code) {
            0 => Ok(()),
            code => Err(RenameDenial::from_code(code)?),
        };
        let basename = match (outcome, u64::from_le_bytes(*length)) {
            (Err(RenameDenial::InvalidBasename | RenameDenial::InvalidBuffer), NO_NAME) => None,
            (Err(RenameDenial::InvalidBuffer), _) | (Ok(()), 0) => return None,
            (_, len) if len <= PATH_MAX_BYTES as u64 => {
                Some(&storage.as_flattened()[..len as usize])
            }
            _ => return None,
        };
        Some(Self {
            caller: decode_caller(*caller)?,
            basename,
            outcome,
        })
    }
}
