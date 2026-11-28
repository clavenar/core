mod host;

use clv_abi::agent::{
    AgentLaunch, CheckpointReply, CheckpointResumeError, DefinitionPin, EncodedDefinitionPin,
    FIND_GREETING_V2,
};
use clv_abi::bridge::{CheckpointId, SearchReceipt};
use clv_abi::goal::{
    CreateOptions, CreatePolicy, GoalDeadline, GoalStatus, ReadLimits, RenameOptions,
};
use clv_abi::tool::TEXT_SEARCH_V1;
use clv_abi::{LaunchParent, Program, ReplyKind, TaskCompletion};

use super::{Goal, GoalReads, MemberRole, ReplyReservation};
use crate::console::write_hex;
use crate::drivers::uart;
use crate::drivers::virtio::block::{
    JournalDevice, JournalDisk, JournalLoadError, JournalSaveError,
};
use crate::fs::ramfs::{HELLO_FILE_CONTENTS, RamFs, RenamePolicy, TaskFs};
use crate::ipc::{Message, MessageKind};
use crate::scheduler::definition::{AgentBinding, FIND_GREETING_V2_PIN, FIND_GREETING_V7_PIN};
use crate::scheduler::{LaunchPipes, SHELL_TASK_INDEX, Scheduler, TaskState, task_index};

use self::host::{HOST_COMPLETED, HOST_PENDING, HostCheckpoint};

const PAUSED: u8 = 1;
const CONSUMED: u8 = 2;
const COMPLETED: u8 = 3;
const CANCELLED: u8 = 4;
const PIN_BYTES: usize = size_of::<EncodedDefinitionPin>();
const BUDGET_BYTES: usize = 5 * size_of::<u64>();
const REPLY_BYTES: usize = HELLO_FILE_CONTENTS.len();
const PLAN_REPLY_BUDGET: usize = 2 * REPLY_BYTES;
const RESULT_RECORD_BYTES: usize = 1 + PIN_BYTES + BUDGET_BYTES + REPLY_BYTES;

#[derive(Clone, Copy)]
struct GreetingCheckpoint {
    definition: EncodedDefinitionPin,
    read_limit: usize,
    read_spent: usize,
    reply_limit: usize,
    reply_spent: usize,
    reply_reserved: usize,
    reply: [u8; REPLY_BYTES],
}

impl GreetingCheckpoint {
    fn capture(goal: &Goal, binding: AgentBinding, message: &Message) -> Option<Self> {
        if message.kind != MessageKind::Reply(ReplyKind::Matches) {
            return None;
        }
        let (read_limit, read_spent) = goal.reads.checkpoint_counts()?;
        let budget = goal.budget();
        let checkpoint = Self {
            definition: *binding.definition?,
            read_limit,
            read_spent,
            reply_limit: goal.options.reply_limit?,
            reply_spent: budget.spent,
            reply_reserved: budget.reserved?,
            reply: message.bytes(false).try_into().ok()?,
        };
        (checkpoint.valid() && checkpoint.valid_paused()).then_some(checkpoint)
    }

    fn valid(&self) -> bool {
        DefinitionPin::decode(&self.definition).is_some_and(|pin| pin.id == FIND_GREETING_V2)
            && self.reply == HELLO_FILE_CONTENTS
            && self.read_spent == REPLY_BYTES
            && self.read_spent <= self.read_limit
            && self
                .reply_spent
                .checked_add(self.reply_reserved)
                .is_some_and(|charged| charged <= self.reply_limit)
    }

    fn valid_paused(&self) -> bool {
        self.reply_spent == REPLY_BYTES && self.reply_reserved >= REPLY_BYTES
    }

    fn completed(self) -> Self {
        // The paused record reserves the bytes for forwarding this reply.
        Self {
            reply_spent: self.reply_spent + self.reply.len(),
            reply_reserved: 0,
            ..self
        }
    }

    fn encode(self, tag: u8) -> [u8; RESULT_RECORD_BYTES] {
        let mut bytes = [0; RESULT_RECORD_BYTES];
        bytes[0] = tag;
        bytes[1..1 + PIN_BYTES].copy_from_slice(&self.definition);
        let (budget, reply) = bytes[1 + PIN_BYTES..].split_at_mut(BUDGET_BYTES);
        for (field, value) in budget.chunks_exact_mut(8).zip([
            self.read_limit,
            self.read_spent,
            self.reply_limit,
            self.reply_spent,
            self.reply_reserved,
        ]) {
            field.copy_from_slice(&(value as u64).to_le_bytes());
        }
        reply.copy_from_slice(&self.reply);
        bytes
    }

