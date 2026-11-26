use crate::{number, split_word};
use clv_abi::Program;
use clv_abi::goal::{MAX_CHILDREN, ReadLimits};
use clv_abi::tool::{FileTool, ToolId};

pub(crate) enum GoalCommand {
    Create,
    Run,
    AgentRun,
    AgentDelegate,
}

#[derive(Clone, Copy)]
enum Flag {
    ReplyBytes,
    Children,
    ReadBytes,
    HelperReadBytes,
    AllowHelper,
    AllowTool,
    Suspended,
    WithFile,
}

const FLAGS: &[(&[u8], Flag)] = &[
    (b"reply-bytes", Flag::ReplyBytes),
    (b"children", Flag::Children),
    (b"read-bytes", Flag::ReadBytes),
    (b"helper-read-bytes", Flag::HelperReadBytes),
    (b"allow-helper", Flag::AllowHelper),
    (b"allow-tool", Flag::AllowTool),
    (b"suspended", Flag::Suspended),
    (b"with-file", Flag::WithFile),
];

#[derive(Default)]
pub(crate) struct GoalOptions {
    pub children: Option<usize>,
    pub reply_bytes: Option<usize>,
    pub read_limits: ReadLimits,
    pub allowed_helper: Option<Program>,
    pub allowed_tool: Option<ToolId>,
    pub suspended: bool,
    pub with_file: bool,
}

impl GoalOptions {
    pub fn parse(mut arguments: &[u8], command: GoalCommand) -> Option<(Self, &[u8])> {
        let mut options = Self::default();
        loop {
            let (word, rest) = split_word(arguments);
            let Some(name) = word.strip_prefix(b"--") else {
                break;
            };
            let &(_, flag) = FLAGS.iter().find(|(spelling, _)| *spelling == name)?;
            arguments = match flag {
                Flag::ReplyBytes => parse_count(rest, &mut options.reply_bytes)?,
                Flag::Children if matches!(command, GoalCommand::Create) => {
                    parse_count(rest, &mut options.children)?
                }
                Flag::ReadBytes if matches!(command, GoalCommand::Create) => {
                    parse_count(rest, &mut options.read_limits.goal)?
                }
                Flag::HelperReadBytes if matches!(command, GoalCommand::Create) => {
                    parse_count(rest, &mut options.read_limits.helpers)?
                }
                Flag::AllowHelper | Flag::AllowTool
                    if matches!(command, GoalCommand::Create)
                        && options.allowed_helper.is_none()
                        && options.allowed_tool.is_none() =>
                {
                    let (name, rest) = split_word(rest);
                    if matches!(flag, Flag::AllowTool) {
                        options.allowed_tool = Some(FileTool::named(name)?.id);
                    } else {
                        options.allowed_helper = Some(Program::named(name)?);
                    }
                    rest
                }
                Flag::Suspended
                    if matches!(command, GoalCommand::AgentRun) && !options.suspended =>
                {
                    options.suspended = true;
                    rest
                }
                Flag::WithFile
                    if matches!(command, GoalCommand::AgentDelegate) && !options.with_file =>
                {
                    options.with_file = true;
                    rest
                }
                _ => return None,
            };
        }
        if options
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
