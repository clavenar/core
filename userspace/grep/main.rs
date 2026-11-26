#![no_std]
#![no_main]

use clv::{LaunchRequest, fs};
use clv_abi::tool::TEXT_SEARCH_V1;
use clv_abi::{GrantKind, MESSAGE_MAX_BYTES, ReplyKind, RequestKind};
use core::panic::PanicInfo;

// SAFETY: The kernel enters this unique symbol with an aligned user stack.
#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
extern "C" fn _start(_task: usize, file: usize, grant: usize) -> ! {
    let mut argument = [0; MESSAGE_MAX_BYTES];
    let mut output = [0; MESSAGE_MAX_BYTES];
    let result = match clv::receive_request(&mut argument) {
        Some(LaunchRequest::Received {
            kind: RequestKind::Argument,
            bytes,
            ..
        }) if grant == GrantKind::File as usize => search(file, bytes, &mut output),
        _ => Err(ReplyKind::ReadFailed),
    };
    let status = match result {
        Ok(length) if clv::send_reply(TEXT_SEARCH_V1.success_reply, &output[..length]) => {
            usize::from(length == 0)
        }
        Ok(_) => 2,
        Err(kind) => {
            clv::send_reply(kind, &[]);
            2
        }
    };
    clv::exit(status)
}

fn contains(line: &[u8], pattern: &[u8]) -> bool {
    pattern.is_empty() || line.windows(pattern.len()).any(|part| part == pattern)
}

fn search(file: usize, pattern: &[u8], output: &mut [u8]) -> Result<usize, ReplyKind> {
    let mut input = [0; 2 * MESSAGE_MAX_BYTES];
    let mut kept = 0;
    let mut oversized_line = false;
    let mut written = 0;
    loop {
        let count = fs::read(file, &mut input[kept..])
            .ok_or(ReplyKind::ReadFailed)?
            .len();
        let end = kept + count;
        let mut consumed = 0;
        for line in input[..end].split_inclusive(|&byte| byte == b'\n') {
            let text = match line.strip_suffix(b"\n") {
                Some(text) => text,
                None if count == 0 => line,
                None => break,
            };
            if contains(text, pattern) {
                if oversized_line {
                    return Err(ReplyKind::ResultTooLarge);
                }
                let next = written + line.len();
                output
                    .get_mut(written..next)
                    .ok_or(ReplyKind::ResultTooLarge)?
                    .copy_from_slice(line);
                written = next;
            }
            consumed += line.len();
            oversized_line = false;
        }
        if count == 0 {
            return Ok(written);
        }
        if consumed == 0 && end == input.len() {
            // A matching line this long cannot fit. Keep enough overlap to find
            // a later match that straddles the next read, even on very long lines.
            if contains(&input, pattern) {
                return Err(ReplyKind::ResultTooLarge);
            }
            consumed = end - pattern.len().saturating_sub(1);
            oversized_line = true;
        }
        input.copy_within(consumed..end, 0);
        kept = end - consumed;
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo<'_>) -> ! {
    clv::exit(2)
}
