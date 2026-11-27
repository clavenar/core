use crate::budget::ReplyBudget;
use crate::tool::ToolId;
use crate::{CompletionKind, NO_CHILD_HANDLE, NO_RAMFS_HANDLE, PATH_MAX_BYTES, Program};

/// Maximum number of successful member launches over the lifetime of a goal.
pub const MAX_CHILDREN: usize = 3;

/// Maximum combined byte length of a canonical source path and a destination basename.
pub const RENAME_DIFF_MAX_BYTES: usize = 512;
pub const MAX_GOAL_RENAMES: usize = 2;
/// Two little-endian u16 lengths precede the names of each staged rename.
pub const RENAME_DIFF_HEADER_BYTES: usize = 4;
pub const RENAME_BATCH_DIFF_MAX_BYTES: usize =
    MAX_GOAL_RENAMES * (RENAME_DIFF_HEADER_BYTES + RENAME_DIFF_MAX_BYTES);

/// Goal lifetime, independent of member completion and the lifetime of a review token.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum GoalDeadline {
    Unlimited,
    At(u64),
    Expired,
}

impl GoalDeadline {
    pub fn after_millis(now_ns: u64, timeout_ms: usize) -> Option<Self> {
        let duration = (timeout_ms as u64).checked_mul(1_000_000)?;
        let at = now_ns.checked_add(duration)?;
        (duration != 0 && at != u64::MAX).then_some(Self::At(at))
    }

    /// Call 35 returns the absolute guest time in nanoseconds, with two reserved values.
    pub const fn register(self) -> usize {
        match self {
            Self::Unlimited => 0,
            Self::At(ns) => ns as usize,
            Self::Expired => usize::MAX,
        }
    }

    pub const fn from_register(value: usize) -> Self {
        match value {
            0 => Self::Unlimited,
            usize::MAX => Self::Expired,
            ns => Self::At(ns as u64),
        }
    }
}

/// A boot-local handle for one reviewed change. Zero denotes the absence of a token.
pub type ReviewToken = core::num::NonZeroUsize;

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct TimedReviewToken {
    pub token: ReviewToken,
    /// Absolute deadline on the guest monotonic clock, in nanoseconds.
    pub deadline_ns: u64,
}

/// Failures encountered while reviewing or publishing a rename.
/// The discriminants are the blocked verdict codes for owner and delegated reviews.
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(usize)]
pub enum PublicationError {
    Conflict = 2,
    PostconditionFailed = 3,
    Unavailable = 4,
}

