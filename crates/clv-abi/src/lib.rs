#![no_std]

//! Syscall numbers and register values for the Clavenar boot user ABI.

pub mod agent;
pub mod audit;
pub mod budget;
pub mod goal;
mod program;
pub mod tool;
pub use program::{PROGRAMS, Program};

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
pub const SYS_GOAL_GRANT_DELEGABLE_FILE: usize = 50;
pub const SYS_AGENT_DELEGATE_WITH_FILE: usize = 51;
pub const SYS_GOAL_AUDIT: usize = 52;
pub const SYS_GOAL_REPLY_AUDIT: usize = 53;
pub const SYS_GOAL_GRANT_FILE_TO: usize = 54;
pub const SYS_SPAWN_WITH_FILE_ARG: usize = 55;
pub const SYS_AGENT_DELEGATE_WITH_FILE_ARG: usize = 56;
pub const SYS_GOAL_READ_AUDIT: usize = 57;
pub const SYS_AGENT_RUN_WITH_REQUEST: usize = 58;
pub const SYS_GOAL_AUDIT_WITH_CALLER: usize = 59;
pub const SYS_GOAL_REVOKE_FILE: usize = 60;
pub const SYS_GOAL_READ_AUDIT_WITH_LIMIT: usize = 61;
pub const SYS_GOAL_GRANT_FILE_TO_WITH_ARG: usize = 62;
pub const SYS_AGENT_STATUS_WITH_DEFINITION: usize = 63;
pub const SYS_AGENT_INVOKE_FILE_TOOL: usize = 64;
pub const SYS_GOAL_AUDIT_WITH_TOOL: usize = 65;
pub const SYS_GOAL_STAGE_RENAME: usize = 66;
pub const SYS_GOAL_DIFF: usize = 67;
pub const SYS_GOAL_REVIEW: usize = 68;
pub const SYS_GOAL_COMMIT: usize = 69;
pub const SYS_GOAL_CREATE_PRESERVING_FILE: usize = 70;
pub const SYS_GOAL_GRANT_FILE_FOR_RENAME: usize = 71;
pub const SYS_AGENT_STAGE_RENAME: usize = 72;
pub const SYS_GOAL_RENAME_AUDIT: usize = 73;
pub const SYS_GOAL_DISCARD: usize = 74;

/// No RamFS handle has this value. Handle 0 is valid.
pub const NO_DIRECTORY_GRANT: usize = usize::MAX;

#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(usize)]
pub enum RequestKind {
    Echo = 1,
    ReadFile = 2,
    Argument = 3,
    DelegateWithFile = 4,
    PlanIntent = 5,
    PlanDefinition = 6,
    StageRename = 7,
}

impl RequestKind {
    pub const fn from_raw(value: usize) -> Option<Self> {
        match value {
            value if value == Self::Echo as usize => Some(Self::Echo),
            value if value == Self::ReadFile as usize => Some(Self::ReadFile),
            value if value == Self::Argument as usize => Some(Self::Argument),
            value if value == Self::DelegateWithFile as usize => Some(Self::DelegateWithFile),
            value if value == Self::PlanIntent as usize => Some(Self::PlanIntent),
            value if value == Self::PlanDefinition as usize => Some(Self::PlanDefinition),
            value if value == Self::StageRename as usize => Some(Self::StageRename),
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
    Matches = 5,
    ResultTooLarge = 6,
    DelegationFailed = 7,
}

impl ReplyKind {
    pub const fn from_raw(value: usize) -> Option<Self> {
        match value {
            value if value == Self::Echo as usize => Some(Self::Echo),
            value if value == Self::FileData as usize => Some(Self::FileData),
            value if value == Self::Denied as usize => Some(Self::Denied),
            value if value == Self::ReadFailed as usize => Some(Self::ReadFailed),
            value if value == Self::Matches as usize => Some(Self::Matches),
            value if value == Self::ResultTooLarge as usize => Some(Self::ResultTooLarge),
            value if value == Self::DelegationFailed as usize => Some(Self::DelegationFailed),
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

pub const PROGRAM_HELLO: usize = Program::Hello as usize;
pub const PROGRAM_ECHO: usize = Program::Echo as usize;

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

/// The caller that owns a child's reply and wait result.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum LaunchParent {
    Shell,
    /// The agent's original child launch handle, independent of program slot reuse.
    Agent(usize),
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
