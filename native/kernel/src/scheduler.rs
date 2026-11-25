mod goal;
mod pipe;

use self::goal::Goal;
use clv_abi::{
    ChildEvent, CompletionKind, GrantKind, PROGRAM_ECHO, PROGRAM_HELLO, ReplyKind, RequestKind,
};

use crate::arch::riscv64::trap::TrapFrame;
use crate::fs::ramfs::{RamFs, TaskFs};
use crate::ipc::pipe::Pipe;
use crate::ipc::{Message, MessageKind};
use crate::memory::sv39::{AddressSpace, USER_STACK_TOP};

pub(crate) const TASK_COUNT: usize = 3;
const HELLO_TASK_INDEX: usize = 0;
const SHELL_TASK_INDEX: usize = 1;
const ECHO_TASK_INDEX: usize = 2;

#[derive(Clone, Copy, PartialEq, Eq)]
enum TaskState {
    Runnable,
    WaitingForChild(usize),
    WaitingForEvent,
    WaitingForMessage(usize),
    WaitingForPipeData(usize),
    WaitingForPipeSpace,
    Stopped,
}

impl TaskState {
    fn wake_from(&mut self, wait: Self) {
        if *self == wait {
            *self = Self::Runnable;
        }
    }

    fn waits_for_result(self, handle: usize) -> bool {
        self == Self::WaitingForChild(handle)
            || self == Self::WaitingForMessage(handle)
            || self == Self::WaitingForEvent
    }
}

#[derive(Clone, Copy)]
pub(crate) enum TaskCompletion {
    Exited(usize),
    Faulted(usize),
}

impl TaskCompletion {
    pub(crate) fn kind_and_value(self) -> (CompletionKind, usize) {
        match self {
            Self::Exited(status) => (CompletionKind::Exited, status),
            Self::Faulted(cause) => (CompletionKind::Faulted, cause),
        }
    }
}

#[derive(Clone, Copy)]
struct PendingEvent {
    sequence: usize,
    kind: CompletionKind,
}

struct ChildLaunch {
    handle: usize,
    request: Option<Message>,
    reply: Option<Message>,
    legacy_file_reply: bool,
    pipe: Option<Pipe>,
    completion: Option<TaskCompletion>,
    event: Option<PendingEvent>,
}

#[derive(Clone, Copy)]
pub(crate) enum LaunchScope {
    Plain,
    Directory(usize),
    File(usize),
    FileWithPipe(usize),
    Goal(usize),
}

pub(crate) enum WaitResult<T> {
    Unavailable,
    Blocked,
    Ready(T),
}

pub(crate) enum Mailbox {
    LaunchRequest,
    ChildReply(usize),
}

pub(crate) enum ReceiveResult {
    Error,
    Blocked,
    Ready {
        length: usize,
        sender: usize,
        tag: usize,
    },
}

struct Task {
    entry: usize,
    frame: TrapFrame,
    address_space: AddressSpace,
    fs: TaskFs,
    state: TaskState,
    timer_fired: bool,
    launch: Option<ChildLaunch>,
}

pub(crate) struct Scheduler {
    tasks: [Task; TASK_COUNT],
    initial_status: usize,
    goal: Option<Goal>,
    // Child launches and goals share a boot-unique handle sequence.
    next_handle: usize,
    next_event_sequence: usize,
    current: usize,
}

impl Scheduler {
    pub(crate) fn new(programs: [(AddressSpace, usize); TASK_COUNT], sstatus: usize) -> Self {
        let tasks = core::array::from_fn(|index| {
            let (address_space, entry) = programs[index];
            Task {
                entry,
                frame: TrapFrame::new_user(entry, USER_STACK_TOP, index + 1, sstatus),
                address_space,
                fs: if index == SHELL_TASK_INDEX {
                    TaskFs::at_root()
                } else {
                    TaskFs::empty()
                },
                state: if index == ECHO_TASK_INDEX {
                    TaskState::Stopped
                } else {
                    TaskState::Runnable
                },
                timer_fired: false,
                launch: None,
            }
        });
        Self {
            tasks,
            initial_status: sstatus,
            goal: None,
            next_handle: 1,
            next_event_sequence: 0,
            current: HELLO_TASK_INDEX,
        }
    }

    pub(crate) fn current_id(&self) -> usize {
        self.current + 1
    }

    pub(crate) fn current_frame(&self) -> &TrapFrame {
        &self.tasks[self.current].frame
    }

    pub(crate) fn address_space(&self) -> AddressSpace {
        self.tasks[self.current].address_space
    }

    pub(crate) fn timer_fired(&self) -> bool {
        self.tasks[self.current].timer_fired
    }

    pub(crate) fn task_fs(&mut self) -> &mut TaskFs {
        &mut self.tasks[self.current].fs
    }

