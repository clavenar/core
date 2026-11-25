use clv_abi::{
    PATH_MAX_BYTES, SYS_CHDIR, SYS_CLOSE, SYS_GETCWD, SYS_MKDIR, SYS_OPEN, SYS_OPEN_AT, SYS_READ,
    SYS_READDIR, SYS_RMDIR, SYS_UNLINK, SYS_WRITE,
};

use super::{syscall, syscall_result, with_stack_bytes};

fn path_call(number: usize, path: &[u8], mode: usize) -> Option<usize> {
    with_stack_bytes::<PATH_MAX_BYTES>(path, |address, length| {
        syscall(number, address, length, mode)
    })
}

pub fn open(path: &[u8], mode: usize) -> Option<usize> {
    path_call(SYS_OPEN, path, mode)
}

pub fn open_at(directory: usize, path: &[u8]) -> Option<usize> {
    with_stack_bytes::<PATH_MAX_BYTES>(path, |address, length| {
        syscall(SYS_OPEN_AT, directory, address, length)
    })
}

pub fn close(handle: usize) -> bool {
    syscall(SYS_CLOSE, handle, 0, 0) == 0
}

pub fn chdir(path: &[u8]) -> bool {
    path_call(SYS_CHDIR, path, 0).is_some()
}

pub fn mkdir(path: &[u8]) -> bool {
    path_call(SYS_MKDIR, path, 0).is_some()
}

pub fn rmdir(path: &[u8]) -> bool {
    path_call(SYS_RMDIR, path, 0).is_some()
}

pub fn unlink(path: &[u8]) -> bool {
    path_call(SYS_UNLINK, path, 0).is_some()
}

pub fn getcwd(buffer: &mut [u8]) -> Option<&[u8]> {
    let count = syscall(
        SYS_GETCWD,
        buffer.as_mut_ptr().expose_provenance(),
        buffer.len(),
        0,
    );
    buffer.get(..count)
}

fn read_into(number: usize, handle: usize, buffer: &mut [u8]) -> Option<&[u8]> {
    let count = syscall(
        number,
        handle,
        buffer.as_mut_ptr().expose_provenance(),
        buffer.len(),
    );
    buffer.get(..count)
}

pub fn read(handle: usize, buffer: &mut [u8]) -> Option<&[u8]> {
    read_into(SYS_READ, handle, buffer)
}

pub fn readdir(handle: usize, buffer: &mut [u8]) -> Option<&[u8]> {
    read_into(SYS_READDIR, handle, buffer)
}

/// Writes from a caller-owned stack buffer.
pub fn write(handle: usize, bytes: &[u8]) -> Option<usize> {
    syscall_result(syscall(
        SYS_WRITE,
        handle,
        bytes.as_ptr().expose_provenance(),
        bytes.len(),
    ))
}
