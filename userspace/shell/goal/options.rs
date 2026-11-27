use crate::command::GoalFlag;
use crate::{number, split_literal, split_word};
use clv_abi::Program;
use clv_abi::goal::{MAX_CHILDREN, ReadLimits};
use clv_abi::tool::{FileTool, ToolId};

pub(crate) enum GoalCommand {
    Create,
    Run,
    AgentRun,
    AgentDelegate,
}

#[derive(Default)]
pub(crate) struct GoalOptions<'a> {
    pub children: Option<usize>,
    pub timeout_ms: Option<usize>,
    pub reply_bytes: Option<usize>,
    pub read_limits: ReadLimits,
    pub read_under: Option<&'a [u8]>,
    pub allowed_helper: Option<Program>,
    pub allowed_tool: Option<ToolId>,
    pub preserve_file: Option<&'a [u8]>,
    pub rename_under: Option<&'a [u8]>,
    pub suspended: bool,
    pub with_file: bool,
}

impl<'a> GoalOptions<'a> {
    pub fn parse(mut arguments: &'a [u8], command: GoalCommand) -> Option<(Self, &'a [u8])> {
        let mut options = Self::default();
        loop {
            let (word, rest) = split_word(arguments);
            if !word.starts_with(b"--") {
                break;
            }
            let flag = GoalFlag::parse(word);
            arguments = match flag {
                GoalFlag::ReplyBytes => parse_count(rest, &mut options.reply_bytes)?,
                GoalFlag::Children if matches!(command, GoalCommand::Create) => {
                    parse_count(rest, &mut options.children)?
                }
                GoalFlag::TimeoutMs if matches!(command, GoalCommand::Create) => {
                    parse_count(rest, &mut options.timeout_ms)?
                }
                GoalFlag::ReadBytes if matches!(command, GoalCommand::Create) => {
                    parse_count(rest, &mut options.read_limits.goal)?
                }
                GoalFlag::ReadUnder if matches!(command, GoalCommand::Create) => {
                    parse_path(rest, &mut options.read_under)?
                }
                GoalFlag::HelperReadBytes if matches!(command, GoalCommand::Create) => {
                    parse_count(rest, &mut options.read_limits.helpers)?
                }
                GoalFlag::AllowHelper | GoalFlag::AllowTool
                    if matches!(command, GoalCommand::Create)
                        && options.allowed_helper.is_none()
                        && options.allowed_tool.is_none() =>
                {
                    let (name, rest) = split_word(rest);
                    if matches!(flag, GoalFlag::AllowTool) {
                        options.allowed_tool = Some(FileTool::named(name)?.id);
                    } else {
                        options.allowed_helper = Some(Program::named(name)?);
                    }
                    rest
                }
                GoalFlag::PreserveFile if matches!(command, GoalCommand::Create) => {
                    parse_path(rest, &mut options.preserve_file)?
                }
                GoalFlag::RenameUnder if matches!(command, GoalCommand::Create) => {
                    parse_path(rest, &mut options.rename_under)?
                }
                GoalFlag::Suspended
                    if matches!(command, GoalCommand::AgentRun) && !options.suspended =>
                {
                    options.suspended = true;
                    rest
                }
                GoalFlag::WithFile
                    if matches!(command, GoalCommand::AgentDelegate) && !options.with_file =>
                {
                    options.with_file = true;
                    rest
                }
                _ => return None,
            };
        }
        if options.timeout_ms == Some(0)
            || options
                .children
                .is_some_and(|n| !(1..=MAX_CHILDREN).contains(&n))
        {
            return None;
        }
        Some((options, arguments))
    }
}

fn parse_count<'a>(arguments: &'a [u8], slot: &mut Option<usize>) -> Option<&'a [u8]> {
    let (digits, rest) = split_word(arguments);
    if slot.is_some() {
        return None;
    }
    *slot = Some(number(digits)?);
    Some(rest)
}

fn parse_path<'a>(arguments: &'a [u8], slot: &mut Option<&'a [u8]>) -> Option<&'a [u8]> {
    if slot.is_some() || arguments.starts_with(b"--") {
        return None;
    }
    let (path, rest) = split_literal(arguments)?;
    if path.is_empty() {
        return None;
    }
    *slot = Some(path);
    Some(rest)
}
