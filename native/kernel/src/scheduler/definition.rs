use clv_abi::RequestKind;
use clv_abi::agent::{DefinitionId, EncodedDefinitionPin, FIND_GREETING_V1};

use crate::ipc::{Message, MessageKind};

static FIND_GREETING_PIN: &EncodedDefinitionPin =
    include_bytes!(concat!(env!("OUT_DIR"), "/find-greeting-v1.pin"));

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
            let id = DefinitionId::from_request(kind, message.bytes(false));
            if id == Some(FIND_GREETING_V1) && program == FIND_GREETING_V1.intent.program() as usize
            {
                definition = Some(FIND_GREETING_PIN);
            } else if kind == RequestKind::PlanDefinition {
                // An explicit revision must resolve before this agent can be admitted.
                return None;
            }
        }
        Some(Self { handle, definition })
    }
}
