//! Wire formats for the goal owner's delegation, reply and RamFS histories.

mod publication;
mod rename;
pub use publication::{
    EncodedPublicationRecord, PublicationAuditView, PublicationOutcome, PublicationRecord,
    PublicationReviewer,
};
pub use rename::{EncodedRenameRecord, RenameDenial, RenameRecord};

use crate::agent::SHA256_BYTES;
use crate::tool::ToolId;
use crate::{LaunchParent, NO_CHILD_HANDLE};

pub const AUDIT_CAPACITY: usize = 4;
/// Agent, program, reply bytes, file flag, denial code and child handle, in RV64 words.
pub type EncodedRecord = [[u8; 8]; 6];
/// Delegation decision followed by the calling agent's child launch handle (zero for the shell).
pub type EncodedRecordWithCaller = [[u8; 8]; 7];
/// Delegation audit row with caller, tool state, raw versioned ID and measured ELF SHA-256.
pub type EncodedRecordWithTool = [[u8; 8]; 13];
/// Program selector in the audit when the requested tool revision is unknown.
pub const UNRESOLVED_TOOL_PROGRAM: usize = usize::MAX;
/// Child, requested bytes, allowance before the decision, and flags, in RV64 words.
/// Flag bit 0 marks an unlimited allowance and bit 1 marks a budget denial.
pub type EncodedReplyRecord = [[u8; 8]; 4];
/// Child, requested capacity, file bytes available for this call, prior allowance, flags.
pub type EncodedReadRecord = [[u8; 8]; 5];
/// Read budget decision followed by the scope of the effective allowance.
pub type EncodedReadRecordWithLimit = [[u8; 8]; 6];
/// Read audit row with flag 4 reserved for directory-scope denials.
pub type EncodedReadDecision = EncodedReadRecordWithLimit;

/// Selects the `goal_read_audit` history in `a3`.
#[derive(Clone, Copy)]
#[repr(usize)]
pub enum ReadAuditView {
    Budget = 0,
    Decisions = 1,
}