    pub(crate) fn rmdir(&self, fs: &mut RamFs, path: &[u8]) -> Option<usize> {
        let live_task_fs = self
            .tasks
            .iter()
            .filter(|task| task.state != TaskState::Stopped)
            .map(|task| &task.fs);
        self.tasks[self.current].fs.rmdir(fs, path, live_task_fs)
    }

    pub(crate) fn spawn(
        &mut self,
        fs: &mut RamFs,
        program: usize,
        scope: LaunchScope,
        request: Option<Message>,
    ) -> Option<usize> {
        let index = match program {
            PROGRAM_HELLO => HELLO_TASK_INDEX,
            PROGRAM_ECHO => ECHO_TASK_INDEX,
            _ => return None,
        };
        if self.current != SHELL_TASK_INDEX || self.tasks[index].state != TaskState::Stopped {
            return None;
        }
        // The maximum value is reserved for syscall errors.
        let handle = self.next_handle;
        let next_handle = handle.checked_add(1)?;
        let mut frame = TrapFrame::new_user(
            self.tasks[index].entry,
            USER_STACK_TOP,
            index + 1,
            self.initial_status,
        );
        let grant = match scope {
            LaunchScope::Plain | LaunchScope::Goal(_) => None,
            LaunchScope::Directory(fd) => Some((GrantKind::Directory, fd, 0)),
            LaunchScope::File(fd) => Some((GrantKind::File, fd, 0)),
            LaunchScope::FileWithPipe(fd) => Some((GrantKind::File, fd, handle)),
        };
        let (task_fs, pipe) = match grant {
            Some((kind, fd, writer)) => {
                let (task_fs, child_fd) = self.tasks[self.current].fs.grant(fs, fd, kind)?;
                frame.set_startup_args(child_fd, kind as usize, writer);
                (task_fs, (writer != 0).then(Pipe::new))
            }
            None => (TaskFs::empty(), None),
        };

        if let LaunchScope::Goal(goal_handle) = scope {
            self.goal.as_mut()?.start(goal_handle, handle)?;
        }
        let task = &mut self.tasks[index];
        // SAFETY: This slot is stopped and its private stack frame is mapped
        // writable in the shared supervisor identity map.
        unsafe { task.address_space.clear_user_stack() };
        task.frame = frame;
        task.fs = task_fs;
        task.timer_fired = false;
        // Older children use an in-band status byte to answer a file read.
        let legacy_file_reply =
            matches!(scope, LaunchScope::File(_) | LaunchScope::FileWithPipe(_))
                || request.as_ref().is_some_and(|message| {
                    message.kind == MessageKind::Request(RequestKind::ReadFile)
                });
        task.launch = Some(ChildLaunch {
            handle,
            request,
            reply: None,
            legacy_file_reply,
            pipe,
            completion: None,
            event: None,
        });
        task.state = TaskState::Runnable;
        self.next_handle = next_handle;
        Some(handle)
    }

    pub(crate) fn wait_child(
        &mut self,
        handle: usize,
        frame: &mut TrapFrame,
    ) -> WaitResult<TaskCompletion> {
        let Some((launch, state)) = self.launch_for_shell(handle) else {
            return WaitResult::Unavailable;
        };
        if let Some(completion) = launch.completion.take() {
            return WaitResult::Ready(completion);
        }
        if *state == TaskState::Runnable
            && self.block_current(frame, TaskState::WaitingForChild(handle))
        {
            WaitResult::Blocked
        } else {
            WaitResult::Unavailable
        }
    }

    pub(crate) fn wait_event(&mut self, frame: &mut TrapFrame) -> WaitResult<ChildEvent> {
        if self.current != SHELL_TASK_INDEX {
            return WaitResult::Unavailable;
        }
        let oldest = self
            .tasks
            .iter_mut()
            .filter_map(|task| {
                let launch = task.launch.as_mut()?;
                let event = launch.event?;
                Some((launch, event))
            })
            .min_by_key(|(_, event)| event.sequence);
        if let Some((launch, event)) = oldest {
            launch.event = None;
            return WaitResult::Ready(ChildEvent::Completed {
                handle: launch.handle,
                kind: event.kind,
            });
        }
        let can_finish = self
            .tasks
            .iter()
            .any(|task| task.launch.is_some() && task.state == TaskState::Runnable);
        if can_finish && self.block_current(frame, TaskState::WaitingForEvent) {
            WaitResult::Blocked
        } else {
            WaitResult::Ready(ChildEvent::Empty)
        }
    }

    pub(crate) fn send_reply(
        &mut self,
        kind: Option<ReplyKind>,
        length: usize,
        copy: impl FnOnce(&mut [u8]) -> bool,
    ) -> Option<usize> {
        if self.tasks[SHELL_TASK_INDEX].state == TaskState::Stopped {
            return None;
        }

        let sender = self.current_id();
        let launch = self.tasks[self.current].launch.as_mut()?;
        if launch.reply.is_some() {
            return None;
        }
        let mut reply = Message::new(
            sender,
            MessageKind::Reply(kind.unwrap_or(ReplyKind::Echo)),
            length,
            copy,
        )?;
        if kind.is_none() && launch.legacy_file_reply {
            reply.decode_legacy_file_reply();
        }
        launch.reply = Some(reply);
        let handle = launch.handle;
        self.tasks[SHELL_TASK_INDEX]
            .state
            .wake_from(TaskState::WaitingForMessage(handle));
        Some(length)
    }

