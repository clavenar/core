#![no_std]

//! Syscall numbers and register values for the Clavenar boot user ABI.

pub mod agent;
pub mod audit;
pub mod bridge;
pub mod budget;
pub mod goal;
pub mod net;
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
pub const SYS_GOAL_BUDGET: usize = 43;
pub const SYS_GOAL_RUN_WITH_REPLY_BUDGET: usize = 44;
pub const SYS_AGENT_RUN_WITH_REPLY_BUDGET: usize = 45;
pub const SYS_GOAL_GRANT_FILE: usize = 46;
pub const SYS_AGENT_DELEGATE: usize = 47;
pub const SYS_GOAL_GRANT_DELEGABLE_FILE: usize = 48;
pub const SYS_AGENT_DELEGATE_WITH_FILE: usize = 49;
pub const SYS_GOAL_AUDIT: usize = 50;
pub const SYS_GOAL_REPLY_AUDIT: usize = 51;
pub const SYS_GOAL_GRANT_FILE_TO: usize = 52;
pub const SYS_SPAWN_WITH_FILE_ARG: usize = 53;
pub const SYS_AGENT_DELEGATE_WITH_FILE_ARG: usize = 54;
pub const SYS_GOAL_READ_AUDIT: usize = 55;
pub const SYS_AGENT_RUN_WITH_REQUEST: usize = 56;
pub const SYS_GOAL_REVOKE_FILE: usize = 57;
pub const SYS_GOAL_GRANT_FILE_TO_WITH_ARG: usize = 58;
pub const SYS_AGENT_STATUS_WITH_DEFINITION: usize = 59;
pub const SYS_AGENT_INVOKE_FILE_TOOL: usize = 60;
pub const SYS_GOAL_STAGE_RENAME: usize = 61;
pub const SYS_GOAL_COMMIT: usize = 62;
pub const SYS_GOAL_GRANT_FILE_FOR_RENAME: usize = 63;
pub const SYS_AGENT_STAGE_RENAME: usize = 64;
pub const SYS_GOAL_RENAME_AUDIT: usize = 65;
pub const SYS_GOAL_DISCARD: usize = 66;
pub const SYS_GOAL_LAUNCH_OBSERVER: usize = 67;
pub const SYS_GOAL_OBSERVE: usize = 68;
pub const SYS_AGENT_LAUNCH_SUPERVISOR: usize = 69;
pub const SYS_AGENT_RESUME_GRANTED: usize = 70;
pub const SYS_GOAL_MONITOR: usize = 71;
pub const SYS_GOAL_LAUNCH_RENAME_REVIEWER: usize = 72;
pub const SYS_GOAL_PUBLICATION_DECISIONS_WITH_REVIEWER: usize = 73;
pub const SYS_GOAL_RENAME_DIFF: usize = 74;
pub const SYS_GOAL_REVIEW_BATCH_GRANTED: usize = 75;
pub const SYS_HOST_BRIDGE: usize = 76;
pub const SYS_NET_FRAME: usize = 77;

/// Call 15 starts the child with stdin and stdout pipes when `a1` has this value.
pub const SPAWN_STDIO: usize = 1;

/// Stream selected by `pipe_close` in `a1`.
#[derive(Clone, Copy)]
#[repr(usize)]
pub enum PipeStream {
    Stdout = 0,
    Stdin = 1,
}

impl PipeStream {
    pub const fn from_register(value: usize) -> Option<Self> {
        match value {
            0 => Some(Self::Stdout),
            1 => Some(Self::Stdin),
            _ => None,
        }
    }
}

