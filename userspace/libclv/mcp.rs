//! MCP JSON-RPC messages and local stdio framing.

mod json;

use clv_abi::PIPE_CAPACITY;
use core::fmt::{self, Write};

pub use json::Value;

use crate::pipe;

pub const VERSION: &str = "2025-11-25";
pub const SEARCH_TOOL: &str = "text.search";
pub const INITIALIZED_NOTIFICATION: &str =
    r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#;
pub const LIST_TOOLS_REQUEST: &str =
    r#"{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}"#;

// Includes the newline. A 64-byte result can occupy 384 escaped bytes.
pub const FRAME_BYTES: usize = 576;
const MAX_ID_BYTES: usize = 64;

pub struct Stdio {
    handle: usize,
    pending: [u8; PIPE_CAPACITY],
    start: usize,
    end: usize,
}

impl Stdio {
    pub fn new(handle: usize) -> Self {
        Self {
            handle,
            pending: [0; PIPE_CAPACITY],
            start: 0,
            end: 0,
        }
    }

    /// Returns clean EOF only between frames.
    pub fn receive<'a>(&mut self, frame: &'a mut [u8; FRAME_BYTES]) -> Option<Option<&'a [u8]>> {
        let frame = &mut frame[..FRAME_BYTES - 1];
        let mut length = 0;
        loop {
            if self.start == self.end {
                self.start = 0;
                self.end = pipe::read(self.handle, &mut self.pending)?.len();
                if self.end == 0 {
                    return (length == 0).then_some(None);
                }
            }
            let bytes = &self.pending[self.start..self.end];
            let newline = bytes.iter().position(|&byte| byte == b'\n');
            let count = newline.unwrap_or(bytes.len());
            let end = length + count;
            frame.get_mut(length..end)?.copy_from_slice(&bytes[..count]);
            length = end;
            self.start += count;
            if newline.is_some() {
                self.start += 1;
                return Some(Some(&frame[..length]));
            }
        }
    }

    pub fn send(
        &self,
        buffer: &mut [u8; FRAME_BYTES],
        message: impl FnOnce(&mut Frame<'_>) -> fmt::Result,
    ) -> Option<()> {
        let mut frame = Frame::new(buffer);
        message(&mut frame).ok()?;
        frame.write_char('\n').ok()?;
        pipe::write_all(self.handle, frame.into_bytes())
    }

    pub fn send_result(
        &self,
        buffer: &mut [u8; FRAME_BYTES],
        id: &str,
        result: impl FnOnce(&mut Frame<'_>) -> fmt::Result,
    ) -> Option<()> {
        self.send(buffer, |frame| {
            write!(frame, r#"{{"jsonrpc":"2.0","id":{id},"result":"#)?;
            result(frame)?;
            frame.write_char('}')
        })
    }

    pub fn send_error(
        &self,
        buffer: &mut [u8; FRAME_BYTES],
        id: Option<&str>,
        error: RpcError,
    ) -> Option<()> {
        let (code, message) = error.details();
        self.send(buffer, |frame| {
            frame.write_str(r#"{"jsonrpc":"2.0","#)?;
            if let Some(id) = id {
                write!(frame, r#""id":{id},"#)?;
            }
            write!(frame, r#""error":{{"code":{code},"message":"#)?;
            frame.string(message)?;
            frame.write_str("}}")
        })
    }
}

pub struct Frame<'a> {
    buffer: &'a mut [u8; FRAME_BYTES],
    length: usize,
}

impl<'a> Frame<'a> {
    pub fn new(buffer: &'a mut [u8; FRAME_BYTES]) -> Self {
        Self { buffer, length: 0 }
    }

    pub fn into_bytes(self) -> &'a [u8] {
        &self.buffer[..self.length]
    }

    pub fn initialize_request(&mut self, client_name: &str) -> fmt::Result {
        self.write_str(
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"#,
        )?;
        self.string(VERSION)?;
        self.write_str(r#","capabilities":{},"clientInfo":{"name":"#)?;
        self.string(client_name)?;
        self.write_str(r#","version":"1"}}}"#)
    }

    fn append(&mut self, bytes: &[u8]) -> fmt::Result {
        let end = self.length + bytes.len();
        self.buffer
            .get_mut(self.length..end)
            .ok_or(fmt::Error)?
            .copy_from_slice(bytes);
        self.length = end;
        Ok(())
    }

    pub fn string(&mut self, text: &str) -> fmt::Result {
        const HEX: &[u8; 16] = b"0123456789abcdef";

        self.write_char('"')?;
        let bytes = text.as_bytes();
        let mut start = 0;
        for (index, &byte) in bytes.iter().enumerate() {
            let escape: &[u8] = match byte {
                b'"' => br#"\""#,
                b'\\' => br"\\",
                b'\n' => br"\n",
                b'\r' => br"\r",
                b'\t' => br"\t",
                0..=31 => &[
                    b'\\',
                    b'u',
                    b'0',
                    b'0',
                    HEX[usize::from(byte >> 4)],
                    HEX[usize::from(byte & 15)],
                ],
                _ => continue,
            };
            self.append(&bytes[start..index])?;
            self.append(escape)?;
            start = index + 1;
        }
        self.append(&bytes[start..])?;
        self.write_char('"')
    }
}

impl Write for Frame<'_> {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        self.append(text.as_bytes())
    }
}

#[derive(Clone, Copy)]
pub enum RpcError {
    Parse,
    InvalidRequest,
    MethodNotFound,
    InvalidParams,
    NotInitialized,
}

impl RpcError {
    fn details(self) -> (i32, &'static str) {
        match self {
            Self::Parse => (-32700, "parse error"),
            Self::InvalidRequest => (-32600, "invalid request"),
            Self::MethodNotFound => (-32601, "method not found"),
            Self::InvalidParams => (-32602, "invalid parameters"),
            Self::NotInitialized => (-32002, "session not initialized"),
        }
    }
}

pub struct Request<'a> {
    pub id: Option<&'a str>,
    pub method: Value<'a>,
    pub params: Option<Value<'a>>,
}

impl<'a> Request<'a> {
    pub fn parse(frame: &'a [u8]) -> Result<Self, (Option<&'a str>, RpcError)> {
        let value = json::parse(frame).ok_or((None, RpcError::Parse))?;
        let [version, id, method, params, result, error] = value
            .fields([b"jsonrpc", b"id", b"method", b"params", b"result", b"error"])
            .ok_or_else(|| {
                (
                    value.field(b"id").and_then(request_id),
                    RpcError::InvalidRequest,
                )
            })?;
        let id = match id {
            None => None,
            Some(id) => Some(request_id(id).ok_or((None, RpcError::InvalidRequest))?),
        };
        let invalid = (id, RpcError::InvalidRequest);
        let method = method.filter(|method| method.is_string()).ok_or(invalid)?;
        if !version.is_some_and(|value| value.string_is(b"2.0"))
            || params.is_some_and(|value| !value.is_object())
            || result.is_some()
            || error.is_some()
        {
            return Err(invalid);
        }
        Ok(Self { id, method, params })
    }
}

fn request_id(value: Value<'_>) -> Option<&str> {
    let encoded = value.encoded()?;
    (encoded.len() <= MAX_ID_BYTES && (value.is_string() || value.integer())).then_some(encoded)
}

/// Extracts a successful response to a specific JSON-RPC request.
pub fn result<'a>(frame: &'a [u8], expected_id: &str) -> Option<Value<'a>> {
    let [version, id, result, error, method] =
        json::parse(frame)?.fields([b"jsonrpc", b"id", b"result", b"error", b"method"])?;
    if !version?.string_is(b"2.0")
        || request_id(id?)? != expected_id
        || error.is_some()
        || method.is_some()
    {
        return None;
    }
    result.filter(|result| result.is_object())
}

