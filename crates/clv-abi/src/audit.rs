//! Wire format for the goal owner's snapshot of helper delegation decisions.

pub const AUDIT_CAPACITY: usize = 4;
/// Agent, program, reply bytes, file flag, denial code and child handle, in RV64 words.
pub type EncodedRecord = [[u8; 8]; 6];

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
