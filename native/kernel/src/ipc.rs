pub(crate) mod pipe;

use clv_abi::agent::{
    DELEGATION_HEADER_BYTES, DELEGATION_REQUEST_MAX_BYTES, EncodedResumeReceipt,
    LAUNCH_REQUEST_MAX_BYTES,
};
use clv_abi::audit::{EncodedPublicationObservation, EncodedReadDenialEvent};
use clv_abi::goal::{EncodedGoalObservation, EncodedRenameReviewReceipt};
use clv_abi::{
    FILE_REPLY_ERROR, FILE_REPLY_OK, MESSAGE_MAX_BYTES, PATH_MAX_BYTES, ReplyKind, RequestKind,
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum MessageKind {
    Request(RequestKind),
    Reply(ReplyKind),
}

impl MessageKind {
    pub(crate) fn tag(self) -> usize {
        match self {
            Self::Request(kind) => kind as usize,
            Self::Reply(kind) => kind as usize,
        }
    }

    fn accepts_length(self, length: usize) -> bool {
        match self {
            Self::Request(RequestKind::Echo | RequestKind::ReadFile)
            | Self::Reply(ReplyKind::Echo) => (1..=MESSAGE_MAX_BYTES).contains(&length),
            Self::Request(
                RequestKind::Argument | RequestKind::FileToolStdio | RequestKind::FileToolMcp,
            )
            | Self::Reply(ReplyKind::FileData | ReplyKind::Matches) => length <= MESSAGE_MAX_BYTES,
            Self::Request(RequestKind::DelegateWithFile) => {
                (DELEGATION_HEADER_BYTES..=DELEGATION_REQUEST_MAX_BYTES).contains(&length)
            }
            Self::Request(RequestKind::PlanIntent) => length == 1,
            Self::Request(RequestKind::PlanDefinition) => length == 2,
            Self::Request(RequestKind::StageRename) => (1..=PATH_MAX_BYTES).contains(&length),
            Self::Request(RequestKind::StageRenamePair) => {
                (3..=LAUNCH_REQUEST_MAX_BYTES).contains(&length)
            }
            Self::Request(
                RequestKind::ObserveGoal
                | RequestKind::ObservePublication
                | RequestKind::ResumeAgent
                | RequestKind::ReviewRename
                | RequestKind::HttpsProbe
                | RequestKind::McpInit
                | RequestKind::McpList
                | RequestKind::McpCall,
            ) => length == 0,
            Self::Request(RequestKind::WatchReadDenials) => length == 1,
            Self::Reply(ReplyKind::ReadDenialAlert) => {
                length == size_of::<EncodedReadDenialEvent>()
            }
            Self::Reply(ReplyKind::GoalObservation) => {
                length == size_of::<EncodedGoalObservation>()
            }
            Self::Reply(ReplyKind::AgentResumed) => length == size_of::<EncodedResumeReceipt>(),
            Self::Reply(ReplyKind::PublicationObserved) => {
                length == size_of::<EncodedPublicationObservation>()
            }
            Self::Reply(ReplyKind::RenameReviewed) => {
                length == size_of::<EncodedRenameReviewReceipt>()
            }
            Self::Reply(
                ReplyKind::HostPlanFailed
                | ReplyKind::HostToolFailed
                | ReplyKind::HttpsFailed
                | ReplyKind::McpInitFailed
                | ReplyKind::McpListFailed
                | ReplyKind::McpCallFailed,
            ) => length == 1,
            Self::Reply(
                ReplyKind::Denied
                | ReplyKind::ReadFailed
                | ReplyKind::ResultTooLarge
                | ReplyKind::DelegationFailed,
            ) => length == 0,
        }
    }

    fn legacy_prefix(self) -> &'static [u8] {
        match self {
            Self::Reply(ReplyKind::FileData) => &[FILE_REPLY_OK],
            Self::Reply(ReplyKind::Denied | ReplyKind::ReadFailed) => &[FILE_REPLY_ERROR],
            _ => &[],
        }
    }
}

pub(crate) struct Message {
    pub(crate) kind: MessageKind,
    pub(crate) sender: usize,
    // Also accommodates the legacy status prefix on a full-sized file reply.
    bytes: [u8; LAUNCH_REQUEST_MAX_BYTES],
    wire_length: usize,
}

impl Message {
    pub(crate) fn new(
        sender: usize,
        kind: MessageKind,
        length: usize,
        copy: impl FnOnce(&mut [u8]) -> bool,
    ) -> Option<Self> {
        if !kind.accepts_length(length) {
            return None;
        }
        let prefix = kind.legacy_prefix();
        let mut bytes = [0; LAUNCH_REQUEST_MAX_BYTES];
        let wire_length = prefix.len() + length;
        bytes[..prefix.len()].copy_from_slice(prefix);
        if !copy(&mut bytes[prefix.len()..wire_length]) {
            return None;
        }
        Some(Self {
            kind,
            sender,
            bytes,
            wire_length,
        })
    }

    pub(crate) fn decode_legacy_file_reply(&mut self) {
        match &self.bytes[..self.wire_length] {
            [FILE_REPLY_OK, ..] => self.kind = MessageKind::Reply(ReplyKind::FileData),
            [FILE_REPLY_ERROR] => self.kind = MessageKind::Reply(ReplyKind::ReadFailed),
            _ => {}
        }
    }

    pub(crate) fn bytes(&self, legacy: bool) -> &[u8] {
        if legacy && matches!(self.kind, MessageKind::Reply(_)) {
            &self.bytes[..self.wire_length.min(MESSAGE_MAX_BYTES)]
        } else {
            &self.bytes[self.kind.legacy_prefix().len()..self.wire_length]
        }
    }
}