impl PublicationError {
    pub const fn from_register(value: usize) -> Option<Self> {
        match value {
            2 => Some(Self::Conflict),
            3 => Some(Self::PostconditionFailed),
            4 => Some(Self::Unavailable),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum TimedRenameReview {
    Preview,
    Ready(TimedReviewToken),
    Blocked(PublicationError),
    SupervisorRequired,
}

impl TimedRenameReview {
    /// The owner receives an approval token and deadline, or a zero token and verdict.
    pub const fn registers(self) -> (usize, usize) {
        match self {
            Self::Preview => (0, 0),
            Self::Ready(review) => (review.token.get(), review.deadline_ns as usize),
            Self::Blocked(reason) => (0, reason as usize),
            Self::SupervisorRequired => (0, 5),
        }
    }

    pub const fn from_registers(token: usize, detail: usize) -> Option<Self> {
        match (ReviewToken::new(token), detail) {
            (Some(token), deadline) if deadline != 0 => Some(Self::Ready(TimedReviewToken {
                token,
                deadline_ns: deadline as u64,
            })),
            (None, 0) => Some(Self::Preview),
            (None, 5) => Some(Self::SupervisorRequired),
            (None, verdict) => match PublicationError::from_register(verdict) {
                Some(reason) => Some(Self::Blocked(reason)),
                None => None,
            },
            _ => None,
        }
    }
}

pub type EncodedRenameReviewReceipt = [[u8; 8]; 5];

/// The owner's receipt for a review performed by a single authorized launch.
pub struct RenameReviewReceipt {
    pub reviewer: usize,
    pub goal: usize,
    pub base_revision: u64,
    pub review: TimedRenameReview,
}

impl RenameReviewReceipt {
    pub fn encode(&self) -> EncodedRenameReviewReceipt {
        let (token, detail) = self.review.registers();
        [
            self.reviewer as u64,
            self.goal as u64,
            self.base_revision,
            token as u64,
            detail as u64,
        ]
        .map(u64::to_le_bytes)
    }

    pub fn decode(bytes: &[u8]) -> Option<Self> {
        let (words, []) = bytes.as_chunks::<8>() else {
            return None;
        };
        let words: &EncodedRenameReviewReceipt = words.try_into().ok()?;
        let [reviewer, goal, base_revision, token, detail] = words.map(u64::from_le_bytes);
        let review = TimedRenameReview::from_registers(token as usize, detail as usize)?;
        if reviewer == 0
            || goal == 0
            || !matches!(
                review,
                TimedRenameReview::Ready(_) | TimedRenameReview::Blocked(_)
            )
        {
            return None;
        }
        Some(Self {
            reviewer: reviewer as usize,
            goal: goal as usize,
            base_revision,
            review,
        })
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(usize)]
pub enum CommitResult {
    Committed = 0,
    Conflict = 1,
    PostconditionFailed = 2,
    ReviewExpired = 3,
}

impl CommitResult {
    pub const fn from_register(value: usize) -> Option<Self> {
        Some(match value {
            0 => Self::Committed,
            1 => Self::Conflict,
            2 => Self::PostconditionFailed,
            3 => Self::ReviewExpired,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(usize)]
pub enum DiscardResult {
    Discarded = 0,
    Empty = 1,
}

#[derive(Clone, Copy)]
pub struct RenameDiff<'a> {
    pub source: &'a [u8],
    pub new_name: &'a [u8],
}

pub type RenameBatchDiff<'a> = [Option<RenameDiff<'a>>; MAX_GOAL_RENAMES];

pub fn decode_rename_batch(mut bytes: &[u8], count: usize) -> Option<RenameBatchDiff<'_>> {
    let mut changes = [None; MAX_GOAL_RENAMES];
    for change in changes.get_mut(..count)? {
        let (&[source_lo, source_hi, name_lo, name_hi], names) =
            bytes.split_first_chunk::<RENAME_DIFF_HEADER_BYTES>()?;
        let source_len = u16::from_le_bytes([source_lo, source_hi]) as usize;
        let name_len = u16::from_le_bytes([name_lo, name_hi]) as usize;
        let total = source_len + name_len;
        if !(1..=PATH_MAX_BYTES).contains(&name_len) || total > RENAME_DIFF_MAX_BYTES {
            return None;
        }
        let (entry, remaining) = names.split_at_checked(total)?;
        let (source, new_name) = entry.split_at(source_len);
        if !source.starts_with(b"/") {
            return None;
        }
        *change = Some(RenameDiff { source, new_name });
        bytes = remaining;
    }
    bytes.is_empty().then_some(changes)
}

/// Cumulative RamFS byte limits for the whole goal and for its delegated helpers together.
#[derive(Clone, Copy, Default)]
pub struct ReadLimits {
    pub goal: Option<usize>,
    pub helpers: Option<usize>,
}

/// Owner's descriptors to bind at goal creation for rename scope and content preservation.
#[derive(Clone, Copy, Default)]
pub struct RenameOptions {
    pub directory: Option<usize>,
    pub preserve_files: [Option<usize>; MAX_GOAL_RENAMES],
}

/// Owner's descriptors for the independent RamFS policies of one goal.
#[derive(Clone, Copy, Default)]
pub struct FileOptions {
    pub read_directory: Option<usize>,
    pub rename: RenameOptions,
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

    /// The first four policy words hold the child limit, flags, reply limit and selector.
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

    /// Bit 2 enables the shared RamFS read limit, including a zero-byte limit.
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

    /// Bit 3 enables a helper read limit independently of the goal limit.
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

    /// Bit 4 uses the selector word for an exact tool ID. Bit 1 uses it for a program.
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

/// Goal creation policy. The second tool ID follows the file and timeout fields.
pub struct CreatePolicy {
    pub options: CreateOptions,
    pub read_limits: ReadLimits,
    pub allowed_tools: [Option<ToolId>; 2],
    /// Only the bound echo supervisor can approve publication of a rename.
    pub require_rename_review: bool,
    pub rename_limit: usize,
}

impl CreatePolicy {
    const REQUIRE_RENAME_REVIEW: usize = 1 << 5;
    const ALLOW_TWO_RENAMES: usize = 1 << 6;

    pub fn registers(&self) -> Option<[usize; 6]> {
        if let [first, Some(second)] = self.allowed_tools
            && (first.is_none() || first == Some(second) || second.0 == 0)
        {
            return None;
        }
        let mut words = self
            .options
            .registers_with_allowed_tool(self.read_limits, self.allowed_tools[0])?;
        if self.require_rename_review {
            words[1] |= Self::REQUIRE_RENAME_REVIEW;
        }
        match self.rename_limit {
            1 => {}
            MAX_GOAL_RENAMES => words[1] |= Self::ALLOW_TWO_RENAMES,
            _ => return None,
        }
        Some(words)
    }

    pub fn from_registers(mut words: [usize; 6], second_tool: Option<ToolId>) -> Option<Self> {
        let require_rename_review = words[1] & Self::REQUIRE_RENAME_REVIEW != 0;
        let rename_limit = if words[1] & Self::ALLOW_TWO_RENAMES != 0 {
            MAX_GOAL_RENAMES
        } else {
            1
        };
        words[1] &= !(Self::REQUIRE_RENAME_REVIEW | Self::ALLOW_TWO_RENAMES);
        let (options, read_limits, allowed_tool) =
            CreateOptions::from_registers_with_allowed_tool(words)?;
        if second_tool.is_some() && (allowed_tool.is_none() || allowed_tool == second_tool) {
            return None;
        }
        Some(Self {
            options,
            read_limits,
            allowed_tools: [allowed_tool, second_tool],
            require_rename_review,
            rename_limit,
        })
    }
}

const GOAL_CREATE_V1_WORDS: usize = 11;

/// Version 3 adds a second exact tool ID to the version 2 request.
pub type EncodedGoalCreate = [[u8; 8]; GOAL_CREATE_V1_WORDS + 2];

pub struct GoalCreateRequest {
    pub policy: CreatePolicy,
    pub files: FileOptions,
    pub timeout_ms: Option<usize>,
}

impl GoalCreateRequest {
    const VERSION: u64 = 3;

    pub fn encode(&self) -> Option<EncodedGoalCreate> {
        let [first_file, second_file] = self.files.rename.preserve_files;
        let handles = [
            self.files.read_directory,
            self.files.rename.directory,
            first_file,
            second_file,
        ];
        if handles.contains(&Some(NO_RAMFS_HANDLE)) || self.timeout_ms == Some(0) {
            return None;
        }
        let [read_directory, rename_directory, first_file, second_file] =
            handles.map(|handle| handle.unwrap_or(NO_RAMFS_HANDLE) as u64);
        let [children, flags, replies, selector, reads, helper_reads] =
            self.policy.registers()?.map(|word| word as u64);
        Some(
            [
                Self::VERSION,
                children,
                flags,
                replies,
                selector,
                reads,
                helper_reads,
                read_directory,
                rename_directory,
                first_file,
                self.timeout_ms.unwrap_or(0) as u64,
                second_file,
                self.policy.allowed_tools[1].map_or(0, |tool| tool.0),
            ]
            .map(u64::to_le_bytes),
        )
    }

    pub fn decode(encoded: &[[u8; 8]]) -> Option<Self> {
        let (prefix, extra) = encoded.split_first_chunk::<GOAL_CREATE_V1_WORDS>()?;
        let [
            version,
            policy @ ..,
            read_directory,
            rename_directory,
            first_file,
            timeout_ms,
        ] = prefix.map(u64::from_le_bytes);
        let handle = |word: u64| (word != NO_RAMFS_HANDLE as u64).then_some(word as usize);
        let (second_file, second_tool) = match (version, extra) {
            (1, []) => (None, None),
            (2, [file]) => (handle(u64::from_le_bytes(*file)), None),
            (Self::VERSION, [file, tool]) => {
                let tool = u64::from_le_bytes(*tool);
                (
                    handle(u64::from_le_bytes(*file)),
                    (tool != 0).then_some(ToolId(tool)),
                )
            }
            _ => return None,
        };
        Some(Self {
            policy: CreatePolicy::from_registers(policy.map(|word| word as usize), second_tool)?,
            files: FileOptions {
                read_directory: handle(read_directory),
                rename: RenameOptions {
                    directory: handle(rename_directory),
                    preserve_files: [handle(first_file), second_file],
                },
            },
            timeout_ms: (timeout_ms != 0).then_some(timeout_ms as usize),
        })
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

/// Call 35's four status words followed by call 43's four reply-budget words.
pub type EncodedGoalObservation = [[u8; 8]; 8];

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct GoalObservation {
    pub status: GoalStatus,
    pub deadline: GoalDeadline,
    pub budget: ReplyBudget,
}

impl GoalObservation {
    pub fn encode(self) -> EncodedGoalObservation {
        let (state, child, detail) = self.status.registers();
        let (flags, available, reserved, spent) = self.budget.registers();
        [
            state,
            child,
            detail,
            self.deadline.register(),
            flags,
            available,
            reserved,
            spent,
        ]
        .map(|word| (word as u64).to_le_bytes())
    }

    pub fn decode(bytes: &[u8]) -> Option<Self> {
        let (words, []) = bytes.as_chunks::<8>() else {
            return None;
        };
        let words: &EncodedGoalObservation = words.try_into().ok()?;
        let [
            state,
            child,
            detail,
            deadline,
            flags,
            available,
            reserved,
            spent,
        ] = words.map(|word| u64::from_le_bytes(word) as usize);
        Some(Self {
            status: GoalStatus::from_registers(state, child, detail)?,
            deadline: GoalDeadline::from_register(deadline),
            budget: ReplyBudget::from_registers(flags, available, reserved, spent)?,
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

/// Execution can already be finished while a private rename still awaits publication.
pub struct CancelOutcome {
    pub execution: CancelResult,
    pub rename_discarded: bool,
}

impl CancelOutcome {
    pub const fn from_registers(execution: usize, rename_discarded: usize) -> Option<Self> {
        let execution = match execution {
            0 => CancelResult::Cancelled,
            1 => CancelResult::AlreadyFinished,
            _ => return None,
        };
        let rename_discarded = match rename_discarded {
            0 => false,
            1 => true,
            _ => return None,
        };
        Some(Self {
            execution,
            rename_discarded,
        })
    }
}
