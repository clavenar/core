use clv_abi::{
    ChildEvent, NO_CHILD_HANDLE, NO_DIRECTORY_GRANT, OPEN_READ, OPEN_WRITE_TRUNCATE,
    PATH_MAX_BYTES, PROBE_VALUE, PROGRAM_HELLO, ReplyKind, RequestKind, SYS_CHDIR, SYS_CLOSE,
    SYS_EVENT_WAIT, SYS_EXIT, SYS_GETCWD, SYS_GOAL_CLOSE, SYS_GOAL_CREATE, SYS_GOAL_RUN,
    SYS_GOAL_STATUS, SYS_MESSAGE_RECV, SYS_MESSAGE_SEND, SYS_MKDIR, SYS_OPEN, SYS_OPEN_AT,
    SYS_PIPE_CLOSE, SYS_PIPE_READ, SYS_PIPE_WRITE, SYS_PROBE, SYS_READ, SYS_READ_CONSOLE,
    SYS_READDIR, SYS_REPLY_RECV, SYS_REPLY_SEND, SYS_REQUEST_RECV, SYS_RMDIR, SYS_SPAWN,
    SYS_SPAWN_AT, SYS_SPAWN_AT_WITH_MESSAGE, SYS_SPAWN_REQUEST, SYS_SPAWN_WITH_FILE,
    SYS_SPAWN_WITH_FILE_PIPE, SYS_SPAWN_WITH_MESSAGE, SYS_TIMER_FIRED, SYS_UNLINK, SYS_WAIT,
    SYS_WRITE, SYS_WRITE_CONSOLE, SYSCALL_ERROR,
};

use crate::arch::riscv64::trap::TrapFrame;
use crate::console::write_task;
use crate::drivers::uart;
use crate::fs::ramfs::{RamFs, TaskFs, WRITABLE_FILE_CAPACITY};
use crate::ipc::pipe::PipeResult;
use crate::ipc::{Message, MessageKind};
use crate::memory::usercopy::UserStack;
use crate::scheduler::{LaunchScope, Mailbox, ReceiveResult, Scheduler, WaitResult};

const CONSOLE_WRITE_LIMIT: usize = 128;
const REG_A0: usize = 10;
const REG_A1: usize = 11;
const REG_A2: usize = 12;
const REG_A3: usize = 13;
const REG_A4: usize = 14;
const REG_A7: usize = 17;

pub(crate) enum Outcome {
    Resume,
    Exit(usize),
    Wait,
}