    fn decode(bytes: &[u8]) -> Option<Self> {
        let (definition, rest) = bytes.split_first_chunk::<PIN_BYTES>()?;
        let (budget, reply) = rest.split_at_checked(BUDGET_BYTES)?;
        let (
            [
                read_limit,
                read_spent,
                reply_limit,
                reply_spent,
                reply_reserved,
            ],
            [],
        ) = budget.as_chunks::<8>()
        else {
            return None;
        };
        let count = |bytes: &[u8; 8]| usize::try_from(u64::from_le_bytes(*bytes)).ok();
        let checkpoint = Self {
            definition: *definition,
            read_limit: count(read_limit)?,
            read_spent: count(read_spent)?,
            reply_limit: count(reply_limit)?,
            reply_spent: count(reply_spent)?,
            reply_reserved: count(reply_reserved)?,
            reply: reply.try_into().ok()?,
        };
        checkpoint.valid().then_some(checkpoint)
    }

    fn definition_is_current(&self) -> bool {
        &self.definition == FIND_GREETING_V2_PIN
    }
}

enum CheckpointRecord {
    Paused(GreetingCheckpoint),
    Completed(GreetingCheckpoint),
    Host(HostCheckpoint, Option<SearchReceipt>),
    Cancelled,
    // Older kernels used this marker for either completion or cancellation.
    Consumed,
}

impl CheckpointRecord {
    fn decode(payload: &[u8], operation: Option<CheckpointId>) -> Option<Self> {
        if let Some(operation) = operation {
            return match payload {
                [HOST_PENDING | HOST_COMPLETED, ..] => HostCheckpoint::decode(payload, operation)
                    .map(|(pending, receipt)| Self::Host(pending, receipt)),
                [CANCELLED] => Some(Self::Cancelled),
                _ => None,
            };
        }
        match payload {
            [PAUSED, bytes @ ..] => {
                let checkpoint = GreetingCheckpoint::decode(bytes)?;
                checkpoint
                    .valid_paused()
                    .then_some(Self::Paused(checkpoint))
            }
            [COMPLETED, bytes @ ..] => {
                let checkpoint = GreetingCheckpoint::decode(bytes)?;
                (checkpoint.reply_spent == PLAN_REPLY_BUDGET && checkpoint.reply_reserved == 0)
                    .then_some(Self::Completed(checkpoint))
            }
            [CANCELLED] => Some(Self::Cancelled),
            [CONSUMED] => Some(Self::Consumed),
            _ => None,
        }
    }

    fn reply(&self) -> Option<CheckpointReply<'_>> {
        match self {
            Self::Completed(saved) if saved.definition_is_current() => Some(CheckpointReply {
                kind: ReplyKind::Matches,
                bytes: &saved.reply,
                read_spent: saved.read_spent,
                reply_spent: saved.reply_spent,
                host_calls: None,
            }),
            Self::Host(saved, Some(receipt)) if saved.definition_is_current() => {
                let (kind, bytes) = receipt.reply();
                Some(CheckpointReply {
                    kind,
                    bytes,
                    read_spent: 0,
                    reply_spent: bytes.len(),
                    host_calls: Some(receipt.used_calls()),
                })
            }
            _ => None,
        }
    }

    fn message(&self, slot: usize) -> Option<(Message, TaskCompletion)> {
        let reply = self.reply()?;
        let status = match reply.kind {
            ReplyKind::Matches => usize::from(reply.bytes.is_empty()),
            _ => 2,
        };
        let message = Message::new(
            slot + 1,
            MessageKind::Reply(reply.kind),
            reply.bytes.len(),
            |bytes| {
                bytes.copy_from_slice(reply.bytes);
                true
            },
        )?;
        Some((message, TaskCompletion::Exited(status)))
    }
}

struct RecoveredGoal {
    definition: &'static EncodedDefinitionPin,
    read_limit: usize,
    reply_limit: usize,
    reply_reserved: usize,
}

pub(in crate::scheduler) enum CheckpointLaunch {
    Local,
    Host(HostCheckpoint),
}

pub(in crate::scheduler) struct CheckpointJournal {
    disk: JournalDisk,
    operation: Option<CheckpointId>,
    record: Option<CheckpointRecord>,
}

impl CheckpointJournal {
    fn paused(&self) -> Option<GreetingCheckpoint> {
        match self.record {
            Some(CheckpointRecord::Paused(checkpoint)) => Some(checkpoint),
            _ => None,
        }
    }

    fn host_pending(&self) -> Option<HostCheckpoint> {
        match self.record {
            Some(CheckpointRecord::Host(pending, None)) => Some(pending),
            _ => None,
        }
    }

