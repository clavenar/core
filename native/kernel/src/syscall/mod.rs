use clv_abi::agent::{
    AgentLaunch, NO_DEFINITION, RUN_PLAN_DEFINITION, RUN_PLAN_INTENT, RUN_REPLY_LIMIT,
    RUN_STAGE_RENAME, RUN_SUSPENDED,
};
use clv_abi::audit::DelegationDenial;
use clv_abi::goal::{CreateOptions, FileRecipient, ReadLimits, ReviewToken};
use clv_abi::tool::ToolId;
use clv_abi::{
    ChildEvent, NO_CHILD_HANDLE, NO_DIRECTORY_GRANT, OPEN_READ, OPEN_WRITE_TRUNCATE,
    PATH_MAX_BYTES, PROBE_VALUE, PROGRAM_HELLO, Program, ReplyKind, RequestKind,
    SYS_AGENT_DELEGATE, SYS_AGENT_DELEGATE_WITH_FILE, SYS_AGENT_DELEGATE_WITH_FILE_ARG,
    SYS_AGENT_INVOKE_FILE_TOOL, SYS_AGENT_RESUME, SYS_AGENT_RUN, SYS_AGENT_RUN_SUSPENDED,
    SYS_AGENT_RUN_WITH_REPLY_BUDGET, SYS_AGENT_RUN_WITH_REQUEST, SYS_AGENT_STAGE_RENAME,
    SYS_AGENT_STATUS, SYS_AGENT_STATUS_WITH_DEFINITION, SYS_AGENT_SUSPEND, SYS_CHDIR, SYS_CLOSE,
    SYS_EVENT_WAIT, SYS_EXIT, SYS_GETCWD, SYS_GOAL_AUDIT, SYS_GOAL_AUDIT_WITH_CALLER,
    SYS_GOAL_AUDIT_WITH_TOOL, SYS_GOAL_BUDGET, SYS_GOAL_CANCEL, SYS_GOAL_CLOSE, SYS_GOAL_COMMIT,
    SYS_GOAL_CREATE, SYS_GOAL_CREATE_PRESERVING_FILE, SYS_GOAL_CREATE_WITH_OPTIONS,
    SYS_GOAL_CREATE_WITH_REPLY_BUDGET, SYS_GOAL_DIFF, SYS_GOAL_DISCARD,
    SYS_GOAL_GRANT_DELEGABLE_FILE, SYS_GOAL_GRANT_FILE, SYS_GOAL_GRANT_FILE_FOR_RENAME,
    SYS_GOAL_GRANT_FILE_TO, SYS_GOAL_GRANT_FILE_TO_WITH_ARG, SYS_GOAL_READ_AUDIT,
    SYS_GOAL_READ_AUDIT_WITH_LIMIT, SYS_GOAL_RENAME_AUDIT, SYS_GOAL_REPLY_AUDIT, SYS_GOAL_REVIEW,
    SYS_GOAL_REVIEW_WITH_DEADLINE, SYS_GOAL_REVOKE_FILE, SYS_GOAL_RUN,
    SYS_GOAL_RUN_WITH_REPLY_BUDGET, SYS_GOAL_STAGE_RENAME, SYS_GOAL_STATUS, SYS_MESSAGE_RECV,
    SYS_MESSAGE_SEND, SYS_MKDIR, SYS_OPEN, SYS_OPEN_AT, SYS_PIPE_CLOSE, SYS_PIPE_READ,
    SYS_PIPE_WRITE, SYS_PROBE, SYS_READ, SYS_READ_CONSOLE, SYS_READDIR, SYS_REPLY_RECV,
    SYS_REPLY_SEND, SYS_REQUEST_RECV, SYS_RMDIR, SYS_SPAWN, SYS_SPAWN_AT,
    SYS_SPAWN_AT_WITH_MESSAGE, SYS_SPAWN_REQUEST, SYS_SPAWN_WITH_FILE, SYS_SPAWN_WITH_FILE_ARG,
    SYS_SPAWN_WITH_FILE_PIPE, SYS_SPAWN_WITH_MESSAGE, SYS_TIMER_FIRED, SYS_UNLINK, SYS_WAIT,
    SYS_WRITE, SYS_WRITE_CONSOLE, SYSCALL_ERROR,
};

use crate::arch::riscv64::trap::TrapFrame;
use crate::console::write_task;
use crate::drivers::uart;
use crate::fs::ramfs::{FileGrantMode, GrantArgument, RamFs, TaskFs, WRITABLE_FILE_CAPACITY};
use crate::ipc::pipe::PipeResult;
use crate::ipc::{Message, MessageKind};
use crate::memory::usercopy::UserStack;
use crate::scheduler::{
    DelegationAudit, DelegationTarget, LaunchScope, Mailbox, ReceiveResult, Scheduler, WaitResult,
};

