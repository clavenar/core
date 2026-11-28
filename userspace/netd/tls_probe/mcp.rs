//! Stateless MCP initialization and tool discovery over HTTPS.

use clv::mcp::{self, FRAME_BYTES, Frame};
use clv::net::MESSAGE_MAX_BYTES;
use core::fmt::{self, Write};

use super::{ExpectedResponse, Failure, HttpsConnection, RESPONSE_BYTES};

impl From<fmt::Error> for Failure {
    fn from(_: fmt::Error) -> Self {
        Self::InvalidResponse
    }
}

pub(super) fn initialize(
    https: &mut HttpsConnection<'_, '_>,
    response: &mut [u8; RESPONSE_BYTES],
) -> Result<(), Failure> {
    let mut buffer = [0; FRAME_BYTES];
    let mut request = Frame::new(&mut buffer);
    request.initialize_request("clavenar-netd")?;
    let result = https.post(
        request.into_bytes(),
        None,
        ExpectedResponse::McpJson,
        response,
    )?;
    if !mcp::result(result, "1").is_some_and(mcp::accepts_initialize) {
        return Err(Failure::InvalidResponse);
    }
    https.post(
        mcp::INITIALIZED_NOTIFICATION.as_bytes(),
        Some(mcp::VERSION),
        ExpectedResponse::McpAccepted,
        response,
    )?;
    Ok(())
}

pub(super) fn discover(
    https: &mut HttpsConnection<'_, '_>,
    response: &mut [u8; RESPONSE_BYTES],
) -> Result<(), Failure> {
    initialize(https, response)?;
    let body = https.post(
        mcp::LIST_TOOLS_REQUEST.as_bytes(),
        Some(mcp::VERSION),
        ExpectedResponse::McpJson,
        response,
    )?;
    mcp::result(body, "2")
        .and_then(mcp::validate_search_list)
        .ok_or(Failure::InvalidResponse)
}

pub(super) fn call_search(
    https: &mut HttpsConnection<'_, '_>,
    response: &mut [u8; RESPONSE_BYTES],
    output: &mut [u8; MESSAGE_MAX_BYTES],
) -> Result<usize, Failure> {
    discover(https, response)?;
    let mut buffer = [0; FRAME_BYTES];
    let mut request = Frame::new(&mut buffer);
    request.search_request("Hello")?;
    let body = https.post(
        request.into_bytes(),
        Some(mcp::VERSION),
        ExpectedResponse::McpJson,
        response,
    )?;
    let length = mcp::result(body, "3")
        .and_then(|result| mcp::decode_search_result(result, output))
        .ok_or(Failure::InvalidResponse)?;
    let text = &output[..length];
    // The shell writes the result to the console, so reject embedded controls.
    if !text
        .strip_suffix(b"\n")
        .unwrap_or(text)
        .iter()
        .all(|byte| (b' '..=b'~').contains(byte))
    {
        return Err(Failure::InvalidResponse);
    }
    Ok(length)
}

impl HttpsConnection<'_, '_> {
    fn post<'a>(
        &mut self,
        body: &[u8],
        protocol_version: Option<&str>,
        expected: ExpectedResponse,
        response: &'a mut [u8; RESPONSE_BYTES],
    ) -> Result<&'a [u8], Failure> {
        let mut buffer = [0; FRAME_BYTES];
        let mut header = Frame::new(&mut buffer);
        header.write_str(concat!(
            "POST /mcp HTTP/1.1\r\nHost: probe.clavenar.test:8443\r\n",
            "Content-Type: application/json\r\nAccept: application/json, text/event-stream\r\n",
            "Connection: close\r\n"
        ))?;
        if let Some(version) = protocol_version {
            write!(header, "MCP-Protocol-Version: {version}\r\n")?;
        }
        write!(header, "Content-Length: {}\r\n\r\n", body.len())?;
        self.request(header.into_bytes(), body, expected, response)
    }
}
