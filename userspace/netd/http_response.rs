use core::ops::Range;

const MAX_HEAD_BYTES: usize = 1024;
const MAX_BODY_BYTES: usize = 1024;
pub(super) const RESPONSE_BYTES: usize = MAX_HEAD_BYTES + MAX_BODY_BYTES;

pub(super) struct InvalidResponse;

/// Returns the declared body range once the complete header is available.
pub(super) fn body_range(bytes: &[u8]) -> Result<Option<Range<usize>>, InvalidResponse> {
    let mut headers = [httparse::EMPTY_HEADER; 32];
    let mut response = httparse::Response::new(&mut headers);
    let head = &bytes[..bytes.len().min(MAX_HEAD_BYTES)];
    let end = match response.parse(head) {
        Ok(httparse::Status::Complete(end)) => end,
        Ok(httparse::Status::Partial) if head.len() < MAX_HEAD_BYTES => return Ok(None),
        _ => return Err(InvalidResponse),
    };
    if response.version != Some(1) || response.code != Some(200) {
        return Err(InvalidResponse);
    }

    let mut length = None;
    for header in response.headers {
        if header.name.eq_ignore_ascii_case("Transfer-Encoding") {
            return Err(InvalidResponse);
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
    Ok(Some(end..end + length))
}
