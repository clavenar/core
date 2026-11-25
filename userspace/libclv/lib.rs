#![no_std]

//! Native RV64 user calls. Paths and outgoing messages are staged on the stack.
//! Receive buffers and direct file and console I/O use the caller's stack buffers.

pub mod fs;

use clv_abi::{
    MESSAGE_MAX_BYTES, NO_DIRECTORY_GRANT, RequestKind, SYS_EXIT, SYS_MESSAGE_RECV,
    SYS_MESSAGE_SEND, SYS_READ_CONSOLE, SYS_REQUEST_RECV, SYS_SPAWN, SYS_SPAWN_AT,
    SYS_SPAWN_AT_WITH_MESSAGE, SYS_SPAWN_REQUEST, SYS_SPAWN_WITH_FILE, SYS_SPAWN_WITH_MESSAGE,
    SYS_TIMER_FIRED, SYS_WAIT, SYS_WRITE_CONSOLE, SYSCALL_ERROR, WAIT_EXITED, WAIT_FAULTED,
};
use core::arch::asm;

#[inline(never)]
fn ecall(
    number: usize,
    mut a0: usize,
    mut a1: usize,
    mut a2: usize,
    a3: usize,
    a4: usize,
) -> (usize, usize, usize) {
    // SAFETY: The kernel takes the number in a7 and arguments in a0 through a4.
    // Results use a0 through a2. The kernel preserves the other registers.
    unsafe {
        asm!(
            "ecall",
            inlateout("a0") a0,
            inlateout("a1") a1,
            inlateout("a2") a2,
            in("a3") a3,
            in("a4") a4,
            in("a7") number,
            options(nostack),
        );
    }
    (a0, a1, a2)
}

fn syscall(number: usize, a0: usize, a1: usize, a2: usize) -> usize {
    ecall(number, a0, a1, a2, 0, 0).0
}

fn with_stack_bytes<const CAPACITY: usize>(
    bytes: &[u8],
    operation: impl FnOnce(usize, usize) -> usize,
) -> Option<usize> {
    let mut buffer = [0; CAPACITY];
    let destination = buffer.get_mut(..bytes.len())?;
    destination.copy_from_slice(bytes);
    syscall_result(operation(
        destination.as_ptr().expose_provenance(),
        destination.len(),
    ))
}

fn syscall_result(value: usize) -> Option<usize> {
    (value != SYSCALL_ERROR).then_some(value)
}

pub fn exit(status: usize) -> ! {
    syscall(SYS_EXIT, status, 0, 0);
    loop {
        core::hint::spin_loop();
    }
}

pub fn timer_fired() -> bool {
    syscall(SYS_TIMER_FIRED, 0, 0, 0) != 0
}

pub fn read_console() -> Option<u8> {
    u8::try_from(syscall(SYS_READ_CONSOLE, 0, 0, 0)).ok()
}

/// Writes from a caller-owned stack buffer in one syscall.
pub fn write_console(bytes: &[u8]) -> Option<usize> {
    syscall_result(syscall(
        SYS_WRITE_CONSOLE,
        bytes.as_ptr().expose_provenance(),
        bytes.len(),
        0,
    ))
}

pub fn print(text: &[u8]) {
    let mut buffer = [0; 32];
    for chunk in text.chunks(buffer.len()) {
        let bytes = &mut buffer[..chunk.len()];
        bytes.copy_from_slice(chunk);
        write_console(bytes);
    }
}

pub fn spawn(program: usize, directory: Option<usize>, request: Option<&[u8]>) -> Option<usize> {
    match request {
        Some(bytes) => {
            with_stack_bytes::<MESSAGE_MAX_BYTES>(bytes, |address, length| match directory {
                Some(fd) => ecall(SYS_SPAWN_AT_WITH_MESSAGE, program, fd, address, length, 0).0,
                None => syscall(SYS_SPAWN_WITH_MESSAGE, program, address, length),
            })
        }
        None => syscall_result(match directory {
            Some(fd) => syscall(SYS_SPAWN_AT, program, fd, 0),
            None => syscall(SYS_SPAWN, program, 0, 0),
        }),
    }
}

pub fn spawn_with_file(program: usize, file: usize) -> Option<usize> {
    syscall_result(syscall(SYS_SPAWN_WITH_FILE, program, file, 0))
}

pub fn spawn_request(
    program: usize,
    directory: Option<usize>,
    kind: RequestKind,
    payload: &[u8],
) -> Option<usize> {
    with_stack_bytes::<MESSAGE_MAX_BYTES>(payload, |address, length| {
        ecall(
            SYS_SPAWN_REQUEST,
            program,
            kind as usize,
            address,
            length,
            directory.unwrap_or(NO_DIRECTORY_GRANT),
        )
        .0
    })
}

pub enum ChildStatus {
    Exited(usize),
    Faulted(usize),
}

pub fn wait(handle: usize) -> Option<ChildStatus> {
    match ecall(SYS_WAIT, handle, 0, 0, 0, 0) {
        (WAIT_EXITED, status, _) => Some(ChildStatus::Exited(status)),
        (WAIT_FAULTED, cause, _) => Some(ChildStatus::Faulted(cause)),
        _ => None,
    }
}

pub fn send_message(bytes: &[u8]) -> bool {
    with_stack_bytes::<MESSAGE_MAX_BYTES>(bytes, |address, length| {
        syscall(SYS_MESSAGE_SEND, address, length, 0)
    }) == Some(bytes.len())
}

/// Returns the received bytes and the sender's task ID.
pub fn receive_message(handle: usize, buffer: &mut [u8]) -> Option<(&[u8], usize)> {
    let (count, sender, _) = ecall(
        SYS_MESSAGE_RECV,
        buffer.as_mut_ptr().expose_provenance(),
        buffer.len(),
        handle,
        0,
        0,
    );
    Some((buffer.get(..count)?, sender))
}

pub enum LaunchRequest<'a> {
    Empty,
    Received {
        kind: RequestKind,
        bytes: &'a [u8],
        sender: usize,
    },
}

/// Reads the calling child's launch request. The kernel supplies the sender's task ID.
pub fn receive_request(buffer: &mut [u8]) -> Option<LaunchRequest<'_>> {
    let (count, sender, kind) = ecall(
        SYS_REQUEST_RECV,
        buffer.as_mut_ptr().expose_provenance(),
        buffer.len(),
        0,
        0,
        0,
    );
    let bytes = buffer.get(..count)?;
    if bytes.is_empty() {
        return Some(LaunchRequest::Empty);
    }
    Some(LaunchRequest::Received {
        kind: RequestKind::from_raw(kind)?,
        bytes,
        sender,
    })
}