    fn save(&mut self, record: CheckpointRecord) -> Option<()> {
        let result = match &record {
            CheckpointRecord::Paused(checkpoint) => self.disk.save(&checkpoint.encode(PAUSED)),
            CheckpointRecord::Completed(checkpoint) => {
                self.disk.save(&checkpoint.encode(COMPLETED))
            }
            CheckpointRecord::Host(pending, receipt) => {
                let (bytes, length) = pending.encode(receipt.as_ref());
                self.disk.save(&bytes[..length])
            }
            CheckpointRecord::Cancelled => self.disk.save(&[CANCELLED]),
            CheckpointRecord::Consumed => unreachable!("legacy checkpoint marker is read-only"),
        };
        match result {
            Ok(_) => {
                self.record = Some(record);
                Some(())
            }
            Err(JournalSaveError::Failed) => {
                uart::write(b"agent checkpoint storage failed\r\n");
                None
            }
            Err(JournalSaveError::OutcomeUnknown(attempt)) => {
                uart::write(b"agent checkpoint storage outcome unknown generation=");
                write_hex(attempt.generation as usize);
                uart::write(b" checksum=");
                write_hex(attempt.checksum as usize);
                uart::write(b"\r\n");
                None
            }
        }
    }
}

impl Scheduler {
    pub(crate) fn attach_checkpoint_disk(&mut self, disk: JournalDevice) {
        let operation = disk.checkpoint_operation();
        let (disk, record) = match disk
            .open_journal(|payload| CheckpointRecord::decode(payload, operation))
        {
            Ok(recovered) => recovered,
            Err(error) => {
                uart::write(match error {
                    JournalLoadError::TooSmall => {
                        b"agent checkpoint journal needs three sectors\r\n"
                    }
                    JournalLoadError::InvalidSlots => b"agent checkpoint journal invalid slots\r\n",
                    JournalLoadError::Io(_) => b"agent checkpoint journal read failed\r\n",
                });
                return;
            }
        };
        if let Some(entry) = disk.latest_record() {
            // A paused record occupies the first slot and a terminal record occupies the second.
            let expected = if matches!(
                record,
                Some(CheckpointRecord::Paused(_) | CheckpointRecord::Host(_, None))
            ) {
                1
            } else {
                2
            };
            if entry.generation != expected || entry.sector != expected {
                uart::write(b"agent checkpoint journal invalid generation\r\n");
                return;
            }
        }
        uart::write(match &record {
            Some(CheckpointRecord::Consumed) => b"agent checkpoint consumed\r\n",
            Some(CheckpointRecord::Cancelled) => b"agent checkpoint cancelled\r\n",
            Some(CheckpointRecord::Paused(checkpoint)) if checkpoint.definition_is_current() => {
                b"agent checkpoint recovered (paused)\r\n"
            }
            Some(CheckpointRecord::Completed(checkpoint)) if checkpoint.definition_is_current() => {
                b"agent checkpoint completed\r\n"
            }
            Some(CheckpointRecord::Host(pending, receipt)) if pending.definition_is_current() => {
                if receipt.is_some() {
                    b"agent host checkpoint completed\r\n"
                } else {
                    b"agent host checkpoint recovered (pending)\r\n"
                }
            }
            Some(
                CheckpointRecord::Paused(_)
                | CheckpointRecord::Completed(_)
                | CheckpointRecord::Host(..),
            ) => b"agent checkpoint definition mismatch\r\n",
            None if disk.is_writable() => b"agent checkpoint journal ready\r\n",
            None => b"agent checkpoint journal writes unavailable\r\n",
        });
        self.checkpoint_disk = Some(CheckpointJournal {
            disk,
            operation,
            record,
        });
    }

    pub(in crate::scheduler) fn check_checkpoint_launch(
        &self,
        handle: usize,
        binding: AgentBinding,
        reply_bytes: Option<usize>,
        suspended: bool,
    ) -> Option<CheckpointLaunch> {
        let journal = self.checkpoint_disk.as_ref()?;
        let goal = self.shell_goal(handle)?;
        if journal.record.is_some()
            || !journal.disk.is_writable()
            || goal.status() != GoalStatus::Ready
            || goal.deadline != GoalDeadline::Unlimited
        {
            return None;
        }
        if let Some(operation) = journal.operation {
            if suspended || self.host_bridge.is_none() {
                return None;
            }
            return HostCheckpoint::prepare(operation, goal, binding, reply_bytes)
                .map(CheckpointLaunch::Host);
        }
        let (read_limit, _) = goal.reads.checkpoint_counts()?;
        let reply_limit = goal.options.reply_limit?;
        let input = goal.file_grants[0].as_ref()?;
        if binding.definition != Some(FIND_GREETING_V2_PIN)
            || goal.options.child_limit != 2
            || read_limit < REPLY_BYTES
            || reply_bytes.unwrap_or(reply_limit) < PLAN_REPLY_BUDGET
            || !input.delegates_boot_file(b"hello.txt", Program::Grep, b"Hello")
        {
            return None;
        }
        goal.helper_program(Program::Grep as usize, Some(TEXT_SEARCH_V1.id))
            .ok()?;
        Some(CheckpointLaunch::Local)
    }

