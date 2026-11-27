use clv_abi::NO_CHILD_HANDLE;
use clv_abi::agent::{
    CheckpointResumeError, DefinitionPin, EncodedDefinitionPin, FIND_GREETING_V7,
};
use clv_abi::bridge::{
    CheckpointId, HostRequest, SearchReceipt, TOOL_TEXT_BYTES, checkpoint_receipt,
};
use clv_abi::tool::HOST_TEXT_SEARCH_V1;

use super::{CheckpointRecord, Goal, TaskCompletion};
use crate::arch::monotonic_nanos;
use crate::drivers::uart;
use crate::fs::ramfs::RamFs;
use crate::scheduler::Scheduler;
use crate::scheduler::definition::{AgentBinding, FIND_GREETING_V7_PIN};

pub(super) const HOST_PENDING: u8 = 5;
pub(super) const HOST_COMPLETED: u8 = 6;
const OPERATION_BYTES: usize = size_of::<CheckpointId>();
const PIN_BYTES: usize = size_of::<EncodedDefinitionPin>();
const FIELDS_BYTES: usize = 1 + OPERATION_BYTES + PIN_BYTES + 3 * size_of::<u64>();
const RECORD_BYTES: usize = FIELDS_BYTES + 2 + TOOL_TEXT_BYTES;
const LOOKUP_TIMEOUT_NS: u64 = 10_000_000_000;

#[derive(Clone, Copy)]
pub(in crate::scheduler) struct HostCheckpoint {
    operation: CheckpointId,
    definition: EncodedDefinitionPin,
    pub(super) reply_limit: usize,
    pub(super) reply_reserved: usize,
}

impl HostCheckpoint {
    pub(super) fn prepare(
        operation: CheckpointId,
        goal: &Goal,
        binding: AgentBinding,
        reply_bytes: Option<usize>,
    ) -> Option<Self> {
        let reply_limit = goal.options.reply_limit?;
        let reply_reserved = reply_bytes.unwrap_or(reply_limit);
        if binding.definition != Some(FIND_GREETING_V7_PIN)
            || goal.options.child_limit != 1
            || goal.reads.checkpoint_counts()? != (0, 0)
            || !goal.allowed_tools.contains(&Some(HOST_TEXT_SEARCH_V1))
            || goal.file_grants.iter().any(Option::is_some)
            || !(TOOL_TEXT_BYTES..=reply_limit).contains(&reply_reserved)
        {
            return None;
        }
        Some(Self {
            operation,
            definition: *FIND_GREETING_V7_PIN,
            reply_limit,
            reply_reserved,
        })
    }

    pub(super) fn definition_is_current(&self) -> bool {
        &self.definition == FIND_GREETING_V7_PIN
    }

    pub(super) fn encode(&self, receipt: Option<&SearchReceipt>) -> ([u8; RECORD_BYTES], usize) {
        let mut bytes = [0; RECORD_BYTES];
        let tag = if receipt.is_some() {
            HOST_COMPLETED
        } else {
            HOST_PENDING
        };
        let mut output = bytes.as_mut_slice();
        for field in [
            &[tag][..],
            &self.operation,
            &self.definition,
            &HOST_TEXT_SEARCH_V1.0.to_le_bytes(),
            &(self.reply_limit as u64).to_le_bytes(),
            &(self.reply_reserved as u64).to_le_bytes(),
        ] {
            let (field_bytes, rest) = output.split_at_mut(field.len());
            field_bytes.copy_from_slice(field);
            output = rest;
        }
        let receipt = receipt.map_or(&[][..], SearchReceipt::encoded);
        output[..receipt.len()].copy_from_slice(receipt);
        (bytes, FIELDS_BYTES + receipt.len())
    }

    pub(super) fn decode(
        bytes: &[u8],
        expected: CheckpointId,
    ) -> Option<(Self, Option<SearchReceipt>)> {
        let (tag, fields) = bytes.split_first()?;
        let (operation, fields) = fields.split_first_chunk::<OPERATION_BYTES>()?;
        let (definition, fields) = fields.split_first_chunk::<PIN_BYTES>()?;
        let (tool, fields) = fields.split_first_chunk::<8>()?;
        let (limit, fields) = fields.split_first_chunk::<8>()?;
        let (reserved, result) = fields.split_first_chunk::<8>()?;
        let reply_limit = usize::try_from(u64::from_le_bytes(*limit)).ok()?;
        let reply_reserved = usize::try_from(u64::from_le_bytes(*reserved)).ok()?;
        if *operation != expected
            || u64::from_le_bytes(*tool) != HOST_TEXT_SEARCH_V1.0
            || DefinitionPin::decode(definition)?.id != FIND_GREETING_V7
            || !(TOOL_TEXT_BYTES..=reply_limit).contains(&reply_reserved)
        {
            return None;
        }
        let receipt = match *tag {
            HOST_PENDING if result.is_empty() => None,
            HOST_COMPLETED => Some(SearchReceipt::decode(result)?),
            _ => return None,
        };
        Some((
            Self {
                operation: *operation,
                definition: *definition,
                reply_limit,
                reply_reserved,
            },
            receipt,
        ))
    }
}

