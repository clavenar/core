//! Wire formats for the goal owner's delegation and reply-budget histories.

pub const AUDIT_CAPACITY: usize = 4;
/// Agent, program, reply bytes, file flag, denial code and child handle, in RV64 words.
pub type EncodedRecord = [[u8; 8]; 6];
/// Child, requested bytes, allowance before the decision, and flags, in RV64 words.
/// Flag bit 0 marks an unlimited allowance and bit 1 marks a budget denial.
pub type EncodedReplyRecord = [[u8; 8]; 4];

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
pub struct ReplyRecord {
    pub child: usize,
    pub bytes: usize,
    pub remaining_before: Option<usize>,
    /// False when the child's reservation could not cover this reply.
    pub sent: bool,
}

impl ReplyRecord {
    pub fn encode(self) -> EncodedReplyRecord {
        let flags = usize::from(self.remaining_before.is_none()) | (usize::from(!self.sent) << 1);
        [
            self.child,
            self.bytes,
            self.remaining_before.unwrap_or(0),
            flags,
        ]
        .map(|word| (word as u64).to_le_bytes())
    }

    pub fn decode(record: &EncodedReplyRecord) -> Option<Self> {
        let [child, bytes, remaining, flags] = record.map(u64::from_le_bytes);
        let remaining = usize::try_from(remaining).ok()?;
        let (remaining_before, sent) = match flags {
            0 => (Some(remaining), true),
            1 if remaining == 0 => (None, true),
            2 => (Some(remaining), false),
            _ => return None,
        };
        Some(Self {
            child: child.try_into().ok()?,
            bytes: bytes.try_into().ok()?,
            remaining_before,
            sent,
        })
    }
}
