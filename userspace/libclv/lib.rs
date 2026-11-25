#![no_std]

//! Native RV64 user calls. Paths and outgoing messages are staged on the stack.
//! Receive buffers and direct file and console I/O use the caller's stack buffers.

pub mod fs;
pub mod goal;
pub mod pipe;

use clv_abi::{
    ChildEvent, CompletionKind, MESSAGE_MAX_BYTES, NO_CHILD_HANDLE, NO_DIRECTORY_GRANT, ReplyKind,
    RequestKind, SYS_EVENT_WAIT, SYS_EXIT, SYS_MESSAGE_RECV, SYS_MESSAGE_SEND, SYS_READ_CONSOLE,
    SYS_REPLY_RECV, SYS_REPLY_SEND, SYS_REQUEST_RECV, SYS_SPAWN, SYS_SPAWN_AT,
    SYS_SPAWN_AT_WITH_MESSAGE, SYS_SPAWN_REQUEST, SYS_SPAWN_WITH_FILE, SYS_SPAWN_WITH_FILE_PIPE,
    SYS_SPAWN_WITH_MESSAGE, SYS_TIMER_FIRED, SYS_WAIT, SYS_WRITE_CONSOLE, SYSCALL_ERROR,
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

fn read_into(number: usize, handle: usize, buffer: &mut [u8]) -> Option<&[u8]> {
    let count = syscall(
        number,
        handle,
        buffer.as_mut_ptr().expose_provenance(),
        buffer.len(),
    );
    buffer.get(..count)
}

fn write_from(number: usize, handle: usize, bytes: &[u8]) -> Option<usize> {
    syscall_result(syscall(
        number,
        handle,
        bytes.as_ptr().expose_provenance(),
        bytes.len(),
    ))
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

/// Grants a file to the child and returns the parent's handle for pipe reads and wait.
pub fn spawn_with_file_pipe(program: usize, file: usize) -> Option<usize> {
    syscall_result(syscall(SYS_SPAWN_WITH_FILE_PIPE, program, file, 0))
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
    let (tag, value, _) = ecall(SYS_WAIT, handle, 0, 0, 0, 0);
    Some(match CompletionKind::from_raw(tag)? {
        CompletionKind::Exited => ChildStatus::Exited(value),
        CompletionKind::Faulted => ChildStatus::Faulted(value),
    })
}

/// Takes the oldest child notification. The exit status or fault cause is available through `wait`.
pub fn event_wait() -> Option<ChildEvent> {
    let (handle, tag, _) = ecall(SYS_EVENT_WAIT, 0, 0, 0, 0, 0);
    match handle {
        SYSCALL_ERROR => None,
        NO_CHILD_HANDLE => Some(ChildEvent::Empty),
        _ => Some(ChildEvent::Completed {
            handle,
            kind: CompletionKind::from_raw(tag)?,
        }),
    }
}

pub fn send_message(bytes: &[u8]) -> bool {
    with_stack_bytes::<MESSAGE_MAX_BYTES>(bytes, |address, length| {
        syscall(SYS_MESSAGE_SEND, address, length, 0)
    }) == Some(bytes.len())
}

/// Queues a typed reply for the calling child's parent.
pub fn send_reply(kind: ReplyKind, bytes: &[u8]) -> bool {
    with_stack_bytes::<MESSAGE_MAX_BYTES>(bytes, |address, length| {
        syscall(SYS_REPLY_SEND, address, length, kind as usize)
    }) == Some(bytes.len())
}

fn receive(number: usize, handle: usize, buffer: &mut [u8]) -> Option<(&[u8], usize, usize)> {
    let (count, sender, tag) = ecall(
        number,
        buffer.as_mut_ptr().expose_provenance(),
        buffer.len(),
        handle,
        0,
        0,
    );
    Some((buffer.get(..count)?, sender, tag))
}

/// Returns the received bytes and the sender's task ID.
pub fn receive_message(handle: usize, buffer: &mut [u8]) -> Option<(&[u8], usize)> {
    let (bytes, sender, _) = receive(SYS_MESSAGE_RECV, handle, buffer)?;
    Some((bytes, sender))
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
    let (bytes, sender, kind) = receive(SYS_REQUEST_RECV, 0, buffer)?;
    if kind == 0 {
        return Some(LaunchRequest::Empty);
    }
    Some(LaunchRequest::Received {
        kind: RequestKind::from_raw(kind)?,
        bytes,
        sender,
    })
}

pub enum ChildReply<'a> {
    Empty,
    Received {
        kind: ReplyKind,
        bytes: &'a [u8],
        sender: usize,
    },
}

/// A received reply has a kind and a sender even when its payload is empty.
pub fn receive_reply(handle: usize, buffer: &mut [u8]) -> Option<ChildReply<'_>> {
    let (bytes, sender, kind) = receive(SYS_REPLY_RECV, handle, buffer)?;
    if kind == 0 {
        return Some(ChildReply::Empty);
    }
    Some(ChildReply::Received {
        kind: ReplyKind::from_raw(kind)?,
        bytes,
        sender,
    })
}
