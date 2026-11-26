use crate::{Program, ReplyKind};

/// RV64 wire identity. The high 32 bits identify the tool and the low 32 its revision.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct ToolId(pub u64);

/// Each file tool receives one delegated read-only file and a bounded literal argument.
pub struct FileTool {
    pub id: ToolId,
    pub name: &'static str,
    pub program: Program,
    pub success_reply: ReplyKind,
}

pub const TEXT_SEARCH_V1: FileTool = FileTool {
    id: ToolId((1 << 32) | 1),
    name: "text.search@1",
    program: Program::Grep,
    success_reply: ReplyKind::Matches,
};

pub const FILE_TOOLS: &[FileTool] = &[TEXT_SEARCH_V1];
