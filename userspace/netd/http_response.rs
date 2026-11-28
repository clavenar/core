use core::ops::Range;

const MAX_HEAD_BYTES: usize = 1024;
const MAX_BODY_BYTES: usize = 1024;
pub(super) const RESPONSE_BYTES: usize = MAX_HEAD_BYTES + MAX_BODY_BYTES;

pub(super) struct InvalidResponse;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ExpectedResponse {
    Ok,
    #[cfg(feature = "tls-probe")]
    McpJson,
    #[cfg(feature = "tls-probe")]
    McpAccepted,
}

impl ExpectedResponse {
    fn status(self) -> u16 {
        match self {
            #[cfg(feature = "tls-probe")]
            Self::McpAccepted => 202,
            _ => 200,
        }
    }
}

/// Returns the declared body range once the complete header is available.
pub(super) fn body_range(
    bytes: &[u8],
    expected: ExpectedResponse,
) -> Result<Option<Range<usize>>, InvalidResponse> {
    let mut headers = [httparse::EMPTY_HEADER; 32];
    let mut response = httparse::Response::new(&mut headers);
    let head = &bytes[..bytes.len().min(MAX_HEAD_BYTES)];
    let end = match response.parse(head) {
        Ok(httparse::Status::Complete(end)) => end,
        Ok(httparse::Status::Partial) if head.len() < MAX_HEAD_BYTES => return Ok(None),
        _ => return Err(InvalidResponse),
    };
    if response.version != Some(1) || response.code != Some(expected.status()) {
        return Err(InvalidResponse);
    }

    let mut length = None;
    #[cfg(feature = "tls-probe")]
    let mut json = false;
    for header in response.headers {
        if header.name.eq_ignore_ascii_case("Transfer-Encoding") {
            return Err(InvalidResponse);
        }
        #[cfg(feature = "tls-probe")]
        if expected != ExpectedResponse::Ok && header.name.eq_ignore_ascii_case("MCP-Session-Id") {
            return Err(InvalidResponse);
        }
        #[cfg(feature = "tls-probe")]
        if expected == ExpectedResponse::McpJson && header.name.eq_ignore_ascii_case("Content-Type")
        {
            if json
                || !header
                    .value
                    .split(|&byte| byte == b';')
                    .next()
                    .is_some_and(|kind| kind.trim_ascii().eq_ignore_ascii_case(b"application/json"))
            {
                return Err(InvalidResponse);
            }
            json = true;
        }
        if header.name.eq_ignore_ascii_case("Content-Length") {
            let value = header.value.trim_ascii();
            if length.is_some() || value.is_empty() || !value.iter().all(u8::is_ascii_digit) {
                return Err(InvalidResponse);
            }
            length = core::str::from_utf8(value)
                .ok()
                .and_then(|value| value.parse::<usize>().ok())
                .filter(|length| *length <= MAX_BODY_BYTES);
            if length.is_none() {
                return Err(InvalidResponse);
            }
        }
    }
    let length = length.ok_or(InvalidResponse)?;
    #[cfg(feature = "tls-probe")]
    if (expected == ExpectedResponse::McpJson && !json)
        || (expected == ExpectedResponse::McpAccepted && length != 0)
    {
        return Err(InvalidResponse);
    }
    Ok(Some(end..end + length))
}
