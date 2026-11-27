use clv_abi::RequestKind;
use clv_abi::agent::{
    DefinitionId, EncodedDefinitionPin, FIND_GREETING_V1, FIND_GREETING_V2, FIND_GREETING_V3,
    FIND_GREETING_V4,
};

use crate::ipc::{Message, MessageKind};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct AgentBinding {
    pub(super) handle: usize,
    pub(super) definition: Option<&'static EncodedDefinitionPin>,
}

impl AgentBinding {
    pub(super) fn resolve(
        handle: usize,
        program: usize,
        request: Option<&Message>,
    ) -> Option<Self> {
        let mut definition = None;
        if let Some(message) = request
            && let MessageKind::Request(kind) = message.kind
        {
            let id = DefinitionId::from_request(kind, message.bytes(false))
                .filter(|id| program == id.intent.program() as usize);
            definition = match id {
                Some(FIND_GREETING_V1) => Some(include_bytes!(concat!(
                    env!("OUT_DIR"),
                    "/find-greeting-v1.pin"
                ))),
                Some(FIND_GREETING_V2) => Some(include_bytes!(concat!(
                    env!("OUT_DIR"),
                    "/find-greeting-v2.pin"
                ))),
                Some(FIND_GREETING_V3) => Some(include_bytes!(concat!(
                    env!("OUT_DIR"),
                    "/find-greeting-v3.pin"
                ))),
                Some(FIND_GREETING_V4) => Some(include_bytes!(concat!(
                    env!("OUT_DIR"),
                    "/find-greeting-v4.pin"
                ))),
                _ => None,
            };
            if definition.is_none() && kind == RequestKind::PlanDefinition {
                // An explicit revision must resolve before this agent can be admitted.
                return None;
            }
        }
        Some(Self { handle, definition })
    }
}
