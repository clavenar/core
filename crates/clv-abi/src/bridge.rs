//! Bounded guest-host records for planning, tool calls and checkpoint recovery.

use crate::ReplyKind;

pub const SUBMIT: usize = 0;
pub const POLL: usize = 1;
pub const SUBMIT_TOOL: usize = 2;

/// The buffer size passed by an agent to call 76.
pub const MAX_FRAME_BYTES: usize = 32;
pub const MAX_WIRE_BYTES: usize = 36;
pub const TOOL_TEXT_BYTES: usize = 20;
pub type CheckpointId = [u8; 16];
/// Length, version, kind and a little-endian u64 request ID.
pub const HEADER_BYTES: usize = 11;
const PLAN_FRAME_BYTES: usize = HEADER_BYTES + 1;
const TOOL_REQUEST_BYTES: usize = PLAN_FRAME_BYTES + 8;
const COMBINED_REQUEST_BYTES: usize = TOOL_REQUEST_BYTES + 8;
const LOOKUP_BYTES: usize = PLAN_FRAME_BYTES + size_of::<CheckpointId>();
const CHECKPOINT_SEARCH_BYTES: usize = LOOKUP_BYTES + 8;
const VERSION: u8 = 1;
const PLAN_REQUEST: u8 = 1;
const PLAN_REPLY: u8 = 2;
const TOOL_REQUEST: u8 = 3;
const TOOL_REPLY: u8 = 4;
const CHECKPOINT_REPLY: u8 = 5;
const SEARCH_FIXTURE: u8 = 3;
const COMBINED_PLAN: u8 = 4;
const COMBINED_TOOL: u8 = 5;
const CHECKPOINT_SEARCH: u8 = 6;
const CHECKPOINT_LOOKUP: u8 = 7;
const TOOL_OK: u8 = 1;
const TOOL_INVALID_REPLY: u8 = 2;
const TOOL_UNAVAILABLE: u8 = 4;
const OUTCOME_UNKNOWN: u8 = 5;
pub const BUDGET_EXHAUSTED: u8 = 0;
pub const SEARCH_GREETING: u8 = 1;
pub const SKIP: u8 = 3;
pub const MODEL_UNAVAILABLE: u8 = 4;

pub enum HostRequest {
    MockGreeting,
    ModelGreeting,
    SearchFixture { goal: u64 },
    Combined { goal: u64, launch: u64, tool: bool },
    CheckpointSearch { operation: CheckpointId, goal: u64 },
    CheckpointLookup { operation: CheckpointId },
}

impl HostRequest {
    pub fn encode(self, request_id: u64, buffer: &mut [u8; MAX_WIRE_BYTES]) -> &[u8] {
        let (kind, selector, length) = match self {
            Self::MockGreeting => (PLAN_REQUEST, 1, PLAN_FRAME_BYTES),
            Self::ModelGreeting => (PLAN_REQUEST, 2, PLAN_FRAME_BYTES),
            Self::SearchFixture { goal } => {
                buffer[PLAN_FRAME_BYTES..TOOL_REQUEST_BYTES].copy_from_slice(&goal.to_le_bytes());
                (TOOL_REQUEST, SEARCH_FIXTURE, TOOL_REQUEST_BYTES)
            }
            Self::Combined { goal, launch, tool } => {
                buffer[PLAN_FRAME_BYTES..TOOL_REQUEST_BYTES].copy_from_slice(&goal.to_le_bytes());
                buffer[TOOL_REQUEST_BYTES..COMBINED_REQUEST_BYTES]
                    .copy_from_slice(&launch.to_le_bytes());
                let (kind, selector) = if tool {
                    (TOOL_REQUEST, COMBINED_TOOL)
                } else {
                    (PLAN_REQUEST, COMBINED_PLAN)
                };
                (kind, selector, COMBINED_REQUEST_BYTES)
            }
            Self::CheckpointSearch { operation, goal } => {
                buffer[PLAN_FRAME_BYTES..LOOKUP_BYTES].copy_from_slice(&operation);
                buffer[LOOKUP_BYTES..CHECKPOINT_SEARCH_BYTES].copy_from_slice(&goal.to_le_bytes());
                (TOOL_REQUEST, CHECKPOINT_SEARCH, CHECKPOINT_SEARCH_BYTES)
            }
            Self::CheckpointLookup { operation } => {
                buffer[PLAN_FRAME_BYTES..LOOKUP_BYTES].copy_from_slice(&operation);
                (TOOL_REQUEST, CHECKPOINT_LOOKUP, LOOKUP_BYTES)
            }
        };
        buffer[..3].copy_from_slice(&[length as u8, VERSION, kind]);
        buffer[3..HEADER_BYTES].copy_from_slice(&request_id.to_le_bytes());
        buffer[HEADER_BYTES] = selector;
        &buffer[..length]
    }
}

