use clv_abi::{PipeStream, SYS_PIPE_CLOSE, SYS_PIPE_READ, SYS_PIPE_WRITE};

use super::{read_into, syscall, write_from};

/// Reads stdout as the parent or stdin as the child into a stack buffer.
/// A nonempty buffer receives zero bytes at EOF.
pub fn read(handle: usize, buffer: &mut [u8]) -> Option<&[u8]> {
    read_into(SYS_PIPE_READ, handle, buffer)
}

/// Writes stdin as the parent or stdout as the child from a stack buffer.
/// A successful write can accept a prefix.
pub fn write(handle: usize, bytes: &[u8]) -> Option<usize> {
    write_from(SYS_PIPE_WRITE, handle, bytes)
}

/// Closes the caller's stdout endpoint.
pub fn close(handle: usize) -> bool {
    syscall(SYS_PIPE_CLOSE, handle, PipeStream::Stdout as usize, 0) == 0
}

/// Closes the caller's stdin endpoint.
pub fn close_stdin(handle: usize) -> bool {
    syscall(SYS_PIPE_CLOSE, handle, PipeStream::Stdin as usize, 0) == 0
}
