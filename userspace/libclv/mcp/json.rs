//! Borrowed JSON values for bounded frames in boot userspace.

const MAX_DEPTH: usize = 8;

#[derive(Clone, Copy)]
pub struct Value<'a>(&'a [u8]);

pub(super) fn parse(frame: &[u8]) -> Option<Value<'_>> {
    core::str::from_utf8(frame).ok()?;
    let mut input = Cursor { rest: frame };
    let value = input.value(0)?;
    input.whitespace();
    input.rest.is_empty().then_some(value)
}

impl<'a> Value<'a> {
    pub fn fields<const N: usize>(self, keys: [&[u8]; N]) -> Option<[Option<Self>; N]> {
        let mut fields = [None; N];
        let mut input = Cursor { rest: self.0 };
        if !input.take(b'{') {
            return None;
        }
        input.whitespace();
        while !input.take(b'}') {
            let (key, value) = input.member(0)?;
            if let Some(index) = keys.iter().position(|name| key.string_is(name)) {
                // A repeated member must never change the meaning of a selected field.
                if fields[index].replace(value).is_some() {
                    return None;
                }
            }
            input.whitespace();
            if !input.take(b',') {
                break;
            }
            input.whitespace();
        }
        Some(fields)
    }

    pub fn field(self, key: &[u8]) -> Option<Self> {
        let [value] = self.fields([key])?;
        value
    }

    pub fn sole_field(self, key: &[u8]) -> Option<Self> {
        let mut input = Cursor { rest: self.0 };
        if !input.take(b'{') {
            return None;
        }
        let (name, value) = input.member(0)?;
        input.whitespace();
        (input.take(b'}') && name.string_is(key)).then_some(value)
    }

    pub fn sole_item(self) -> Option<Self> {
        let mut input = Cursor { rest: self.0 };
        if !input.take(b'[') {
            return None;
        }
        let item = input.value(0)?;
        input.whitespace();
        input.take(b']').then_some(item)
    }

    pub fn is_object(self) -> bool {
        self.0.starts_with(b"{")
    }

    pub fn is_string(self) -> bool {
        self.0.starts_with(b"\"")
    }

    pub fn boolean(self) -> Option<bool> {
        match self.0 {
            b"true" => Some(true),
            b"false" => Some(false),
            _ => None,
        }
    }

    pub(super) fn integer(self) -> bool {
        matches!(self.0.first(), Some(b'-' | b'0'..=b'9'))
            && self
                .0
                .iter()
                .all(|byte| !matches!(byte, b'.' | b'e' | b'E'))
    }

    pub(super) fn encoded(self) -> Option<&'a str> {
        core::str::from_utf8(self.0).ok()
    }

    pub fn string_is(self, mut expected: &[u8]) -> bool {
        let mut input = Cursor { rest: self.0 };
        input
            .string(|chunk| {
                expected = expected.strip_prefix(chunk)?;
                Some(())
            })
            .is_some()
            && expected.is_empty()
    }

    pub fn decode_string(self, buffer: &mut [u8]) -> Option<usize> {
        let mut length = 0;
        let mut input = Cursor { rest: self.0 };
        input.string(|chunk| {
            let end = length + chunk.len();
            buffer.get_mut(length..end)?.copy_from_slice(chunk);
            length = end;
            Some(())
        })?;
        Some(length)
    }
}

struct Cursor<'a> {
    rest: &'a [u8],
}

impl<'a> Cursor<'a> {
    fn whitespace(&mut self) {
        while matches!(self.rest.first(), Some(b' ' | b'\t' | b'\r' | b'\n')) {
            self.rest = &self.rest[1..];
        }
    }

    fn take(&mut self, byte: u8) -> bool {
        if let [first, rest @ ..] = self.rest
            && *first == byte
        {
            self.rest = rest;
            return true;
        }
        false
    }

    fn next(&mut self) -> Option<u8> {
        let (byte, rest) = self.rest.split_first()?;
        self.rest = rest;
        Some(*byte)
    }

