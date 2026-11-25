use clv_abi::{SYS_PIPE_CLOSE, SYS_PIPE_READ, SYS_PIPE_WRITE};

use super::{read_into, syscall, write_from};

/// Reads into a stack buffer. A nonempty buffer receives zero bytes at EOF.
pub fn read(handle: usize, buffer: &mut [u8]) -> Option<&[u8]> {
    read_into(SYS_PIPE_READ, handle, buffer)
}

/// Writes from a stack buffer. A successful write can accept a prefix.
pub fn write(handle: usize, bytes: &[u8]) -> Option<usize> {
    write_from(SYS_PIPE_WRITE, handle, bytes)
}

pub fn close(handle: usize) -> bool {
    syscall(SYS_PIPE_CLOSE, handle, 0, 0) == 0
}
