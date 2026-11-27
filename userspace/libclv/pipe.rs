use clv_abi::{PipeStream, SYS_PIPE_CLOSE, SYS_PIPE_READ, SYS_PIPE_WRITE};

use super::{read_into, syscall, write_from};

/// Reads stdout as the parent or stdin as the child into a stack buffer.
/// A nonempty buffer receives zero bytes at EOF.
pub fn read(handle: usize, buffer: &mut [u8]) -> Option<&[u8]> {
    read_into(SYS_PIPE_READ, handle, buffer)
}

/// Reads through EOF into a stack buffer and fails if the stream exceeds it.
pub fn read_to_end(handle: usize, buffer: &mut [u8]) -> Option<&[u8]> {
    let mut length = 0;
    while length < buffer.len() {
        let count = read(handle, &mut buffer[length..])?.len();
        if count == 0 {
            return Some(&buffer[..length]);
        }
        length += count;
    }
    read(handle, &mut [0])?.is_empty().then_some(&*buffer)
}

/// Writes stdin as the parent or stdout as the child from a stack buffer.
/// A successful write can accept a prefix.
pub fn write(handle: usize, bytes: &[u8]) -> Option<usize> {
    write_from(SYS_PIPE_WRITE, handle, bytes)
}

/// Writes a stack buffer in successive pipe writes until it has all been queued.
pub fn write_all(handle: usize, mut bytes: &[u8]) -> Option<()> {
    while !bytes.is_empty() {
        let count = write(handle, bytes)?;
        if count == 0 {
            return None;
        }
        bytes = bytes.get(count..)?;
    }
    Some(())
}

/// Closes the caller's stdout endpoint.
pub fn close(handle: usize) -> bool {
    syscall(SYS_PIPE_CLOSE, handle, PipeStream::Stdout as usize, 0) == 0
}

/// Closes the caller's stdin endpoint.
pub fn close_stdin(handle: usize) -> bool {
    syscall(SYS_PIPE_CLOSE, handle, PipeStream::Stdin as usize, 0) == 0
}