    fn value(&mut self, depth: usize) -> Option<Value<'a>> {
        if depth > MAX_DEPTH {
            return None;
        }
        self.whitespace();
        let start = self.rest;
        match *start.first()? {
            b'{' => self.container(b'}', depth)?,
            b'[' => self.container(b']', depth)?,
            b'"' => self.string(|_| Some(()))?,
            b'-' | b'0'..=b'9' => self.number()?,
            b't' => self.rest = start.strip_prefix(b"true")?,
            b'f' => self.rest = start.strip_prefix(b"false")?,
            b'n' => self.rest = start.strip_prefix(b"null")?,
            _ => return None,
        }
        Some(Value(&start[..start.len() - self.rest.len()]))
    }

    fn container(&mut self, close: u8, depth: usize) -> Option<()> {
        self.next()?;
        self.whitespace();
        if !self.take(close) {
            loop {
                if close == b'}' {
                    self.member(depth + 1)?;
                } else {
                    self.value(depth + 1)?;
                }
                self.whitespace();
                if self.take(close) {
                    break;
                }
                self.take(b',').then_some(())?;
            }
        }
        Some(())
    }

    fn member(&mut self, depth: usize) -> Option<(Value<'a>, Value<'a>)> {
        self.whitespace();
        let start = self.rest;
        self.string(|_| Some(()))?;
        let key = Value(&start[..start.len() - self.rest.len()]);
        self.whitespace();
        self.take(b':').then_some(())?;
        Some((key, self.value(depth)?))
    }

    fn number(&mut self) -> Option<()> {
        self.take(b'-');
        if !self.take(b'0') {
            self.digits()?;
        }
        if self.take(b'.') {
            self.digits()?;
        }
        if self.take(b'e') || self.take(b'E') {
            if !self.take(b'-') {
                self.take(b'+');
            }
            self.digits()?;
        }
        Some(())
    }

    fn digits(&mut self) -> Option<()> {
        let start = self.rest.len();
        while matches!(self.rest.first(), Some(b'0'..=b'9')) {
            self.rest = &self.rest[1..];
        }
        (self.rest.len() < start).then_some(())
    }

    fn string(&mut self, mut chunk: impl FnMut(&[u8]) -> Option<()>) -> Option<()> {
        self.take(b'"').then_some(())?;
        loop {
            let end = self
                .rest
                .iter()
                .position(|byte| matches!(byte, b'"' | b'\\' | 0..=31))?;
            let (raw, rest) = self.rest.split_at(end);
            chunk(raw)?;
            self.rest = rest;
            match self.next()? {
                b'"' => return Some(()),
                b'\\' => chunk(self.escape()?.encode_utf8(&mut [0; 4]).as_bytes())?,
                _ => return None,
            }
        }
    }

    fn escape(&mut self) -> Option<char> {
        Some(match self.next()? {
            byte @ (b'"' | b'\\' | b'/') => char::from(byte),
            b'b' => '\u{8}',
            b'f' => '\u{c}',
            b'n' => '\n',
            b'r' => '\r',
            b't' => '\t',
            b'u' => {
                let high = self.hex_code_unit()?;
                let scalar = if (0xd800..=0xdbff).contains(&high) {
                    self.rest = self.rest.strip_prefix(b"\\u")?;
                    let low = self.hex_code_unit()?;
                    if !(0xdc00..=0xdfff).contains(&low) {
                        return None;
                    }
                    0x10000 + ((high - 0xd800) << 10) + (low - 0xdc00)
                } else {
                    high
                };
                char::from_u32(scalar)?
            }
            _ => return None,
        })
    }

    fn hex_code_unit(&mut self) -> Option<u32> {
        let (digits, rest) = self.rest.split_first_chunk::<4>()?;
        let mut value = 0;
        for byte in digits {
            value = (value << 4) | char::from(*byte).to_digit(16)?;
        }
        self.rest = rest;
        Some(value)
    }
}
