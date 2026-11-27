use clv::mcp::{self, FRAME_BYTES, Request, RpcError, Stdio, Value};
use clv_abi::{MESSAGE_MAX_BYTES, ReplyKind};
use core::fmt::Write;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    AwaitInitialize,
    AwaitInitialized,
    Ready,
}

struct Session<'a> {
    stream: Stdio,
    file: usize,
    approved: &'a [u8],
    phase: Phase,
    searched: bool,
}

pub(super) fn serve(io: usize, file: usize, approved: &[u8]) -> Option<()> {
    let mut session = Session {
        stream: Stdio::new(io),
        file,
        approved,
        phase: Phase::AwaitInitialize,
        searched: false,
    };
    let mut input = [0; FRAME_BYTES];
    let mut output = [0; FRAME_BYTES];
    while let Some(line) = session.stream.receive(&mut input)? {
        match Request::parse(line) {
            Ok(request) => session.handle(request, &mut output)?,
            Err((id, error)) => session.stream.send_error(&mut output, id, error)?,
        }
    }
    (session.phase == Phase::Ready).then_some(())
}

impl Session<'_> {
    fn handle(&mut self, request: Request<'_>, output: &mut [u8; FRAME_BYTES]) -> Option<()> {
        let Some(id) = request.id else {
            if self.phase == Phase::AwaitInitialized
                && request.method.string_is(b"notifications/initialized")
            {
                self.phase = Phase::Ready;
            }
            return Some(());
        };
        let method = request.method;
        if method.string_is(b"initialize") {
            self.initialize(id, request.params, output)
        } else if method.string_is(b"ping") {
            self.stream
                .send_result(output, id, |frame| frame.write_str("{}"))
        } else if self.phase != Phase::Ready {
            self.stream
                .send_error(output, Some(id), RpcError::NotInitialized)
        } else if method.string_is(b"tools/list") {
            self.list(id, request.params, output)
        } else if method.string_is(b"tools/call") {
            self.call(id, request.params, output)
        } else {
            self.stream
                .send_error(output, Some(id), RpcError::MethodNotFound)
        }
    }

    fn initialize(
        &mut self,
        id: &str,
        params: Option<Value<'_>>,
        output: &mut [u8; FRAME_BYTES],
    ) -> Option<()> {
        if self.phase != Phase::AwaitInitialize {
            return self
                .stream
                .send_error(output, Some(id), RpcError::InvalidRequest);
        }
        let valid = params
            .and_then(|params| params.fields([b"protocolVersion", b"capabilities", b"clientInfo"]))
            .is_some_and(|[version, capabilities, client]| {
                version.is_some_and(Value::is_string)
                    && capabilities.is_some_and(Value::is_object)
                    && client.is_some_and(mcp::is_implementation)
            });
        if !valid {
            return self
                .stream
                .send_error(output, Some(id), RpcError::InvalidParams);
        }
        self.stream.send_result(output, id, |frame| {
            frame.write_str(r#"{"protocolVersion":"#)?;
            frame.string(mcp::VERSION)?;
            frame.write_str(
                r#","capabilities":{"tools":{}},"serverInfo":{"name":"grep","version":"1"}}"#,
            )
        })?;
        self.phase = Phase::AwaitInitialized;
        Some(())
    }

    fn list(
        &self,
        id: &str,
        params: Option<Value<'_>>,
        output: &mut [u8; FRAME_BYTES],
    ) -> Option<()> {
        if params.is_some_and(|params| !matches!(params.fields([b"cursor"]), Some([None]))) {
            return self
                .stream
                .send_error(output, Some(id), RpcError::InvalidParams);
        }
        self.stream.send_result(output, id, |frame| {
            frame.write_str(r#"{"tools":[{"name":"#)?;
            frame.string(mcp::SEARCH_TOOL)?;
            frame.write_str(concat!(
                r#","inputSchema":{"type":"object","properties":{"pattern":{"type":"string"}},"#,
                r#""required":["pattern"],"additionalProperties":false}}]}"#,
            ))
        })
    }

    fn call(
        &mut self,
        id: &str,
        params: Option<Value<'_>>,
        output: &mut [u8; FRAME_BYTES],
    ) -> Option<()> {
        let pattern = params
            .and_then(|params| params.fields([b"name", b"arguments"]))
            .and_then(|[name, arguments]| {
                name?.string_is(mcp::SEARCH_TOOL.as_bytes()).then_some(())?;
                arguments?
                    .sole_field(b"pattern")
                    .filter(|pattern| pattern.is_string())
            });
        let Some(pattern) = pattern else {
            return self
                .stream
                .send_error(output, Some(id), RpcError::InvalidParams);
        };
        if !pattern.string_is(self.approved) {
            return self.tool_result(
                output,
                id,
                "pattern differs from the approved literal",
                true,
            );
        }
        if self.searched {
            return self.tool_result(
                output,
                id,
                "the granted file has already been searched",
                true,
            );
        }
        // A launch holds one file cursor. Compare decoded bytes before touching it.
        self.searched = true;
        let mut matches = [0; MESSAGE_MAX_BYTES];
        match super::search(self.file, self.approved, &mut matches) {
            Ok(length) => match core::str::from_utf8(&matches[..length]) {
                Ok(text) => self.tool_result(output, id, text, false),
                Err(_) => self.tool_result(output, id, "match is not UTF-8", true),
            },
            Err(ReplyKind::ResultTooLarge) => {
                self.tool_result(output, id, "grep result exceeds 64 bytes", true)
            }
            Err(_) => self.tool_result(output, id, "cannot read the granted file", true),
        }
    }

    fn tool_result(
        &self,
        output: &mut [u8; FRAME_BYTES],
        id: &str,
        text: &str,
        is_error: bool,
    ) -> Option<()> {
        self.stream.send_result(output, id, |frame| {
            frame.write_str(r#"{"content":[{"type":"text","text":"#)?;
            frame.string(text)?;
            write!(frame, r#"}}],"isError":{is_error}}}"#)
        })
    }
}