const CONSOLE_WRITE_LIMIT: usize = 128;
const REG_A0: usize = 10;
const REG_A1: usize = 11;
const REG_A2: usize = 12;
const REG_A3: usize = 13;
const REG_A4: usize = 14;
const REG_A5: usize = 15;
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
        SYS_READ if stack.contains(a1, a2) => scheduler
            .read_file(fs, a0, a2, |bytes| stack.copy_to_user(bytes, a1, a2))
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
        SYS_SPAWN_WITH_FILE_ARG => spawn_with_request(
            scheduler,
            fs,
            a0,
            LaunchScope::File(a1),
            RequestKind::Argument,
            a2,
            a3,
        )
        .unwrap_or(SYSCALL_ERROR),
        SYS_SPAWN_WITH_FILE_PIPE => scheduler
            .spawn(fs, a0, LaunchScope::FileWithPipe(a1), None)
            .unwrap_or(SYSCALL_ERROR),
        SYS_GOAL_CREATE | SYS_GOAL_CREATE_WITH_REPLY_BUDGET => scheduler
            .create_goal(
                CreateOptions {
                    child_limit: 1,
                    reply_limit: (number == SYS_GOAL_CREATE_WITH_REPLY_BUDGET).then_some(a0),
                    allowed_helper: None,
                },
                ReadLimits::default(),
                None,
                None,
                fs,
            )
            .unwrap_or(SYSCALL_ERROR),
        SYS_GOAL_CREATE_WITH_OPTIONS | SYS_GOAL_CREATE_PRESERVING_FILE => {
            let preserving = number == SYS_GOAL_CREATE_PRESERVING_FILE;
            let words = if preserving {
                read_create_options(stack, a1, a2)
            } else {
                Some([
                    a0,
                    a1,
                    a2,
                    a3,
                    frame.registers[REG_A4],
                    frame.registers[REG_A5],
                ])
            };
            words
                .and_then(CreateOptions::from_registers_with_allowed_tool)
                .and_then(|(options, limits, tool)| {
                    scheduler.create_goal(options, limits, tool, preserving.then_some(a0), fs)
                })
                .unwrap_or(SYSCALL_ERROR)
        }
        SYS_GOAL_RUN | SYS_GOAL_RUN_WITH_REPLY_BUDGET => {
            let scope = LaunchScope::Goal {
                handle: a0,
                reply_bytes: (number == SYS_GOAL_RUN_WITH_REPLY_BUDGET).then_some(a2),
            };
            scheduler
                .spawn(fs, a1, scope, None)
                .unwrap_or(SYSCALL_ERROR)
        }
        SYS_GOAL_STAGE_RENAME => scheduler
            .stage_goal_rename(a0, fs, a1, a3, |name| stack.copy_from_user(a2, name))
            .unwrap_or(SYSCALL_ERROR),
        SYS_AGENT_STAGE_RENAME => scheduler
            .stage_agent_rename(fs, a0, a2, |name| stack.copy_from_user(a1, name))
            .unwrap_or(SYSCALL_ERROR),
        SYS_GOAL_DIFF | SYS_GOAL_REVIEW | SYS_GOAL_REVIEW_WITH_DEADLINE => scheduler
            .read_goal_diff(a0, number != SYS_GOAL_DIFF, fs, |names| {
                stack.copy_to_user(names, a1, a2)
            })
            .map_or(SYSCALL_ERROR, |(count, source_len, review)| {
                frame.registers[REG_A1] = source_len;
                if number != SYS_GOAL_DIFF {
                    (frame.registers[REG_A2], frame.registers[REG_A3]) =
                        review.review_registers(number == SYS_GOAL_REVIEW_WITH_DEADLINE);
                }
                count
            }),
        SYS_GOAL_COMMIT => ReviewToken::new(a1)
            .and_then(|token| scheduler.commit_goal(a0, token, fs))
            .map_or(SYSCALL_ERROR, |result| result as usize),
        SYS_GOAL_DISCARD => scheduler
            .discard_goal_rename(a0, fs)
            .map_or(SYSCALL_ERROR, |result| result as usize),
        SYS_GOAL_BUDGET => scheduler.goal_budget(a0).map_or(SYSCALL_ERROR, |budget| {
            let (flags, available, reserved, spent) = budget.registers();
            frame.registers[REG_A1] = available;
            frame.registers[REG_A2] = reserved;
            frame.registers[REG_A3] = spent;
            flags
        }),
        SYS_GOAL_AUDIT | SYS_GOAL_AUDIT_WITH_CALLER | SYS_GOAL_AUDIT_WITH_TOOL => {
            let view = match number {
                SYS_GOAL_AUDIT => DelegationAudit::Decision,
                SYS_GOAL_AUDIT_WITH_CALLER => DelegationAudit::Caller,
                _ => DelegationAudit::Tool,
            };
            write_audit_result(
                frame,
                scheduler
                    .goal_delegation_audit(a0, view, |bytes| stack.copy_to_user(bytes, a1, a2)),
            )
        }
        SYS_GOAL_REPLY_AUDIT => write_audit_result(
            frame,
            scheduler.goal_reply_audit(a0, |bytes| stack.copy_to_user(bytes, a1, a2)),
        ),
        SYS_GOAL_RENAME_AUDIT => write_audit_result(
            frame,
            scheduler.goal_rename_audit(a0, |bytes| stack.copy_to_user(bytes, a1, a2)),
        ),
        SYS_GOAL_READ_AUDIT | SYS_GOAL_READ_AUDIT_WITH_LIMIT => scheduler
            .goal_read_audit(a0, number == SYS_GOAL_READ_AUDIT_WITH_LIMIT, |bytes| {
                stack.copy_to_user(bytes, a1, a2)
            })
            .map_or(SYSCALL_ERROR, |(count, overwritten, spent)| {
                frame.registers[REG_A1] = overwritten;
                frame.registers[REG_A2] = spent;
                count
            }),
        SYS_GOAL_GRANT_FILE | SYS_GOAL_GRANT_DELEGABLE_FILE | SYS_GOAL_GRANT_FILE_FOR_RENAME => {
            let mode = match number {
                SYS_GOAL_GRANT_DELEGABLE_FILE => FileGrantMode::Delegate(FileRecipient::Any),
                SYS_GOAL_GRANT_FILE_FOR_RENAME => FileGrantMode::StageRename,
                _ => FileGrantMode::Read,
            };
            scheduler
                .grant_goal_file(a0, a1, mode, None, fs)
                .map_or(SYSCALL_ERROR, |()| 0)
        }
        SYS_GOAL_GRANT_FILE_TO | SYS_GOAL_GRANT_FILE_TO_WITH_ARG => Program::from_raw(a2)
            .and_then(|program| {
                let argument = if number == SYS_GOAL_GRANT_FILE_TO_WITH_ARG {
                    Some(GrantArgument::copy_from(
                        frame.registers[REG_A4],
                        |bytes| stack.copy_from_user(a3, bytes),
                    )?)
                } else {
                    None
                };
                scheduler.grant_goal_file(
                    a0,
                    a1,
                    FileGrantMode::Delegate(FileRecipient::Program(program)),
                    argument,
                    fs,
                )
            })
            .map_or(SYSCALL_ERROR, |()| 0),
        SYS_GOAL_REVOKE_FILE => scheduler
            .revoke_goal_file(a0, fs)
            .map_or(SYSCALL_ERROR, |()| 0),
        SYS_GOAL_STATUS => scheduler.goal_status(a0).map_or(SYSCALL_ERROR, |status| {
            write_status(frame, status.registers())
        }),
        SYS_GOAL_CLOSE => scheduler
            .close_goal(a0, fs)
            .map_or(SYSCALL_ERROR, |result| result as usize),
        SYS_GOAL_CANCEL => scheduler
            .cancel_goal(a0, fs)
            .map_or(SYSCALL_ERROR, |result| result as usize),
        SYS_AGENT_RUN | SYS_AGENT_RUN_SUSPENDED => write_agent_launch(
            frame,
            scheduler.run_agent(fs, a0, a1, number == SYS_AGENT_RUN_SUSPENDED, None, None),
        ),
        SYS_AGENT_RUN_WITH_REPLY_BUDGET if a3 <= 1 => write_agent_launch(
            frame,
            scheduler.run_agent(fs, a0, a1, a3 == 1, Some(a2), None),
        ),
        SYS_AGENT_RUN_WITH_REQUEST => {
            let kind = match a2 & !(RUN_SUSPENDED | RUN_REPLY_LIMIT) {
                0 => Some(RequestKind::DelegateWithFile),
                RUN_PLAN_INTENT => Some(RequestKind::PlanIntent),
                RUN_PLAN_DEFINITION => Some(RequestKind::PlanDefinition),
                RUN_STAGE_RENAME => Some(RequestKind::StageRename),
                _ => None,
            };
            let launch = kind
                .and_then(|kind| {
                    copy_request(
                        scheduler,
                        kind,
                        frame.registers[REG_A4],
                        frame.registers[REG_A5],
                    )
                })
                .and_then(|request| {
                    scheduler.run_agent(
                        fs,
                        a0,
                        a1,
                        a2 & RUN_SUSPENDED != 0,
                        (a2 & RUN_REPLY_LIMIT != 0).then_some(a3),
                        Some(request),
                    )
                });
            write_agent_launch(frame, launch)
        }
        SYS_AGENT_DELEGATE
        | SYS_AGENT_DELEGATE_WITH_FILE
        | SYS_AGENT_DELEGATE_WITH_FILE_ARG
        | SYS_AGENT_INVOKE_FILE_TOOL => {
            let target = if number == SYS_AGENT_INVOKE_FILE_TOOL {
                DelegationTarget::FileTool(ToolId(a1 as u64))
            } else {
                DelegationTarget::Program {
                    selector: a1,
                    with_file: number != SYS_AGENT_DELEGATE,
                }
            };
            let argument = if matches!(
                number,
                SYS_AGENT_DELEGATE_WITH_FILE_ARG | SYS_AGENT_INVOKE_FILE_TOOL
            ) {
                copy_request(
                    scheduler,
                    RequestKind::Argument,
                    a3,
                    frame.registers[REG_A4],
                )
                .map(Some)
                .ok_or(DelegationDenial::InvalidArgument)
            } else {
                Ok(None)
            };
            scheduler
                .delegate(fs, a0, target, a2, argument)
                .unwrap_or(SYSCALL_ERROR)
        }
        SYS_AGENT_STATUS | SYS_AGENT_STATUS_WITH_DEFINITION => {
            scheduler
                .agent_status(a0)
                .map_or(SYSCALL_ERROR, |(agent, pin)| {
                    if number == SYS_AGENT_STATUS_WITH_DEFINITION
                        && !stack.copy_to_user(pin.unwrap_or(&NO_DEFINITION), a1, a2)
                    {
                        return SYSCALL_ERROR;
                    }
                    frame.registers[REG_A3] = agent.goal;
                    write_status(frame, agent.registers())
                })
        }
        SYS_AGENT_SUSPEND | SYS_AGENT_RESUME => scheduler
            .set_agent_suspended(a0, number == SYS_AGENT_SUSPEND)
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
        SYS_SPAWN_WITH_MESSAGE => spawn_with_request(
            scheduler,
            fs,
            a0,
            LaunchScope::Plain,
            RequestKind::Echo,
            a1,
            a2,
        )
        .unwrap_or(SYSCALL_ERROR),
        SYS_SPAWN_AT_WITH_MESSAGE => {
            // The original scoped call supplied a path to hello and raw bytes to echo.
            let kind = if a0 == PROGRAM_HELLO {
                RequestKind::ReadFile
            } else {
                RequestKind::Echo
            };
            spawn_with_request(scheduler, fs, a0, LaunchScope::Directory(a1), kind, a2, a3)
                .unwrap_or(SYSCALL_ERROR)
        }
        SYS_SPAWN_REQUEST => {
            let fd = frame.registers[REG_A4];
            let scope = if fd == NO_DIRECTORY_GRANT {
                LaunchScope::Plain
            } else {
                LaunchScope::Directory(fd)
            };
            RequestKind::from_raw(a1)
                .filter(|kind| {
                    matches!(
                        kind,
                        RequestKind::Echo | RequestKind::ReadFile | RequestKind::Argument
                    )
                })
                .and_then(|kind| spawn_with_request(scheduler, fs, a0, scope, kind, a2, a3))
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

fn write_audit_result(frame: &mut TrapFrame, snapshot: Option<(usize, usize)>) -> usize {
    snapshot.map_or(SYSCALL_ERROR, |(count, overwritten)| {
        frame.registers[REG_A1] = overwritten;
        count
    })
}

fn write_status(frame: &mut TrapFrame, (state, child, value): (usize, usize, usize)) -> usize {
    frame.registers[REG_A1] = child;
    frame.registers[REG_A2] = value;
    state
}

fn write_agent_launch(frame: &mut TrapFrame, launch: Option<AgentLaunch>) -> usize {
    launch.map_or(SYSCALL_ERROR, |launch| {
        frame.registers[REG_A1] = launch.child;
        launch.agent
    })
}

fn spawn_with_request(
    scheduler: &mut Scheduler,
    fs: &mut RamFs,
    program: usize,
    scope: LaunchScope,
    kind: RequestKind,
    address: usize,
    length: usize,
) -> Option<usize> {
    let request = copy_request(scheduler, kind, address, length)?;
    scheduler.spawn(fs, program, scope, Some(request))
}

fn copy_request(
    scheduler: &Scheduler,
    kind: RequestKind,
    address: usize,
    length: usize,
) -> Option<Message> {
    let stack = scheduler.address_space().stack;
    Message::new(
        scheduler.current_id(),
        MessageKind::Request(kind),
        length,
        |bytes| stack.copy_from_user(address, bytes),
    )
}

fn read_create_options(stack: UserStack, address: usize, length: usize) -> Option<[usize; 6]> {
    let mut words = [[0; size_of::<usize>()]; 6];
    let bytes = words.as_flattened_mut();
    if length != bytes.len() || !stack.copy_from_user(address, bytes) {
        return None;
    }
    Some(words.map(usize::from_le_bytes))
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