impl Scheduler {
    pub(in crate::scheduler) fn dispatch_host_checkpoint(
        &mut self,
        pending: HostCheckpoint,
        goal: usize,
        child: usize,
        fs: &mut RamFs,
    ) {
        let journal = self
            .checkpoint_disk
            .as_mut()
            .expect("host checkpoint launch requires a journal");
        if journal
            .save(CheckpointRecord::Host(pending, None))
            .is_none()
        {
            let slot = self
                .launch_index(child)
                .expect("host checkpoint child is installed");
            self.stop_task(slot, fs, TaskCompletion::Exited(2));
            self.wake_goal_monitor();
            return;
        }
        let request = HostRequest::CheckpointSearch {
            operation: pending.operation,
            goal: goal as u64,
        };
        let bridge = self
            .host_bridge
            .as_mut()
            .expect("host checkpoint launch requires a bridge");
        let deadline = monotonic_nanos().saturating_add(LOOKUP_TIMEOUT_NS);
        if bridge.submit(child, child, request, deadline).is_ok() {
            // Dispatch has no reply. The owner explicitly reconciles the durable receipt.
            bridge.release(child);
            uart::write(b"agent host checkpoint saved (pending)\r\n");
        } else {
            uart::write(b"agent host checkpoint dispatch unavailable\r\n");
        }
    }

    pub(super) fn reconcile_host_checkpoint(
        &mut self,
        pending: HostCheckpoint,
    ) -> Result<SearchReceipt, CheckpointResumeError> {
        use CheckpointResumeError::{OutcomeUnknown, Unavailable, Waiting};

        let journal = self.checkpoint_disk.as_ref().ok_or(Unavailable)?;
        if !pending.definition_is_current() || !journal.disk.is_writable() {
            return Err(Unavailable);
        }
        let bridge = self.host_bridge.as_mut().ok_or(OutcomeUnknown)?;
        // The shell owns this lookup, independently of any boot-local agent handle.
        if !bridge.is_pending() {
            let id = self.next_handle;
            let next_handle = id.checked_add(1).ok_or(Unavailable)?;
            let request = HostRequest::CheckpointLookup {
                operation: pending.operation,
            };
            let deadline = monotonic_nanos().saturating_add(LOOKUP_TIMEOUT_NS);
            bridge
                .submit(NO_CHILD_HANDLE, id, request, deadline)
                .map_err(|_| OutcomeUnknown)?;
            self.next_handle = next_handle;
            return Err(Waiting);
        }
        let mut receipt = Err(OutcomeUnknown);
        let length = bridge
            .poll(NO_CHILD_HANDLE, |id, frame| {
                receipt = checkpoint_receipt(frame, id)
                    .map_err(|_| OutcomeUnknown)
                    .and_then(|receipt| receipt.ok_or(OutcomeUnknown));
                true
            })
            .map_err(|_| OutcomeUnknown)?;
        if length == 0 { Err(Waiting) } else { receipt }
    }

    pub(super) fn complete_live_host_checkpoint(
        &mut self,
        slot: usize,
        fs: &mut RamFs,
        pending: HostCheckpoint,
    ) -> Result<(), CheckpointResumeError> {
        use CheckpointResumeError::Unavailable;

        let launch = self.tasks[slot].launch.as_ref().ok_or(Unavailable)?;
        let (goal, member, binding) = self.live_agent_member(launch.handle).ok_or(Unavailable)?;
        if binding.definition != Some(FIND_GREETING_V7_PIN)
            || !goal.allowed_tools.contains(&Some(HOST_TEXT_SEARCH_V1))
            || member.replies.remaining() != Some(pending.reply_reserved)
        {
            return Err(Unavailable);
        }
        let receipt = self.reconcile_host_checkpoint(pending)?;
        self.finish_live_checkpoint(
            slot,
            fs,
            FIND_GREETING_V7_PIN,
            CheckpointRecord::Host(pending, Some(receipt)),
        )
        .ok_or(Unavailable)
    }
}