    pub(in crate::scheduler) fn arm_checkpoint(&mut self, child: usize) {
        self.goal
            .as_mut()
            .expect("agent started in a goal")
            .checkpoint_child = Some(child);
    }

    pub(in crate::scheduler) fn checkpoint_after_helper_exit(
        &mut self,
        index: usize,
        completion: TaskCompletion,
        fs: &mut RamFs,
    ) {
        if index != task_index(Program::Grep) {
            return;
        }
        let Some(launch) = self.tasks[index].launch.as_ref() else {
            return;
        };
        let LaunchParent::Agent(child) = launch.parent else {
            return;
        };
        let Some((goal, _, binding)) = self.live_agent_member(child) else {
            return;
        };
        if goal.checkpoint_child != Some(child) {
            return;
        }
        let parent = self
            .launch_index(child)
            .expect("live checkpoint agent has a slot");
        let saved = match (completion, &launch.reply) {
            (TaskCompletion::Exited(0), Some(reply)) => {
                GreetingCheckpoint::capture(goal, binding, reply)
            }
            _ => None,
        };
        let Some(saved) = saved else {
            uart::write(b"agent checkpoint search incomplete\r\n");
            self.stop_task(parent, fs, TaskCompletion::Exited(2));
            return;
        };
        let journal = self
            .checkpoint_disk
            .as_mut()
            .expect("checkpoint launch required a journal");
        if journal.save(CheckpointRecord::Paused(saved)).is_some() {
            // The helper has exited and the parent has yet to forward its reply.
            self.tasks[parent].state = TaskState::Suspended;
            // A wait that began while this child could finish must return to the owner.
            let shell = &mut self.tasks[SHELL_TASK_INDEX].state;
            if shell.waits_for_result(child) {
                *shell = TaskState::Runnable;
            }
            uart::write(b"agent checkpoint saved (paused)\r\n");
        } else {
            self.stop_task(parent, fs, TaskCompletion::Exited(2));
        }
    }

    pub(super) fn has_live_checkpoint(&self, child: usize) -> bool {
        self.goal
            .as_ref()
            .is_some_and(|goal| goal.checkpoint_child == Some(child))
            && self.checkpoint_disk.as_ref().is_some_and(|journal| {
                journal.paused().is_some() || journal.host_pending().is_some()
            })
    }

    pub(super) fn cancel_checkpoint(&mut self) -> Option<()> {
        let journal = self.checkpoint_disk.as_mut()?;
        if journal.paused().is_none() && journal.host_pending().is_none() {
            return None;
        }
        journal.save(CheckpointRecord::Cancelled)
    }

