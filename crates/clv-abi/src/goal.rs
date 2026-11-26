use crate::{CompletionKind, NO_CHILD_HANDLE, Program};

pub const MAX_CHILDREN: usize = 2;

/// Immutable options for goal creation. The helper restriction applies to delegation.
#[derive(Clone, Copy)]
pub struct CreateOptions {
    pub child_limit: usize,
    pub reply_limit: Option<usize>,
    pub allowed_helper: Option<Program>,
}

impl CreateOptions {
    const REPLY_LIMIT: usize = 1;
    const ALLOWED_HELPER: usize = 2;
    const READ_LIMIT: usize = 4;

    /// Call 48 takes the child limit, option flags, reply limit and helper selector in a0–a3.
    pub fn registers(self) -> (usize, usize, usize, usize) {
        let mut flags = 0;
        if self.reply_limit.is_some() {
            flags |= Self::REPLY_LIMIT;
        }
        if self.allowed_helper.is_some() {
            flags |= Self::ALLOWED_HELPER;
        }
        (
            self.child_limit,
            flags,
            self.reply_limit.unwrap_or(0),
            self.allowed_helper.map_or(0, |program| program as usize),
        )
    }

    pub fn from_registers(
        child_limit: usize,
        flags: usize,
        reply_limit: usize,
        selector: usize,
    ) -> Option<Self> {
        if flags & !(Self::REPLY_LIMIT | Self::ALLOWED_HELPER) != 0 {
            return None;
        }
        let allowed_helper = if flags & Self::ALLOWED_HELPER != 0 {
            Some(Program::from_raw(selector)?)
        } else {
            None
        };
        Some(Self {
            child_limit,
            reply_limit: (flags & Self::REPLY_LIMIT != 0).then_some(reply_limit),
            allowed_helper,
        })
    }

    /// Bit 2 enables a shared RamFS read limit in a4, including a zero-byte limit.
    pub fn registers_with_read_limit(
        self,
        limit: Option<usize>,
    ) -> (usize, usize, usize, usize, usize) {
        let (children, flags, replies, helper) = self.registers();
        let flags = if limit.is_some() {
            flags | Self::READ_LIMIT
        } else {
            flags
        };
        (children, flags, replies, helper, limit.unwrap_or(0))
    }

    pub fn from_registers_with_read_limit(
        children: usize,
        flags: usize,
        replies: usize,
        helper: usize,
        limit: usize,
    ) -> Option<(Self, Option<usize>)> {
        let options = Self::from_registers(children, flags & !Self::READ_LIMIT, replies, helper)?;
        Some((options, (flags & Self::READ_LIMIT != 0).then_some(limit)))
    }
}

/// Which helpers may receive a read-only copy of a goal agent's file handle.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum FileRecipient {
    Any,
    Program(Program),
}

const READY: usize = 0;
const RUNNING: usize = 1;
const COMPLETED: usize = 2;
const FAILED_EXIT: usize = 3;
const FAILED_FAULT: usize = 4;
const CANCELLED: usize = 5;

/// A non-consuming goal snapshot. The child handle identifies the member reporting the state.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum GoalStatus {
    Ready,
    Running {
        child: usize,
    },
    Completed {
        child: usize,
    },
    /// The kernel reports a nonzero exit status or a fault cause here.
    Failed {
        child: usize,
        kind: CompletionKind,
        value: usize,
    },
    Cancelled {
        child: Option<usize>,
    },
}

impl GoalStatus {
    /// The RV64 status result in a0 (state), a1 (child) and a2 (exit code or cause).
    pub const fn registers(self) -> (usize, usize, usize) {
        match self {
            Self::Ready => (READY, NO_CHILD_HANDLE, 0),
            Self::Running { child } => (RUNNING, child, 0),
            Self::Completed { child } => (COMPLETED, child, 0),
            Self::Failed {
                child,
                kind: CompletionKind::Exited,
                value,
            } => (FAILED_EXIT, child, value),
            Self::Failed {
                child,
                kind: CompletionKind::Faulted,
                value,
            } => (FAILED_FAULT, child, value),
            Self::Cancelled { child: Some(child) }
            | Self::Failed {
                child,
                kind: CompletionKind::Cancelled,
                ..
            } => (CANCELLED, child, 0),
            Self::Cancelled { child: None } => (CANCELLED, NO_CHILD_HANDLE, 0),
        }
    }

    pub const fn from_registers(tag: usize, child: usize, value: usize) -> Option<Self> {
        Some(match tag {
            READY => Self::Ready,
            RUNNING => Self::Running { child },
            COMPLETED => Self::Completed { child },
            FAILED_EXIT => Self::Failed {
                child,
                kind: CompletionKind::Exited,
                value,
            },
            FAILED_FAULT => Self::Failed {
                child,
                kind: CompletionKind::Faulted,
                value,
            },
            CANCELLED if child == NO_CHILD_HANDLE => Self::Cancelled { child: None },
            CANCELLED => Self::Cancelled { child: Some(child) },
            _ => return None,
        })
    }
}

#[repr(usize)]
pub enum CloseResult {
    Closed = 0,
    Busy = 1,
}

#[repr(usize)]
pub enum CancelResult {
    Cancelled = 0,
    AlreadyFinished = 1,
}
