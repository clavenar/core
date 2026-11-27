//! Bounded guest-host records for planning and tool calls.

pub const SUBMIT: usize = 0;
pub const POLL: usize = 1;
pub const SUBMIT_TOOL: usize = 2;

pub const MAX_FRAME_BYTES: usize = 32;
/// Length, version, kind and a little-endian u64 request ID.
pub const HEADER_BYTES: usize = 11;
const PLAN_FRAME_BYTES: usize = HEADER_BYTES + 1;
const TOOL_REQUEST_BYTES: usize = PLAN_FRAME_BYTES + 8;
const COMBINED_REQUEST_BYTES: usize = TOOL_REQUEST_BYTES + 8;
const VERSION: u8 = 1;
const PLAN_REQUEST: u8 = 1;
const PLAN_REPLY: u8 = 2;
const TOOL_REQUEST: u8 = 3;
const TOOL_REPLY: u8 = 4;
const SEARCH_FIXTURE: u8 = 3;
const COMBINED_PLAN: u8 = 4;
const COMBINED_TOOL: u8 = 5;
const TOOL_OK: u8 = 1;
const TOOL_UNAVAILABLE: u8 = 4;
pub const BUDGET_EXHAUSTED: u8 = 0;
pub const SEARCH_GREETING: u8 = 1;
pub const SKIP: u8 = 3;
pub const MODEL_UNAVAILABLE: u8 = 4;

pub enum HostRequest {
    MockGreeting,
    ModelGreeting,
    SearchFixture { goal: u64 },
    Combined { goal: u64, launch: u64, tool: bool },
}

impl HostRequest {
    pub fn encode(self, request_id: u64, buffer: &mut [u8; MAX_FRAME_BYTES]) -> &[u8] {
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
        Some([TOOL_OK, text @ ..]) => {
            let line = text.strip_suffix(b"\n").unwrap_or(text);
            if line.iter().all(|byte| (b' '..=b'~').contains(byte)) {
                Ok(text)
            } else {
                Err(BridgeFailure::InvalidReply)
            }
        }
        Some([BUDGET_EXHAUSTED]) => Err(BridgeFailure::BudgetExhausted),
        Some([TOOL_UNAVAILABLE]) => Err(BridgeFailure::Unavailable),
        _ => Err(BridgeFailure::InvalidReply),
    }
}

fn reply_payload(frame: &[u8], expected_id: u64, kind: u8) -> Option<&[u8]> {
    if !(PLAN_FRAME_BYTES..=MAX_FRAME_BYTES).contains(&frame.len())
        || frame[..3] != [frame.len() as u8, VERSION, kind]
        || request_id(frame)? != expected_id
    {
        return None;
    }
    Some(&frame[HEADER_BYTES..])
}