/// No RamFS handle has this value. Handle 0 is valid.
pub const NO_RAMFS_HANDLE: usize = usize::MAX;
pub const NO_DIRECTORY_GRANT: usize = NO_RAMFS_HANDLE;

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
    /// Empty assignment to report a goal snapshot using the calling launch's grant.
    ObserveGoal = 8,
    /// Empty assignment to resume the agent bound to this launch.
    ResumeAgent = 9,
    /// Empty assignment to review the rename bound to this launch.
    ReviewRename = 10,
    StageRenamePair = 11,
    /// The approved literal for a registered tool launched with stdio pipes.
    FileToolStdio = 12,
    /// The approved literal for a registered tool launched as a local MCP server.
    FileToolMcp = 13,
    /// Empty assignment to report the disposition of the bound rename transaction.
    ObservePublication = 14,
    /// The denial threshold for the bound read-scope watch.
    WatchReadDenials = 15,
    HttpsProbe = 16,
    McpInit = 17,
    McpList = 18,
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
            value if value == Self::ObserveGoal as usize => Some(Self::ObserveGoal),
            value if value == Self::ResumeAgent as usize => Some(Self::ResumeAgent),
            value if value == Self::ReviewRename as usize => Some(Self::ReviewRename),
            value if value == Self::StageRenamePair as usize => Some(Self::StageRenamePair),
            value if value == Self::FileToolStdio as usize => Some(Self::FileToolStdio),
            value if value == Self::FileToolMcp as usize => Some(Self::FileToolMcp),
            value if value == Self::ObservePublication as usize => Some(Self::ObservePublication),
            value if value == Self::WatchReadDenials as usize => Some(Self::WatchReadDenials),
            value if value == Self::HttpsProbe as usize => Some(Self::HttpsProbe),
            value if value == Self::McpInit as usize => Some(Self::McpInit),
            value if value == Self::McpList as usize => Some(Self::McpList),
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
    /// Only `goal_observe` can queue this kernel-generated report.
    GoalObservation = 8,
    /// Only `agent_resume_granted` can queue this kernel-generated receipt.
    AgentResumed = 9,
    /// Only `goal_review_granted` can queue this receipt for the goal owner.
    RenameReviewed = 10,
    /// One byte explaining why the guest could not obtain an approved host plan.
    HostPlanFailed = 11,
    /// One byte explaining why the guest could not obtain a host tool result.
    HostToolFailed = 12,
    /// Only `goal_observe` can queue this kernel-generated publication snapshot.
    PublicationObserved = 13,
    /// The read-denial monitor's alert carrying the original event.
    ReadDenialAlert = 14,
    /// One byte explaining why the agent could not fetch the approved HTTPS fixture.
    HttpsFailed = 15,
    /// One byte explaining why the agent could not initialize native MCP.
    McpInitFailed = 16,
    McpListFailed = 17,
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
            value if value == Self::GoalObservation as usize => Some(Self::GoalObservation),
            value if value == Self::AgentResumed as usize => Some(Self::AgentResumed),
            value if value == Self::RenameReviewed as usize => Some(Self::RenameReviewed),
            value if value == Self::HostPlanFailed as usize => Some(Self::HostPlanFailed),
            value if value == Self::HostToolFailed as usize => Some(Self::HostToolFailed),
            value if value == Self::PublicationObserved as usize => Some(Self::PublicationObserved),
            value if value == Self::ReadDenialAlert as usize => Some(Self::ReadDenialAlert),
            value if value == Self::HttpsFailed as usize => Some(Self::HttpsFailed),
            value if value == Self::McpInitFailed as usize => Some(Self::McpInitFailed),
            value if value == Self::McpListFailed as usize => Some(Self::McpListFailed),
            _ => None,
        }
    }
}

/// A child receives its RamFS grant handle in `a1` and its kind in `a2`.
/// A launch without a grant sets `a2` to 0.
/// A file grant with a pipe also supplies the write endpoint in `a3`.
/// A rename pair supplies its second file handle in `a3`.
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(usize)]
pub enum GrantKind {
    Directory = 1,
    File = 2,
    RenamePair = 3,
}

pub const PATH_MAX_BYTES: usize = 64;
pub const MESSAGE_MAX_BYTES: usize = 64;
pub const PIPE_CAPACITY: usize = 32;
pub const FILE_REPLY_OK: u8 = 0;
pub const FILE_REPLY_ERROR: u8 = 1;

/// Maximum `p_memsz` of the read/execute segment in an RV64 boot user ELF.
pub const USER_IMAGE_MAX_BYTES: usize = 64 * 1024;

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
