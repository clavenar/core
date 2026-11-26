use crate::split_word;
use clv_abi::Program;
use clv_abi::goal::{MAX_CHILDREN, ReadLimits};

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
    Suspended,
    WithFile,
}

const FLAGS: &[(&[u8], Flag)] = &[
    (b"reply-bytes", Flag::ReplyBytes),
    (b"children", Flag::Children),
    (b"read-bytes", Flag::ReadBytes),
    (b"helper-read-bytes", Flag::HelperReadBytes),
    (b"allow-helper", Flag::AllowHelper),
    (b"suspended", Flag::Suspended),
    (b"with-file", Flag::WithFile),
];

#[derive(Default)]
pub(crate) struct GoalOptions {
    pub children: Option<usize>,
    pub reply_bytes: Option<usize>,
    pub read_limits: ReadLimits,
    pub allowed_helper: Option<Program>,
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
                Flag::AllowHelper
                    if matches!(command, GoalCommand::Create)
                        && options.allowed_helper.is_none() =>
                {
                    let (name, rest) = split_word(rest);
                    options.allowed_helper = Some(Program::named(name)?);
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
    if digits.is_empty() || slot.is_some() {
        return None;
    }
    let mut value = 0usize;
    for &digit in digits {
        if !digit.is_ascii_digit() {
            return None;
        }
        value = value
            .checked_mul(10)?
            .checked_add(usize::from(digit - b'0'))?;
    }
    *slot = Some(value);
    Some(rest)
}
