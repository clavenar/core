use crate::tool::ToolId;
use crate::{CompletionKind, NO_CHILD_HANDLE, Program};

/// Maximum number of successful member launches over the lifetime of a goal.
pub const MAX_CHILDREN: usize = 3;

/// Maximum combined byte length of a canonical source path and a destination basename.
pub const RENAME_DIFF_MAX_BYTES: usize = 512;

pub enum GoalDiff<'a> {
    Empty,
    Rename {
        source: &'a [u8],
        new_name: &'a [u8],
    },
}

/// Cumulative RamFS byte limits for the whole goal and for its delegated helpers together.
#[derive(Clone, Copy, Default)]
pub struct ReadLimits {
    pub goal: Option<usize>,
    pub helpers: Option<usize>,
}

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
    const HELPER_READ_LIMIT: usize = 8;
    const ALLOWED_TOOL: usize = 16;

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

    /// Bit 3 enables a helper read limit in a5, independently of the goal limit in a4.
    pub fn registers_with_read_limits(self, limits: ReadLimits) -> [usize; 6] {
        let (children, mut flags, replies, helper, reads) =
            self.registers_with_read_limit(limits.goal);
        if limits.helpers.is_some() {
            flags |= Self::HELPER_READ_LIMIT;
        }
        [
            children,
            flags,
            replies,
            helper,
            reads,
            limits.helpers.unwrap_or(0),
        ]
    }

    pub fn from_registers_with_read_limits(
        [children, flags, replies, helper, reads, helper_reads]: [usize; 6],
    ) -> Option<(Self, ReadLimits)> {
        let (options, goal) = Self::from_registers_with_read_limit(
            children,
            flags & !Self::HELPER_READ_LIMIT,
            replies,
            helper,
            reads,
        )?;
        let helpers = (flags & Self::HELPER_READ_LIMIT != 0).then_some(helper_reads);
        Some((options, ReadLimits { goal, helpers }))
    }

    /// Bit 4 uses a3 for an exact tool ID. Bit 1 uses the same word for a program.
    pub fn registers_with_allowed_tool(
        self,
        limits: ReadLimits,
        allowed_tool: Option<ToolId>,
    ) -> Option<[usize; 6]> {
        let mut registers = self.registers_with_read_limits(limits);
        if let Some(id) = allowed_tool {
            if self.allowed_helper.is_some() {
                return None;
            }
            registers[1] |= Self::ALLOWED_TOOL;
            registers[3] = id.0.try_into().ok()?;
        }
        Some(registers)
    }

    pub fn from_registers_with_allowed_tool(
        mut registers: [usize; 6],
    ) -> Option<(Self, ReadLimits, Option<ToolId>)> {
        let allowed_tool = if registers[1] & Self::ALLOWED_TOOL != 0 {
            if registers[1] & Self::ALLOWED_HELPER != 0 {
                return None;
            }
            registers[1] &= !Self::ALLOWED_TOOL;
            Some(ToolId(registers[3] as u64))
        } else {
            None
        };
        let (options, limits) = Self::from_registers_with_read_limits(registers)?;
        Some((options, limits, allowed_tool))
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
