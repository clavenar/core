//! Bounded guest-host records and the mock planner's syscall results.

use crate::agent::PlanIntent;

pub const SUBMIT_PLAN: usize = 0;
pub const POLL_PLAN: usize = 1;

pub const MAX_FRAME_BYTES: usize = 32;
/// Length, version, kind and a little-endian u64 request ID.
pub const HEADER_BYTES: usize = 11;
const PLAN_FRAME_BYTES: usize = HEADER_BYTES + 1;
const VERSION: u8 = 1;
const PLAN_REQUEST: u8 = 1;
const PLAN_REPLY: u8 = 2;
pub const BUDGET_EXHAUSTED: u8 = 0;
pub const SEARCH_GREETING: u8 = 1;

#[derive(Clone, Copy)]
#[repr(u8)]
pub enum PlanFailure {
    Unavailable = 1,
    TimedOut = 2,
    InvalidReply = 3,
    BudgetExhausted = 4,
}

impl PlanFailure {
    pub const fn from_raw(raw: usize) -> Option<Self> {
        match raw {
            1 => Some(Self::Unavailable),
            2 => Some(Self::TimedOut),
            3 => Some(Self::InvalidReply),
            4 => Some(Self::BudgetExhausted),
            _ => None,
        }
    }
}

pub fn plan_request(request_id: u64, intent: PlanIntent) -> [u8; PLAN_FRAME_BYTES] {
    let mut frame = [0; PLAN_FRAME_BYTES];
    frame[..3].copy_from_slice(&[PLAN_FRAME_BYTES as u8, VERSION, PLAN_REQUEST]);
    frame[3..HEADER_BYTES].copy_from_slice(&request_id.to_le_bytes());
    frame[HEADER_BYTES] = intent as u8;
    frame
}

pub fn request_id(frame: &[u8]) -> Option<u64> {
    Some(u64::from_le_bytes(
        frame.get(3..HEADER_BYTES)?.try_into().ok()?,
    ))
}

pub fn plan_action(frame: &[u8], expected_id: u64) -> Option<u8> {
    if frame.len() != PLAN_FRAME_BYTES
        || frame[..3] != [PLAN_FRAME_BYTES as u8, VERSION, PLAN_REPLY]
        || request_id(frame)? != expected_id
    {
        return None;
    }
    Some(frame[HEADER_BYTES])
}