pub(crate) fn dispatch(
    scheduler: &mut Scheduler,
    fs: &mut RamFs,
    frame: &mut TrapFrame,
) -> Outcome {
    let a0 = frame.registers[REG_A0];
    let a1 = frame.registers[REG_A1];
    let a2 = frame.registers[REG_A2];
    let a3 = frame.registers[REG_A3];
    let number = frame.registers[REG_A7];
    let stack = scheduler.address_space().stack;
    frame.registers[REG_A0] = match number {
        SYS_PROBE => {
            write_task(scheduler.current_id());
            uart::write(b" user probe\r\n");
            PROBE_VALUE
        }
        SYS_EXIT => return Outcome::Exit(a0),
        SYS_WRITE_CONSOLE => write_console(stack, a0, a1),
        SYS_TIMER_FIRED => usize::from(scheduler.timer_fired()),
        SYS_READ_CONSOLE => uart::read_byte().map_or(SYSCALL_ERROR, usize::from),
        SYS_OPEN => open_file(scheduler.task_fs(), fs, stack, a0, a1, a2).unwrap_or(SYSCALL_ERROR),
        SYS_OPEN_AT => with_user_path(stack, a1, a2, |path| {
            scheduler.task_fs().open_at(fs, a0, path)
        })
        .unwrap_or(SYSCALL_ERROR),
        SYS_READ => scheduler
            .task_fs()
            .read(fs, a0, a2, |bytes| stack.copy_to_user(bytes, a1, a2))
            .unwrap_or(SYSCALL_ERROR),
        SYS_READDIR => scheduler
            .task_fs()
            .readdir(fs, a0, |name| stack.copy_to_user(name, a1, a2))
            .unwrap_or(SYSCALL_ERROR),
        SYS_CLOSE => scheduler.task_fs().close(fs, a0).unwrap_or(SYSCALL_ERROR),
        SYS_WRITE => {
            write_file(scheduler.task_fs(), fs, stack, a0, a1, a2).unwrap_or(SYSCALL_ERROR)
        }
        SYS_UNLINK => with_user_path(stack, a0, a1, |path| scheduler.task_fs().unlink(fs, path))
            .unwrap_or(SYSCALL_ERROR),
        SYS_MKDIR => with_user_path(stack, a0, a1, |path| scheduler.task_fs().mkdir(fs, path))
            .unwrap_or(SYSCALL_ERROR),
        SYS_CHDIR => with_user_path(stack, a0, a1, |path| scheduler.task_fs().chdir(fs, path))
            .unwrap_or(SYSCALL_ERROR),
        SYS_GETCWD => scheduler
            .task_fs()
            .getcwd(fs, |path| stack.copy_to_user(path, a0, a1))
            .unwrap_or(SYSCALL_ERROR),
        SYS_RMDIR => {
            with_user_path(stack, a0, a1, |path| scheduler.rmdir(fs, path)).unwrap_or(SYSCALL_ERROR)
        }
        SYS_SPAWN => scheduler
            .spawn(fs, a0, LaunchScope::Plain, None)
            .unwrap_or(SYSCALL_ERROR),
        SYS_SPAWN_AT => scheduler
            .spawn(fs, a0, LaunchScope::Directory(a1), None)
            .unwrap_or(SYSCALL_ERROR),
        SYS_SPAWN_WITH_FILE => scheduler
            .spawn(fs, a0, LaunchScope::File(a1), None)
            .unwrap_or(SYSCALL_ERROR),
        SYS_SPAWN_WITH_FILE_PIPE => scheduler
            .spawn(fs, a0, LaunchScope::FileWithPipe(a1), None)
            .unwrap_or(SYSCALL_ERROR),
        SYS_GOAL_CREATE => scheduler.create_goal().unwrap_or(SYSCALL_ERROR),
        SYS_GOAL_RUN => scheduler
            .spawn(fs, a1, LaunchScope::Goal(a0), None)
            .unwrap_or(SYSCALL_ERROR),
        SYS_GOAL_STATUS => match scheduler.goal_status(a0) {
            Some(status) => {
                let (state, child, value) = status.registers();
                frame.registers[REG_A1] = child;
                frame.registers[REG_A2] = value;
                state
            }
            None => SYSCALL_ERROR,
        },
        SYS_GOAL_CLOSE => scheduler
            .close_goal(a0)
            .map_or(SYSCALL_ERROR, |result| result as usize),
        SYS_PIPE_READ | SYS_PIPE_WRITE if stack.contains(a1, a2) => {
            let result = if number == SYS_PIPE_READ {
                scheduler.read_pipe(a0, a2, frame, |bytes| stack.copy_to_user(bytes, a1, a2))
            } else {
                scheduler.write_pipe(a0, a2, frame, |bytes| stack.copy_from_user(a1, bytes))
            };
            match result {
                PipeResult::Ready(count) => count,
                PipeResult::Wait => return Outcome::Wait,
                PipeResult::Error => SYSCALL_ERROR,
            }
        }
        SYS_PIPE_CLOSE => {
            if scheduler.close_pipe(a0) {
                0
            } else {
                SYSCALL_ERROR
            }
        }
        SYS_SPAWN_WITH_MESSAGE => {
            spawn_with_request(scheduler, fs, a0, None, RequestKind::Echo, a1, a2)
                .unwrap_or(SYSCALL_ERROR)
        }
        SYS_SPAWN_AT_WITH_MESSAGE => {
            // The original scoped call supplied a path to hello and raw bytes to echo.
            let kind = if a0 == PROGRAM_HELLO {
                RequestKind::ReadFile
            } else {
                RequestKind::Echo
            };
            spawn_with_request(scheduler, fs, a0, Some(a1), kind, a2, a3).unwrap_or(SYSCALL_ERROR)
        }
        SYS_SPAWN_REQUEST => {
            let fd = frame.registers[REG_A4];
            let directory = (fd != NO_DIRECTORY_GRANT).then_some(fd);
            RequestKind::from_raw(a1)
                .and_then(|kind| spawn_with_request(scheduler, fs, a0, directory, kind, a2, a3))
                .unwrap_or(SYSCALL_ERROR)
        }
        SYS_WAIT => match scheduler.wait_child(a0, frame) {
            WaitResult::Unavailable => SYSCALL_ERROR,
            WaitResult::Blocked => return Outcome::Wait,
            WaitResult::Ready(completion) => {
                let (kind, value) = completion.kind_and_value();
                frame.registers[REG_A1] = value;
                kind as usize
            }
        },
        SYS_EVENT_WAIT => match scheduler.wait_event(frame) {
            WaitResult::Unavailable => SYSCALL_ERROR,
            WaitResult::Blocked => return Outcome::Wait,
            WaitResult::Ready(ChildEvent::Completed { handle, kind }) => {
                frame.registers[REG_A1] = kind as usize;
                handle
            }
            WaitResult::Ready(ChildEvent::Empty) => {
                frame.registers[REG_A1] = 0;
                NO_CHILD_HANDLE
            }
        },
        SYS_MESSAGE_SEND => scheduler
            .send_reply(None, a1, |bytes| stack.copy_from_user(a0, bytes))
            .unwrap_or(SYSCALL_ERROR),
        SYS_REPLY_SEND => ReplyKind::from_raw(a2)
            .and_then(|kind| {
                scheduler.send_reply(Some(kind), a1, |bytes| stack.copy_from_user(a0, bytes))
            })
            .unwrap_or(SYSCALL_ERROR),
        SYS_MESSAGE_RECV | SYS_REQUEST_RECV | SYS_REPLY_RECV
            if (a1 > 0 || number == SYS_REPLY_RECV) && stack.contains(a0, a1) =>
        {
            let legacy = number == SYS_MESSAGE_RECV;
            let mailbox = match number {
                SYS_REQUEST_RECV => Mailbox::LaunchRequest,
                SYS_MESSAGE_RECV if a2 == NO_CHILD_HANDLE => Mailbox::LaunchRequest,
                _ => Mailbox::ChildReply(a2),
            };
            match scheduler.receive_message(mailbox, legacy, frame, |bytes| {
                stack.copy_to_user(bytes, a0, a1)
            }) {
                ReceiveResult::Error => SYSCALL_ERROR,
                ReceiveResult::Blocked => return Outcome::Wait,
                ReceiveResult::Ready {
                    length,
                    sender,
                    tag,
                } => {
                    frame.registers[REG_A1] = sender;
                    if !legacy {
                        frame.registers[REG_A2] = tag;
                    }
                    length
                }
            }
        }
        _ => SYSCALL_ERROR,
    };
    // ECALL is a 32-bit instruction, including on cores with compressed code.
    frame.sepc += 4;
    Outcome::Resume
}

