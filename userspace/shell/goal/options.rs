use crate::split_word;
use clv_abi::goal::MAX_CHILDREN;

pub(crate) enum GoalCommand {
    Create,
    Run,
    AgentRun,
    AgentDelegate,
}

#[derive(Default)]
pub(crate) struct GoalOptions {
    pub children: Option<usize>,
    pub reply_bytes: Option<usize>,
    pub suspended: bool,
    pub with_file: bool,
}

impl GoalOptions {
    pub fn parse(mut arguments: &[u8], command: GoalCommand) -> Option<(Self, &[u8])> {
        let mut options = Self::default();
        loop {
            match split_word(arguments) {
                (b"--reply-bytes", rest) if options.reply_bytes.is_none() => {
                    let (bytes, rest) = count(rest)?;
                    options.reply_bytes = Some(bytes);
                    arguments = rest;
                }
                (b"--children", rest)
                    if matches!(command, GoalCommand::Create) && options.children.is_none() =>
                {
                    let (children, rest) = count(rest)?;
                    if !(1..=MAX_CHILDREN).contains(&children) {
                        return None;
                    }
                    options.children = Some(children);
                    arguments = rest;
                }
                (b"--suspended", rest)
                    if matches!(command, GoalCommand::AgentRun) && !options.suspended =>
                {
                    options.suspended = true;
                    arguments = rest;
                }
                (b"--with-file", rest)
                    if matches!(command, GoalCommand::AgentDelegate) && !options.with_file =>
                {
                    options.with_file = true;
                    arguments = rest;
                }
                (word, _) if word.starts_with(b"--") => return None,
                _ => return Some((options, arguments)),
            }
        }
    }
}

fn count(arguments: &[u8]) -> Option<(usize, &[u8])> {
    let (digits, rest) = split_word(arguments);
    if digits.is_empty() {
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
    Some((value, rest))
}