#[derive(Clone, Copy)]
#[repr(u8)]
pub enum BridgeFailure {
    Unavailable = 1,
    TimedOut = 2,
    InvalidReply = 3,
    BudgetExhausted = 4,
    Skipped = 5,
    ModelUnavailable = 6,
    ToolNotAllowed = 7,
}

impl BridgeFailure {
    pub const fn from_raw(raw: usize) -> Option<Self> {
        match raw {
            1 => Some(Self::Unavailable),
            2 => Some(Self::TimedOut),
            3 => Some(Self::InvalidReply),
            4 => Some(Self::BudgetExhausted),
            5 => Some(Self::Skipped),
            6 => Some(Self::ModelUnavailable),
            7 => Some(Self::ToolNotAllowed),
            _ => None,
        }
    }
}

pub fn request_id(frame: &[u8]) -> Option<u64> {
    Some(u64::from_le_bytes(
        frame.get(3..HEADER_BYTES)?.try_into().ok()?,
    ))
}

pub fn plan_action(frame: &[u8], expected_id: u64) -> Option<u8> {
    let [action] = reply_payload(frame, expected_id, PLAN_REPLY)? else {
        return None;
    };
    Some(*action)
}

pub fn tool_text(frame: &[u8], expected_id: u64) -> Result<&[u8], BridgeFailure> {
    match reply_payload(frame, expected_id, TOOL_REPLY) {
        Some([TOOL_OK, text @ ..]) if valid_text(text) => Ok(text),
        Some([BUDGET_EXHAUSTED]) => Err(BridgeFailure::BudgetExhausted),
        Some([TOOL_UNAVAILABLE]) => Err(BridgeFailure::Unavailable),
        _ => Err(BridgeFailure::InvalidReply),
    }
}

fn valid_text(text: &[u8]) -> bool {
    text.len() <= TOOL_TEXT_BYTES
        && text
            .strip_suffix(b"\n")
            .unwrap_or(text)
            .iter()
            .all(|byte| (b' '..=b'~').contains(byte))
}

/// A settled host search, encoded as status, call charge and bounded tool text.
#[derive(Clone, Copy)]
pub struct SearchReceipt {
    bytes: [u8; 2 + TOOL_TEXT_BYTES],
    length: usize,
}

impl SearchReceipt {
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        match bytes {
            [TOOL_OK, 1, text @ ..] if valid_text(text) => {}
            [BUDGET_EXHAUSTED | TOOL_UNAVAILABLE, 0] | [TOOL_INVALID_REPLY, 1] => {}
            _ => return None,
        }
        let mut receipt = Self {
            bytes: [0; 2 + TOOL_TEXT_BYTES],
            length: bytes.len(),
        };
        receipt.bytes[..receipt.length].copy_from_slice(bytes);
        Some(receipt)
    }

    pub fn encoded(&self) -> &[u8] {
        &self.bytes[..self.length]
    }

    pub fn used_calls(&self) -> usize {
        self.bytes[1].into()
    }

    pub fn reply(&self) -> (ReplyKind, &[u8]) {
        match self.bytes[0] {
            TOOL_OK => (ReplyKind::Matches, &self.encoded()[2..]),
            BUDGET_EXHAUSTED => (
                ReplyKind::HostToolFailed,
                &[BridgeFailure::BudgetExhausted as u8],
            ),
            TOOL_UNAVAILABLE => (
                ReplyKind::HostToolFailed,
                &[BridgeFailure::Unavailable as u8],
            ),
            _ => (
                ReplyKind::HostToolFailed,
                &[BridgeFailure::InvalidReply as u8],
            ),
        }
    }
}

pub fn checkpoint_receipt(
    frame: &[u8],
    expected_id: u64,
) -> Result<Option<SearchReceipt>, BridgeFailure> {
    match reply_payload(frame, expected_id, CHECKPOINT_REPLY) {
        Some([OUTCOME_UNKNOWN]) => Ok(None),
        Some(bytes) => SearchReceipt::decode(bytes)
            .map(Some)
            .ok_or(BridgeFailure::InvalidReply),
        None => Err(BridgeFailure::InvalidReply),
    }
}

fn reply_payload(frame: &[u8], expected_id: u64, kind: u8) -> Option<&[u8]> {
    if !(PLAN_FRAME_BYTES..=MAX_WIRE_BYTES).contains(&frame.len())
        || frame[..3] != [frame.len() as u8, VERSION, kind]
        || request_id(frame)? != expected_id
    {
        return None;
    }
    Some(&frame[HEADER_BYTES..])
}
