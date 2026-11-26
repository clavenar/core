#![no_std]

//! Syscall numbers and register values for the Clavenar boot user ABI.

pub mod agent;
pub mod budget;
pub mod goal;

pub const SYS_PROBE: usize = 0;
pub const SYS_EXIT: usize = 1;
pub const SYS_WRITE_CONSOLE: usize = 2;
pub const SYS_TIMER_FIRED: usize = 3;
pub const SYS_READ_CONSOLE: usize = 4;
pub const SYS_OPEN: usize = 5;
pub const SYS_READ: usize = 6;
pub const SYS_READDIR: usize = 7;
pub const SYS_CLOSE: usize = 8;
pub const SYS_WRITE: usize = 9;
pub const SYS_UNLINK: usize = 10;
pub const SYS_MKDIR: usize = 11;
pub const SYS_CHDIR: usize = 12;
pub const SYS_GETCWD: usize = 13;
pub const SYS_RMDIR: usize = 14;
pub const SYS_SPAWN: usize = 15;
pub const SYS_WAIT: usize = 16;
pub const SYS_OPEN_AT: usize = 17;
pub const SYS_SPAWN_AT: usize = 18;
pub const SYS_MESSAGE_SEND: usize = 19;
pub const SYS_MESSAGE_RECV: usize = 20;
pub const SYS_SPAWN_WITH_MESSAGE: usize = 21;
pub const SYS_SPAWN_AT_WITH_MESSAGE: usize = 22;
pub const SYS_SPAWN_WITH_FILE: usize = 23;
pub const SYS_SPAWN_REQUEST: usize = 24;
pub const SYS_REQUEST_RECV: usize = 25;
pub const SYS_REPLY_SEND: usize = 26;
pub const SYS_REPLY_RECV: usize = 27;
pub const SYS_SPAWN_WITH_FILE_PIPE: usize = 28;
pub const SYS_PIPE_READ: usize = 29;
pub const SYS_PIPE_WRITE: usize = 30;
pub const SYS_PIPE_CLOSE: usize = 31;
pub const SYS_EVENT_WAIT: usize = 32;
pub const SYS_GOAL_CREATE: usize = 33;
pub const SYS_GOAL_RUN: usize = 34;
pub const SYS_GOAL_STATUS: usize = 35;
pub const SYS_GOAL_CLOSE: usize = 36;
pub const SYS_GOAL_CANCEL: usize = 37;
pub const SYS_AGENT_RUN: usize = 38;
pub const SYS_AGENT_STATUS: usize = 39;
pub const SYS_AGENT_SUSPEND: usize = 40;
pub const SYS_AGENT_RESUME: usize = 41;
pub const SYS_AGENT_RUN_SUSPENDED: usize = 42;
pub const SYS_GOAL_CREATE_WITH_REPLY_BUDGET: usize = 43;
pub const SYS_GOAL_BUDGET: usize = 44;
pub const SYS_GOAL_RUN_WITH_REPLY_BUDGET: usize = 45;
pub const SYS_AGENT_RUN_WITH_REPLY_BUDGET: usize = 46;
pub const SYS_GOAL_GRANT_FILE: usize = 47;
pub const SYS_GOAL_CREATE_WITH_OPTIONS: usize = 48;
pub const SYS_AGENT_DELEGATE: usize = 49;

/// No RamFS handle has this value. Handle 0 is valid.
pub const NO_DIRECTORY_GRANT: usize = usize::MAX;

#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(usize)]
pub enum RequestKind {
    Echo = 1,
    ReadFile = 2,
}

impl RequestKind {
    pub const fn from_raw(value: usize) -> Option<Self> {
        match value {
            value if value == Self::Echo as usize => Some(Self::Echo),
            value if value == Self::ReadFile as usize => Some(Self::ReadFile),
            _ => None,
        }
    }
}

/// Zero is reserved for an empty reply mailbox.
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(usize)]
pub enum ReplyKind {
    Echo = 1,
    FileData = 2,
    Denied = 3,
    ReadFailed = 4,
}

impl ReplyKind {
    pub const fn from_raw(value: usize) -> Option<Self> {
        match value {
            value if value == Self::Echo as usize => Some(Self::Echo),
            value if value == Self::FileData as usize => Some(Self::FileData),
            value if value == Self::Denied as usize => Some(Self::Denied),
            value if value == Self::ReadFailed as usize => Some(Self::ReadFailed),
            _ => None,
        }
    }
}

/// A child receives its RamFS grant handle in `a1` and its kind in `a2`.
/// A launch without a grant sets `a2` to 0.
/// A file grant with a pipe also supplies the write endpoint in `a3`.
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(usize)]
pub enum GrantKind {
    Directory = 1,
    File = 2,
}

pub const PATH_MAX_BYTES: usize = 64;
pub const MESSAGE_MAX_BYTES: usize = 64;
pub const PIPE_CAPACITY: usize = 32;
pub const FILE_REPLY_OK: u8 = 0;
pub const FILE_REPLY_ERROR: u8 = 1;
pub const PROGRAM_HELLO: usize = 0;
pub const PROGRAM_ECHO: usize = 1;
/// A child passes zero in `a2` to receive its request. An empty event wait returns it in `a0`.
pub const NO_CHILD_HANDLE: usize = 0;
pub const WAIT_EXITED: usize = 0;
pub const WAIT_FAULTED: usize = 1;
pub const WAIT_CANCELLED: usize = 2;

/// Completion tags shared by `wait` and `event_wait`.
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(usize)]
pub enum CompletionKind {
    Exited = WAIT_EXITED,
    Faulted = WAIT_FAULTED,
    Cancelled = WAIT_CANCELLED,
}

impl CompletionKind {
    pub const fn from_raw(value: usize) -> Option<Self> {
        match value {
            WAIT_EXITED => Some(Self::Exited),
            WAIT_FAULTED => Some(Self::Faulted),
            WAIT_CANCELLED => Some(Self::Cancelled),
            _ => None,
        }
    }
}

/// The result collected by `wait`. Cancellation carries no exit status or fault cause.
#[derive(Clone, Copy)]
pub enum TaskCompletion {
    Exited(usize),
    Faulted(usize),
    Cancelled,
}

impl TaskCompletion {
    pub const fn kind_and_value(self) -> (CompletionKind, usize) {
        match self {
            Self::Exited(status) => (CompletionKind::Exited, status),
            Self::Faulted(cause) => (CompletionKind::Faulted, cause),
            Self::Cancelled => (CompletionKind::Cancelled, 0),
        }
    }
}

/// The oldest pending child completion, or an empty queue when no child can progress.
pub enum ChildEvent {
    Empty,
    Completed { handle: usize, kind: CompletionKind },
}

pub const OPEN_READ: usize = 0;
pub const OPEN_WRITE_TRUNCATE: usize = 1;
pub const SYSCALL_ERROR: usize = usize::MAX;
pub const PROBE_VALUE: usize = 42;