fn spawn_with_request(
    scheduler: &mut Scheduler,
    fs: &mut RamFs,
    program: usize,
    directory_fd: Option<usize>,
    kind: RequestKind,
    address: usize,
    length: usize,
) -> Option<usize> {
    let stack = scheduler.address_space().stack;
    let request = Message::new(
        scheduler.current_id(),
        MessageKind::Request(kind),
        length,
        |bytes| stack.copy_from_user(address, bytes),
    )?;
    let scope = directory_fd.map_or(LaunchScope::Plain, LaunchScope::Directory);
    scheduler.spawn(fs, program, scope, Some(request))
}

fn open_file(
    task_fs: &mut TaskFs,
    fs: &mut RamFs,
    stack: UserStack,
    address: usize,
    length: usize,
    mode: usize,
) -> Option<usize> {
    let writable = match mode {
        OPEN_READ => false,
        OPEN_WRITE_TRUNCATE => true,
        _ => return None,
    };
    with_user_path(stack, address, length, |path| {
        task_fs.open(fs, path, writable)
    })
}

fn with_user_path(
    stack: UserStack,
    address: usize,
    length: usize,
    operation: impl FnOnce(&[u8]) -> Option<usize>,
) -> Option<usize> {
    let mut buffer = [0; PATH_MAX_BYTES];
    let path = buffer.get_mut(..length)?;
    if !stack.copy_from_user(address, path) {
        return None;
    }
    operation(path)
}

fn write_file(
    task_fs: &mut TaskFs,
    fs: &mut RamFs,
    stack: UserStack,
    fd: usize,
    address: usize,
    length: usize,
) -> Option<usize> {
    let mut buffer = [0; WRITABLE_FILE_CAPACITY];
    let bytes = buffer.get_mut(..length)?;
    if !stack.copy_from_user(address, bytes) {
        return None;
    }
    task_fs.write(fs, fd, bytes)
}

fn write_console(stack: UserStack, address: usize, length: usize) -> usize {
    let mut buffer = [0; CONSOLE_WRITE_LIMIT];
    let Some(bytes) = buffer.get_mut(..length) else {
        return SYSCALL_ERROR;
    };
    if !stack.copy_from_user(address, bytes) {
        return SYSCALL_ERROR;
    }

    uart::write(bytes);
    length
}
