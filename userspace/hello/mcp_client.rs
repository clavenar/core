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
    let initialize = request(stream, frame, "1", |frame| {
        frame.initialize_request("hello")
    })?;
    if !mcp::accepts_initialize(initialize) {
        return None;
    }
    stream.send(frame, |frame| {
        frame.write_str(mcp::INITIALIZED_NOTIFICATION)
    })?;

    let list = request(stream, frame, "2", |frame| {
        frame.write_str(mcp::LIST_TOOLS_REQUEST)
    })?;
    let [kind, properties, required, additional] = mcp::search_tool_schema(list)?.fields([
        b"type",
        b"properties",
        b"required",
        b"additionalProperties",
    ])?;
    if !kind?.string_is(b"object")
        || !properties?
            .sole_field(b"pattern")?
            .field(b"type")?
            .string_is(b"string")
        || !required?.sole_item()?.string_is(b"pattern")
        || additional?.boolean()?
    {
        return None;
    }

    let result = request(stream, frame, "3", |frame| frame.search_request(pattern))?;
    mcp::decode_search_result(result, matches)
}

fn request<'a>(
    stream: &mut Stdio,
    buffer: &'a mut [u8; FRAME_BYTES],
    id: &str,
    message: impl FnOnce(&mut Frame<'_>) -> fmt::Result,
) -> Option<Value<'a>> {
    stream.send(buffer, message)?;
    mcp::result(stream.receive(buffer)??, id)
}