pub fn is_implementation(value: Value<'_>) -> bool {
    let Some([Some(name), Some(version)]) = value.fields([b"name", b"version"]) else {
        return false;
    };
    name.is_string() && version.is_string()
}

pub fn accepts_initialize(result: Value<'_>) -> bool {
    let Some([Some(version), Some(capabilities), Some(server)]) =
        result.fields([b"protocolVersion", b"capabilities", b"serverInfo"])
    else {
        return false;
    };
    version.string_is(VERSION.as_bytes())
        && capabilities
            .field(b"tools")
            .is_some_and(|tools| tools.is_object())
        && is_implementation(server)
}

/// Finds the search tool's schema in a complete, single-tool list.
pub fn search_tool_schema(list: Value<'_>) -> Option<Value<'_>> {
    let [tools, None] = list.fields([b"tools", b"nextCursor"])? else {
        return None;
    };
    let [name, schema] = tools?.sole_item()?.fields([b"name", b"inputSchema"])?;
    name?.string_is(SEARCH_TOOL.as_bytes()).then_some(schema?)
}

/// Checks the input contract of the pinned HTTPS fixture.
pub fn validate_search_list(list: Value<'_>) -> Option<()> {
    let [kind, properties, required, additional, _] = search_tool_schema(list)?.fields_only([
        b"type",
        b"properties",
        b"required",
        b"additionalProperties",
        b"title",
    ])?;
    let [pattern, _] = properties?
        .sole_field(b"pattern")?
        .fields_only([b"type", b"title"])?;
    (kind?.string_is(b"object")
        && pattern?.string_is(b"string")
        && required?.sole_item()?.string_is(b"pattern")
        && additional.is_none_or(|value| value.boolean() == Some(false)))
    .then_some(())
}