impl ReadAuditView {
    pub const fn from_register(value: usize) -> Option<Self> {
        Some(match value {
            0 => Self::Budget,
            1 => Self::Decisions,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy)]
pub struct DelegationRequest {
    pub agent: usize,
    pub program: usize,
    pub reply_bytes: usize,
    pub with_file: bool,
}

#[derive(Clone, Copy)]
#[repr(u8)]
pub enum DelegationDenial {
    AgentFinished = 1,
    UnknownProgram = 2,
    ProgramBusy = 3,
    LaunchHandlesExhausted = 4,
    GoalFull = 5,
    InsufficientReplyBytes = 6,
    NoDelegableFile = 7,
    FileHandlesExhausted = 8,
    FileRecipientNotAllowed = 9,
    InvalidArgument = 10,
    HelperNotAllowed = 11,
    ArgumentNotAllowed = 12,
    ToolNotAllowed = 13,
}

impl DelegationDenial {
    fn from_code(code: u64) -> Option<Self> {
        Some(match code {
            1 => Self::AgentFinished,
            2 => Self::UnknownProgram,
            3 => Self::ProgramBusy,
            4 => Self::LaunchHandlesExhausted,
            5 => Self::GoalFull,
            6 => Self::InsufficientReplyBytes,
            7 => Self::NoDelegableFile,
            8 => Self::FileHandlesExhausted,
            9 => Self::FileRecipientNotAllowed,
            10 => Self::InvalidArgument,
            11 => Self::HelperNotAllowed,
            12 => Self::ArgumentNotAllowed,
            13 => Self::ToolNotAllowed,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy)]
pub struct DelegationRecord {
    pub request: DelegationRequest,
    /// Success carries the helper's original launch handle.
    pub outcome: Result<usize, DelegationDenial>,
}

impl DelegationRecord {
    pub fn encode(self) -> EncodedRecord {
        let (code, child) = match self.outcome {
            Ok(handle) => (0, handle),
            Err(reason) => (reason as usize, 0),
        };
        let request = self.request;
        [
            request.agent,
            request.program,
            request.reply_bytes,
            usize::from(request.with_file),
            code,
            child,
        ]
        .map(|word| (word as u64).to_le_bytes())
    }

    pub fn decode(record: &EncodedRecord) -> Option<Self> {
        let [agent, program, reply_bytes, file, code, child] = record.map(u64::from_le_bytes);
        let request = DelegationRequest {
            agent: agent.try_into().ok()?,
            program: program.try_into().ok()?,
            reply_bytes: reply_bytes.try_into().ok()?,
            with_file: match file {
                0 => false,
                1 => true,
                _ => return None,
            },
        };
        let outcome = match (code, child) {
            (0, 1..) => Ok(child.try_into().ok()?),
            (_, 0) => Err(DelegationDenial::from_code(code)?),
            _ => return None,
        };
        Some(Self { request, outcome })
    }
}

#[derive(Clone, Copy)]
pub struct DelegationRecordWithCaller {
    pub decision: DelegationRecord,
    pub caller: LaunchParent,
}

fn encode_caller(caller: LaunchParent) -> [u8; 8] {
    let handle = match caller {
        LaunchParent::Shell => NO_CHILD_HANDLE,
        LaunchParent::Agent(child) => child,
    };
    (handle as u64).to_le_bytes()
}

fn decode_caller(word: [u8; 8]) -> Option<LaunchParent> {
    Some(match usize::try_from(u64::from_le_bytes(word)).ok()? {
        NO_CHILD_HANDLE => LaunchParent::Shell,
        child => LaunchParent::Agent(child),
    })
}

impl DelegationRecordWithCaller {
    pub fn encode(self) -> EncodedRecordWithCaller {
        let mut record = [[0; 8]; 7];
        let [decision @ .., caller] = &mut record;
        *decision = self.decision.encode();
        *caller = encode_caller(self.caller);
        record
    }

    pub fn decode(record: &EncodedRecordWithCaller) -> Option<Self> {
        let [decision @ .., caller] = record;
        Some(Self {
            decision: DelegationRecord::decode(decision)?,
            caller: decode_caller(*caller)?,
        })
    }
}

pub struct RequestedTool<'a> {
    pub id: ToolId,
    /// Present when the exact requested revision resolved in the kernel registry.
    pub image_sha256: Option<&'a [u8; SHA256_BYTES]>,
}

pub struct DelegationRecordWithTool<'a> {
    pub delegation: DelegationRecordWithCaller,
    pub tool: Option<RequestedTool<'a>>,
}

impl<'a> DelegationRecordWithTool<'a> {
    pub fn encode(&self) -> EncodedRecordWithTool {
        let mut record = [[0; 8]; 13];
        let (prefix, [state, id, digest @ ..]) = record.split_last_chunk_mut::<6>().unwrap();
        prefix.copy_from_slice(&self.delegation.encode());
        if let Some(tool) = &self.tool {
            *id = tool.id.0.to_le_bytes();
            let tag: u64 = if let Some(hash) = tool.image_sha256 {
                digest.as_flattened_mut().copy_from_slice(hash);
                2
            } else {
                1
            };
            *state = tag.to_le_bytes();
        }
        record
    }

    pub fn decode(record: &'a EncodedRecordWithTool) -> Option<Self> {
        let (prefix, [state, id, digest @ ..]) = record.split_last_chunk::<6>()?;
        let id = ToolId(u64::from_le_bytes(*id));
        let tool = match u64::from_le_bytes(*state) {
            0 => None,
            1 => Some(RequestedTool {
                id,
                image_sha256: None,
            }),
            2 => Some(RequestedTool {
                id,
                image_sha256: Some(digest.as_flattened().try_into().ok()?),
            }),
            _ => return None,
        };
        Some(Self {
            delegation: DelegationRecordWithCaller::decode(prefix.try_into().ok()?)?,
            tool,
        })
    }
}

#[derive(Clone, Copy)]
pub struct ReplyRecord {
    pub child: usize,
    pub bytes: usize,
    pub remaining_before: Option<usize>,
    /// False when the child's reservation could not cover this reply.
    pub sent: bool,
}

impl ReplyRecord {
    pub fn encode(self) -> EncodedReplyRecord {
        [
            self.child,
            self.bytes,
            self.remaining_before.unwrap_or(0),
            budget_flags(self.remaining_before, self.sent),
        ]
        .map(|word| (word as u64).to_le_bytes())
    }

    pub fn decode(record: &EncodedReplyRecord) -> Option<Self> {
        let [child, bytes, remaining, flags] = record.map(u64::from_le_bytes);
        let (remaining_before, sent) = budget_decision(remaining, flags)?;
        Some(Self {
            child: child.try_into().ok()?,
            bytes: bytes.try_into().ok()?,
            remaining_before,
            sent,
        })
    }
}

#[derive(Clone, Copy)]
pub struct ReadRecord {
    pub child: usize,
    pub requested: usize,
    /// The smaller of the requested capacity and the bytes left in the file.
    pub bytes: usize,
    /// The tightest remaining allowance applicable to this launch.
    pub remaining_before: Option<usize>,
    /// A delivered read charged `bytes`. A denied read charged zero.
    pub delivered: bool,
}

impl ReadRecord {
    pub fn encode(self) -> EncodedReadRecord {
        [
            self.child,
            self.requested,
            self.bytes,
            self.remaining_before.unwrap_or(0),
            budget_flags(self.remaining_before, self.delivered),
        ]
        .map(|word| (word as u64).to_le_bytes())
    }

    pub fn decode(record: &EncodedReadRecord) -> Option<Self> {
        let [child, requested, bytes, remaining, flags] = record.map(u64::from_le_bytes);
        let (remaining_before, delivered) = budget_decision(remaining, flags)?;
        Some(Self {
            child: child.try_into().ok()?,
            requested: requested.try_into().ok()?,
            bytes: bytes.try_into().ok()?,
            remaining_before,
            delivered,
        })
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ReadLimitScope {
    Goal = 0,
    Helper = 1,
}

#[derive(Clone, Copy)]
pub struct ReadRecordWithLimit {
    pub read: ReadRecord,
    pub limit: ReadLimitScope,
}

impl ReadRecordWithLimit {
    pub fn encode(self) -> EncodedReadRecordWithLimit {
        let mut record = [[0; 8]; 6];
        let [read @ .., limit] = &mut record;
        *read = self.read.encode();
        *limit = (self.limit as u64).to_le_bytes();
        record
    }

    pub fn decode(record: &EncodedReadRecordWithLimit) -> Option<Self> {
        let [read @ .., limit] = record;
        let read = ReadRecord::decode(read)?;
        let limit = match u64::from_le_bytes(*limit) {
            0 => ReadLimitScope::Goal,
            1 if read.remaining_before.is_some() => ReadLimitScope::Helper,
            _ => return None,
        };
        Some(Self { read, limit })
    }
}

#[derive(Clone, Copy)]
pub enum ReadDecision {
    Budget(ReadRecordWithLimit),
    OutsideScope { child: usize, requested: usize },
}

impl ReadDecision {
    const OUTSIDE_SCOPE: u64 = 4;

    pub fn encode(self) -> EncodedReadDecision {
        match self {
            Self::Budget(entry) => entry.encode(),
            Self::OutsideScope { child, requested } => {
                [child as u64, requested as u64, 0, 0, Self::OUTSIDE_SCOPE, 0].map(u64::to_le_bytes)
            }
        }
    }

    pub fn decode(record: &EncodedReadDecision) -> Option<Self> {
        match record.map(u64::from_le_bytes) {
            [child, requested, 0, 0, Self::OUTSIDE_SCOPE, 0] => Some(Self::OutsideScope {
                child: child.try_into().ok()?,
                requested: requested.try_into().ok()?,
            }),
            _ => ReadRecordWithLimit::decode(record).map(Self::Budget),
        }
    }
}

fn budget_flags(remaining: Option<usize>, allowed: bool) -> usize {
    usize::from(remaining.is_none()) | (usize::from(!allowed) << 1)
}

fn budget_decision(remaining: u64, flags: u64) -> Option<(Option<usize>, bool)> {
    match flags {
        0 => Some((Some(remaining.try_into().ok()?), true)),
        1 if remaining == 0 => Some((None, true)),
        2 => Some((Some(remaining.try_into().ok()?), false)),
        _ => None,
    }
}