    pub(crate) fn checkpoint_reply(&self) -> Option<CheckpointReply<'_>> {
        if self.current != SHELL_TASK_INDEX {
            return None;
        }
        self.checkpoint_disk.as_ref()?.record.as_ref()?.reply()
    }

    pub(super) fn complete_live_checkpoint(
        &mut self,
        slot: usize,
        fs: &mut RamFs,
    ) -> Result<(), CheckpointResumeError> {
        let journal = self
            .checkpoint_disk
            .as_ref()
            .ok_or(CheckpointResumeError::Unavailable)?;
        if let Some(pending) = journal.host_pending() {
            return self.complete_live_host_checkpoint(slot, fs, pending);
        }
        let saved = journal.paused().ok_or(CheckpointResumeError::Unavailable)?;
        self.finish_live_checkpoint(
            slot,
            fs,
            FIND_GREETING_V2_PIN,
            CheckpointRecord::Completed(saved.completed()),
        )
        .ok_or(CheckpointResumeError::Unavailable)
    }

    fn finish_live_checkpoint(
        &mut self,
        slot: usize,
        fs: &mut RamFs,
        definition: &EncodedDefinitionPin,
        record: CheckpointRecord,
    ) -> Option<()> {
        let (reply, completion) = record.message(slot)?;
        let launch = self.tasks[slot].launch.as_ref()?;
        let (_, member, binding) = self.live_agent_member(launch.handle)?;
        if launch.reply.is_some()
            || binding.definition != Some(definition)
            || member
                .replies
                .remaining()
                .is_none_or(|remaining| remaining < reply.bytes(false).len())
        {
            return None;
        }
        self.checkpoint_disk.as_mut()?.save(record)?;
        self.publish_checkpoint_reply(slot, fs, reply, completion);
        Some(())
    }

    pub(crate) fn resume_recovered_checkpoint(
        &mut self,
        fs: &mut RamFs,
    ) -> Result<(usize, AgentLaunch), CheckpointResumeError> {
        if self.current != SHELL_TASK_INDEX || self.goal.is_some() {
            return Err(CheckpointResumeError::Unavailable);
        }
        let journal = self
            .checkpoint_disk
            .as_ref()
            .ok_or(CheckpointResumeError::Unavailable)?;
        let (recovered, completed) = if let Some(pending) = journal.host_pending() {
            let receipt = self.reconcile_host_checkpoint(pending)?;
            (
                RecoveredGoal {
                    definition: FIND_GREETING_V7_PIN,
                    read_limit: 0,
                    reply_limit: pending.reply_limit,
                    reply_reserved: pending.reply_reserved,
                },
                CheckpointRecord::Host(pending, Some(receipt)),
            )
        } else {
            let saved = journal.paused().ok_or(CheckpointResumeError::Unavailable)?;
            (
                RecoveredGoal {
                    definition: FIND_GREETING_V2_PIN,
                    read_limit: saved.read_limit,
                    reply_limit: saved.reply_limit,
                    reply_reserved: saved.reply_reserved,
                },
                CheckpointRecord::Completed(saved.completed()),
            )
        };
        self.restore_checkpoint(fs, recovered, completed)
            .ok_or(CheckpointResumeError::Unavailable)
    }

    fn restore_checkpoint(
        &mut self,
        fs: &mut RamFs,
        recovered: RecoveredGoal,
        completed: CheckpointRecord,
    ) -> Option<(usize, AgentLaunch)> {
        let handle = self.next_handle;
        let agent = handle.checked_add(1)?;
        let launch = self
            .prepare_launch(Program::Hello, Some(agent), LaunchParent::Shell, None)
            .ok()?;
        let (reply, completion) = completed.message(launch.slot)?;
        let saved = completed.reply()?;
        let binding = AgentBinding {
            handle: agent,
            definition: Some(recovered.definition),
        };
        let policy = CreatePolicy {
            options: CreateOptions {
                child_limit: 1,
                reply_limit: Some(recovered.reply_limit),
                allowed_helper: None,
            },
            allowed_tools: [None, None],
            read_limits: ReadLimits::default(),
            durable: false,
            require_rename_review: false,
            rename_limit: 1,
            allow_https_probe: false,
            allow_mcp_init: false,
        };
        let rename_policy = RenamePolicy::capture(
            &self.tasks[self.current].fs,
            fs,
            RenameOptions::default(),
            None,
        )?;
        let reads = GoalReads::restored(recovered.read_limit, saved.read_spent);
        let mut goal = Goal::new(
            handle,
            policy,
            GoalDeadline::Unlimited,
            reads,
            rename_policy,
        );
        let slot = launch.slot;
        let child = launch.handle;
        let spent_before_reply = saved.reply_spent.checked_sub(saved.bytes.len())?;
        let reservation = ReplyReservation::restored(recovered.reply_reserved, spent_before_reply);
        // Prepare the reply and handles before committing completion. Installation cannot fail.
        self.checkpoint_disk.as_mut()?.save(completed)?;
        goal.admit(0, child, MemberRole::Agent(binding), reservation, fs);
        self.goal = Some(goal);
        self.install_child(
            launch,
            TaskFs::empty(),
            None,
            LaunchPipes::default(),
            TaskState::Suspended,
        );
        self.publish_checkpoint_reply(slot, fs, reply, completion);
        Some((handle, AgentLaunch { agent, child }))
    }

    fn publish_checkpoint_reply(
        &mut self,
        slot: usize,
        fs: &mut RamFs,
        reply: Message,
        completion: TaskCompletion,
    ) {
        let launch = self.tasks[slot]
            .launch
            .as_mut()
            .expect("checkpoint child is installed");
        self.goal
            .as_mut()
            .expect("checkpoint child has a goal")
            .charge_reply(launch.handle, reply.bytes(false).len())
            .expect("checkpoint reply fits its reserved budget");
        launch.reply = Some(reply);
        self.stop_task(slot, fs, completion);
        self.wake_goal_monitor();
    }
}
