use crate::{CompletionKind, NO_CHILD_HANDLE};

const READY: usize = 0;
const RUNNING: usize = 1;
const COMPLETED: usize = 2;
const FAILED_EXIT: usize = 3;
const FAILED_FAULT: usize = 4;
const CANCELLED: usize = 5;

/// A non-consuming snapshot of one goal and its original child launch.
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
