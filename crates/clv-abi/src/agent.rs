use crate::goal::GoalStatus;
use crate::tool::ToolId;
use crate::{MESSAGE_MAX_BYTES, PATH_MAX_BYTES, Program, RequestKind};

const SUSPENDED: usize = 6;
pub const RUN_SUSPENDED: usize = 1;
pub const RUN_REPLY_LIMIT: usize = 2;
/// Selects a one-byte PlanIntent in place of the default FileDelegation payload.
pub const RUN_PLAN_INTENT: usize = 4;
/// Selects the two-byte definition ID in a PlanDefinition request.
pub const RUN_PLAN_DEFINITION: usize = 8;
/// Selects a destination basename for the agent to propose on its granted file.
pub const RUN_STAGE_RENAME: usize = 16;
/// Selects two destination basenames in rename grant order.
pub const RUN_STAGE_RENAME_PAIR: usize = 32;
/// A zero agent handle on a delegation call selects the calling task's live agent.
pub const CURRENT_AGENT: usize = 0;

pub const DELEGATION_HEADER_BYTES: usize = 16;
pub const DELEGATION_REQUEST_MAX_BYTES: usize = DELEGATION_HEADER_BYTES + MESSAGE_MAX_BYTES;
/// The largest launch request carries a length byte and two basenames.
pub const LAUNCH_REQUEST_MAX_BYTES: usize = 1 + 2 * PATH_MAX_BYTES;

pub fn encode_rename_pair<'a>(
    names: [&[u8]; 2],
    buffer: &'a mut [u8; LAUNCH_REQUEST_MAX_BYTES],
) -> Option<&'a [u8]> {
    if names
        .iter()
        .any(|name| !(1..=PATH_MAX_BYTES).contains(&name.len()))
    {
        return None;
    }
    let [first, second] = names;
    let split = 1 + first.len();
    let end = split + second.len();
    buffer[0] = first.len() as u8;
    buffer[1..split].copy_from_slice(first);
    buffer[split..end].copy_from_slice(second);
    Some(&buffer[..end])
}

pub fn decode_rename_pair(bytes: &[u8]) -> Option<[&[u8]; 2]> {
    let (&length, names) = bytes.split_first()?;
    let (first, second) = names.split_at_checked(usize::from(length))?;
    let names = [first, second];
    names
        .iter()
        .all(|name| (1..=PATH_MAX_BYTES).contains(&name.len()))
        .then_some(names)
}

/// One-byte intent delivered to a user-space planner.
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PlanIntent {
    FindGreeting = 1,
}

impl PlanIntent {
    pub const fn name(self) -> &'static str {
        match self {
            Self::FindGreeting => "find-greeting",
        }
    }

    pub const fn program(self) -> Program {
        match self {
            Self::FindGreeting => Program::Hello,
        }
    }

    pub const fn from_raw(id: usize) -> Option<Self> {
        match id {
            id if id == Self::FindGreeting as usize => Some(Self::FindGreeting),
            _ => None,
        }
    }

    pub fn decode(bytes: &[u8]) -> Option<Self> {
        let [id] = bytes else { return None };
        Self::from_raw(usize::from(*id))
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct DefinitionId {
    pub intent: PlanIntent,
    pub revision: u8,
}

pub const FIND_GREETING_V1: DefinitionId = DefinitionId {
    intent: PlanIntent::FindGreeting,
    revision: 1,
};

pub const FIND_GREETING_V2: DefinitionId = DefinitionId {
    intent: PlanIntent::FindGreeting,
    revision: 2,
};

pub const FIND_GREETING_V3: DefinitionId = DefinitionId {
    intent: PlanIntent::FindGreeting,
    revision: 3,
};

pub const FIND_GREETING_V4: DefinitionId = DefinitionId {
    intent: PlanIntent::FindGreeting,
    revision: 4,
};

impl DefinitionId {
    pub const fn encode(self) -> [u8; 2] {
        [self.intent as u8, self.revision]
    }

    /// The original one-byte intent names revision 1 for the lifetime of its ABI.
    pub fn from_request(kind: RequestKind, bytes: &[u8]) -> Option<Self> {
        match (kind, bytes) {
            (RequestKind::PlanIntent, [id]) if *id == FIND_GREETING_V1.intent as u8 => {
                Some(FIND_GREETING_V1)
            }
            (RequestKind::PlanDefinition, [id, revision]) => Some(Self {
                intent: PlanIntent::from_raw(usize::from(*id))?,
                revision: *revision,
            }),
            _ => None,
        }
    }
}

pub const SHA256_BYTES: usize = 32;
/// Intent, revision and SHA-256. An all-zero record identifies an unpinned agent.
pub type EncodedDefinitionPin = [u8; 2 + SHA256_BYTES];
pub const NO_DEFINITION: EncodedDefinitionPin = [0; 2 + SHA256_BYTES];

pub struct DefinitionPin<'a> {
    pub id: DefinitionId,
    pub sha256: &'a [u8; SHA256_BYTES],
}

