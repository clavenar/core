use crate::split_word;
use clv_abi::goal::MAX_CHILDREN;

pub(crate) enum GoalCommand {
    Create,
    Run,
    AgentRun,
}

#[derive(Default)]
pub(crate) struct GoalOptions {
    pub children: Option<usize>,
    pub reply_bytes: Option<usize>,
    pub suspended: bool,
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
                (word, _) if word.starts_with(b"--") => return None,
                _ => return Some((options, arguments)),
            }
        }
    }
}

fn count(arguments: &[u8]) -> Option<(usize, &[u8])> {
    let (value, rest) = split_word(arguments);
    if !value.first()?.is_ascii_digit() {
        return None;
    }
    Some((core::str::from_utf8(value).ok()?.parse().ok()?, rest))
}
