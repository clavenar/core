use clv::mcp::{self, FRAME_BYTES, Frame, Stdio, Value};
use clv::{ChildStatus, agent, pipe};
use clv_abi::{MESSAGE_MAX_BYTES, ReplyKind};
use core::fmt::{self, Write};

pub(super) fn search(
    invocation: &agent::FileToolInvocation<'_>,
    matches: &mut [u8; MESSAGE_MAX_BYTES],
) -> Result<usize, ReplyKind> {
    let Some(helper) = agent::invoke_file_tool_mcp(agent::CURRENT_AGENT, invocation) else {
        return Err(ReplyKind::DelegationFailed);
    };
    let mut stream = Stdio::new(helper);
    let mut frame = [0; FRAME_BYTES];
    let length = exchange(&mut stream, &mut frame, invocation.argument, matches);

    // Close both endpoints on failure so neither side can wait for more traffic.
    let stdin_closed = pipe::close_stdin(helper);
    let clean_eof =
        stdin_closed && length.is_some() && matches!(stream.receive(&mut frame), Some(None));
    pipe::close(helper);
    let status = clv::wait(helper);
    if let (Some(length), true, Some(ChildStatus::Exited(0))) = (length, clean_eof, status) {
        Ok(length)
    } else {
        Err(ReplyKind::ReadFailed)
    }
}

fn exchange(
    stream: &mut Stdio,
    frame: &mut [u8; FRAME_BYTES],
    pattern: &[u8],
    matches: &mut [u8; MESSAGE_MAX_BYTES],
) -> Option<usize> {
    let pattern = core::str::from_utf8(pattern).ok()?;
    let initialize = request(stream, frame, "1", "initialize", |frame| {
        frame.write_str(r#"{"protocolVersion":"#)?;
        frame.string(mcp::VERSION)?;
        frame.write_str(r#","capabilities":{},"clientInfo":{"name":"hello","version":"1"}}"#)
    })?;
    if !mcp::accepts_initialize(initialize) {
        return None;
    }
    stream.send(frame, |frame| {
        frame.write_str(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#)
    })?;

    let list = request(stream, frame, "2", "tools/list", |frame| {
        frame.write_str("{}")
    })?;
    let [tools, None] = list.fields([b"tools", b"nextCursor"])? else {
        return None;
    };
    let [name, schema] = tools?.sole_item()?.fields([b"name", b"inputSchema"])?;
    let [kind, properties, required, additional] =
        schema?.fields([b"type", b"properties", b"required", b"additionalProperties"])?;
    if !name?.string_is(mcp::SEARCH_TOOL.as_bytes())
        || !kind?.string_is(b"object")
        || !properties?
            .sole_field(b"pattern")?
            .field(b"type")?
            .string_is(b"string")
        || !required?.sole_item()?.string_is(b"pattern")
        || additional?.boolean()?
    {
        return None;
    }

    let result = request(stream, frame, "3", "tools/call", |frame| {
        frame.write_str(r#"{"name":"#)?;
        frame.string(mcp::SEARCH_TOOL)?;
        frame.write_str(r#","arguments":{"pattern":"#)?;
        frame.string(pattern)?;
        frame.write_str("}}")
    })?;
    let [content, is_error] = result.fields([b"content", b"isError"])?;
    if let Some(is_error) = is_error
        && is_error.boolean()?
    {
        return None;
    }
    let [kind, text] = content?.sole_item()?.fields([b"type", b"text"])?;
    if !kind?.string_is(b"text") {
        return None;
    }
    text?.decode_string(matches)
}

fn request<'a>(
    stream: &mut Stdio,
    buffer: &'a mut [u8; FRAME_BYTES],
    id: &str,
    method: &str,
    params: impl FnOnce(&mut Frame<'_>) -> fmt::Result,
) -> Option<Value<'a>> {
    stream.send(buffer, |frame| {
        write!(
            frame,
            r#"{{"jsonrpc":"2.0","id":{id},"method":"{method}","params":"#
        )?;
        params(frame)?;
        frame.write_char('}')
    })?;
    mcp::result(stream.receive(buffer)??, id)
}