    pub(crate) fn receive_message(
        &mut self,
        mailbox: Mailbox,
        legacy: bool,
        frame: &mut TrapFrame,
        copy: impl FnOnce(&[u8]) -> bool,
    ) -> ReceiveResult {
        let pending = match mailbox {
            Mailbox::LaunchRequest => self.tasks[self.current]
                .launch
                .as_mut()
                .map(|launch| (&mut launch.request, None)),
            Mailbox::ChildReply(handle) => self
                .launch_for_shell(handle)
                .map(|(launch, state)| (&mut launch.reply, Some((handle, *state)))),
        };
        let Some((pending, wait_for)) = pending else {
            return ReceiveResult::Error;
        };
        if let Some(message) = pending.as_ref() {
            let bytes = message.bytes(legacy);
            if !copy(bytes) {
                return ReceiveResult::Error;
            }
            let result = ReceiveResult::Ready {
                length: bytes.len(),
                sender: message.sender,
                tag: message.kind.tag(),
            };
            *pending = None;
            return result;
        }
        match wait_for {
            Some((_, TaskState::WaitingForPipeSpace)) => ReceiveResult::Error,
            Some((handle, TaskState::Runnable))
                if self.block_current(frame, TaskState::WaitingForMessage(handle)) =>
            {
                ReceiveResult::Blocked
            }
            _ => ReceiveResult::Ready {
                length: 0,
                sender: 0,
                tag: 0,
            },
        }
    }

    pub(crate) fn preempt(&mut self, frame: &mut TrapFrame) -> bool {
        let task = &mut self.tasks[self.current];
        task.timer_fired = true;
        task.frame = *frame;
        self.switch(frame)
    }

    pub(crate) fn stop_current(
        &mut self,
        fs: &mut RamFs,
        frame: &mut TrapFrame,
        completion: TaskCompletion,
    ) -> bool {
        let shell_live = self.tasks[SHELL_TASK_INDEX].state != TaskState::Stopped;
        let task = &mut self.tasks[self.current];
        task.state = TaskState::Stopped;
        task.fs.close_all(fs);
        if let Some(launch) = &mut task.launch {
            launch.request = None;
            launch.completion = Some(completion);
            if let Some(goal) = &mut self.goal {
                goal.finish_child(launch.handle, completion);
            }
            if shell_live {
                launch.event = Some(PendingEvent {
                    sequence: self.next_event_sequence,
                    kind: completion.kind_and_value().0,
                });
                // Spawn checks for handle exhaustion. One event per launch keeps this count in range.
                self.next_event_sequence += 1;
            }
            if let Some(pipe) = &mut launch.pipe {
                pipe.close_writer();
            }
            let handle = launch.handle;
            let shell = &mut self.tasks[SHELL_TASK_INDEX];
            if shell.state.waits_for_result(handle)
                || shell.state == TaskState::WaitingForPipeData(handle)
            {
                shell.state = TaskState::Runnable;
            }
        } else if self.current == SHELL_TASK_INDEX {
            for task in &mut self.tasks {
                if let Some(launch) = &mut task.launch {
                    launch.reply = None;
                    launch.event = None;
                    launch.close_pipe_reader(&mut task.state);
                }
            }
        }
        if self.tasks[SHELL_TASK_INDEX].state == TaskState::Stopped {
            self.goal.take_if(|goal| !goal.is_running());
        }
        self.switch(frame)
    }

    fn launch_for_shell(&mut self, handle: usize) -> Option<(&mut ChildLaunch, &mut TaskState)> {
        if self.current != SHELL_TASK_INDEX {
            return None;
        }
        self.tasks.iter_mut().find_map(|task| {
            let launch = task.launch.as_mut()?;
            (launch.handle == handle).then_some((launch, &mut task.state))
        })
    }

    fn block_current(&mut self, frame: &mut TrapFrame, state: TaskState) -> bool {
        // Keep the ECALL's PC so the task retries its syscall after waking.
        let current = self.current;
        self.tasks[current].frame = *frame;
        if !self.switch(frame) {
            return false;
        }
        self.tasks[current].state = state;
        true
    }

    fn switch(&mut self, frame: &mut TrapFrame) -> bool {
        let Some(next) = (1..TASK_COUNT)
            .map(|offset| (self.current + offset) % TASK_COUNT)
            .find(|&index| self.tasks[index].state == TaskState::Runnable)
        else {
            return false;
        };

        self.current = next;
        *frame = self.tasks[next].frame;
        true
    }
}
