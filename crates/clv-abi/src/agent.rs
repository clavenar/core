use crate::goal::GoalStatus;
use crate::{MESSAGE_MAX_BYTES, Program};

const SUSPENDED: usize = 6;
pub const RUN_SUSPENDED: usize = 1;
pub const RUN_REPLY_LIMIT: usize = 2;
/// A zero agent handle on a delegation call selects the calling task's live agent.
pub const CURRENT_AGENT: usize = 0;

pub const DELEGATION_HEADER_BYTES: usize = 16;
pub const DELEGATION_REQUEST_MAX_BYTES: usize = DELEGATION_HEADER_BYTES + MESSAGE_MAX_BYTES;

/// User-space instruction carried by a DelegateWithFile request. The two
/// little-endian RV64 words precede the helper's literal argument.
pub struct FileDelegation<'a> {
    pub program: Program,
    pub reply_bytes: usize,
    pub argument: &'a [u8],
}

impl<'a> FileDelegation<'a> {
    pub fn encode<'b>(
        &self,
        buffer: &'b mut [u8; DELEGATION_REQUEST_MAX_BYTES],
    ) -> Option<&'b [u8]> {
        let (header, argument) = buffer.split_at_mut(DELEGATION_HEADER_BYTES);
        argument
            .get_mut(..self.argument.len())?
            .copy_from_slice(self.argument);
        header[..8].copy_from_slice(&(self.program as u64).to_le_bytes());
        header[8..].copy_from_slice(&(self.reply_bytes as u64).to_le_bytes());
        Some(&buffer[..DELEGATION_HEADER_BYTES + self.argument.len()])
    }

    pub fn decode(bytes: &'a [u8]) -> Option<Self> {
        let (header, argument) = bytes.split_at_checked(DELEGATION_HEADER_BYTES)?;
        if argument.len() > MESSAGE_MAX_BYTES {
            return None;
        }
        let selector = u64::from_le_bytes(header[..8].try_into().ok()?);
        let reply_bytes = u64::from_le_bytes(header[8..].try_into().ok()?);
        Some(Self {
            program: Program::from_raw(selector.try_into().ok()?)?,
            reply_bytes: reply_bytes.try_into().ok()?,
            argument,
        })
    }
}

/// The two handles returned by a successful agent run.
pub struct AgentLaunch {
    pub agent: usize,
    pub child: usize,
}

/// The owning goal and the state of the agent's original child launch.
pub struct AgentStatus {
    pub goal: usize,
    /// A live agent's goal stays running while the child is suspended.
    pub status: GoalStatus,
    /// True while the scheduler holds the original child at its saved frame.
    pub suspended: bool,
}

impl AgentStatus {
    /// RV64 state, child and detail in a0–a2. The owning goal is returned in a3.
    pub const fn registers(&self) -> (usize, usize, usize) {
        match self.status {
            GoalStatus::Running { child } if self.suspended => (SUSPENDED, child, 0),
            status => status.registers(),
        }
    }

    pub fn from_registers(state: usize, child: usize, value: usize, goal: usize) -> Option<Self> {
        let suspended = state == SUSPENDED;
        let status = if suspended {
            GoalStatus::Running { child }
        } else {
            GoalStatus::from_registers(state, child, value)?
        };
        Some(Self {
            goal,
            status,
            suspended,
        })
    }
}

#[repr(usize)]
pub enum ControlResult {
    Applied = 0,
    AlreadyFinished = 1,
}