impl<'a> DefinitionPin<'a> {
    pub fn encode(&self) -> EncodedDefinitionPin {
        let mut bytes = [0; 2 + SHA256_BYTES];
        bytes[..2].copy_from_slice(&self.id.encode());
        bytes[2..].copy_from_slice(self.sha256);
        bytes
    }

    pub fn decode(bytes: &'a EncodedDefinitionPin) -> Option<Self> {
        let (id, hash) = bytes.split_first_chunk::<2>()?;
        Some(Self {
            id: DefinitionId::from_request(RequestKind::PlanDefinition, id)?,
            sha256: hash.try_into().ok()?,
        })
    }
}

/// User-space instruction carried by a DelegateWithFile request. The two
/// little-endian RV64 words precede the helper's literal argument.
#[derive(Clone, Copy)]
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
        encode_file_action(self.program as u64, self.reply_bytes, self.argument, buffer)
    }

    pub fn decode(bytes: &'a [u8]) -> Option<Self> {
        let (selector, reply_bytes, argument) = decode_file_action(bytes)?;
        Some(Self {
            program: Program::from_raw(selector.try_into().ok()?)?,
            reply_bytes,
            argument,
        })
    }
}

/// A definition step names a tool revision independently of its backing executable.
pub struct FileToolInvocation<'a> {
    pub tool: ToolId,
    pub reply_bytes: usize,
    pub argument: &'a [u8],
}

impl<'a> FileToolInvocation<'a> {
    pub fn encode<'b>(
        &self,
        buffer: &'b mut [u8; DELEGATION_REQUEST_MAX_BYTES],
    ) -> Option<&'b [u8]> {
        encode_file_action(self.tool.0, self.reply_bytes, self.argument, buffer)
    }

    pub fn decode(bytes: &'a [u8]) -> Option<Self> {
        let (selector, reply_bytes, argument) = decode_file_action(bytes)?;
        Some(Self {
            tool: ToolId(selector),
            reply_bytes,
            argument,
        })
    }
}

fn encode_file_action<'a>(
    selector: u64,
    reply_bytes: usize,
    argument: &[u8],
    buffer: &'a mut [u8; DELEGATION_REQUEST_MAX_BYTES],
) -> Option<&'a [u8]> {
    let (header, literal) = buffer.split_at_mut(DELEGATION_HEADER_BYTES);
    literal.get_mut(..argument.len())?.copy_from_slice(argument);
    header[..8].copy_from_slice(&selector.to_le_bytes());
    header[8..].copy_from_slice(&(reply_bytes as u64).to_le_bytes());
    Some(&buffer[..DELEGATION_HEADER_BYTES + argument.len()])
}

fn decode_file_action(bytes: &[u8]) -> Option<(u64, usize, &[u8])> {
    let (selector, rest) = bytes.split_first_chunk::<8>()?;
    let (reply_bytes, argument) = rest.split_first_chunk::<8>()?;
    if argument.len() > MESSAGE_MAX_BYTES {
        return None;
    }
    Some((
        u64::from_le_bytes(*selector),
        u64::from_le_bytes(*reply_bytes).try_into().ok()?,
        argument,
    ))
}

/// The two handles returned by a successful agent run.
pub struct AgentLaunch {
    pub agent: usize,
    pub child: usize,
}

pub type EncodedResumeReceipt = [[u8; 8]; 4];

/// Boot-unique handles recorded when a supervisor makes a suspended child runnable.
pub struct ResumeReceipt {
    pub supervisor: usize,
    pub goal: usize,
    pub agent: usize,
    pub child: usize,
}

impl ResumeReceipt {
    pub fn encode(self) -> EncodedResumeReceipt {
        [self.supervisor, self.goal, self.agent, self.child].map(|word| (word as u64).to_le_bytes())
    }

    pub fn decode(bytes: &[u8]) -> Option<Self> {
        let (words, []) = bytes.as_chunks::<8>() else {
            return None;
        };
        let words: &EncodedResumeReceipt = words.try_into().ok()?;
        let [supervisor, goal, agent, child] = words.map(|word| u64::from_le_bytes(word) as usize);
        Some(Self {
            supervisor,
            goal,
            agent,
            child,
        })
    }
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
