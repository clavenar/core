use crate::split_word;

#[derive(Default)]
pub(crate) struct GoalOptions {
    pub reply_bytes: Option<usize>,
    pub suspended: bool,
}

impl GoalOptions {
    pub fn parse(mut arguments: &[u8], allow_suspended: bool) -> Option<(Self, &[u8])> {
        let mut options = Self::default();
        loop {
            match split_word(arguments) {
                (b"--reply-bytes", rest) if options.reply_bytes.is_none() => {
                    let (value, rest) = split_word(rest);
                    if !value.first()?.is_ascii_digit() {
                        return None;
                    }
                    options.reply_bytes = Some(core::str::from_utf8(value).ok()?.parse().ok()?);
                    arguments = rest;
                }
                (b"--suspended", rest) if allow_suspended && !options.suspended => {
                    options.suspended = true;
                    arguments = rest;
                }
                (word, _) if word.starts_with(b"--") => return None,
                _ => return Some((options, arguments)),
            }
        }
    }
}
