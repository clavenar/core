use clv_abi::{MESSAGE_MAX_BYTES, NO_CHILD_HANDLE, SYSCALL_ERROR};

use crate::arch::riscv64::trap::TrapFrame;
use crate::fs::ramfs::{RamFs, TaskFs};
use crate::memory::sv39::{AddressSpace, USER_STACK_TOP};

const HELLO_TASK_INDEX: usize = 0;
const SHELL_TASK_INDEX: usize = 1;

#[derive(PartialEq, Eq)]
enum TaskState {
    Runnable,
    WaitingForHello,
    WaitingForMessage,
    Stopped,
}

#[derive(Clone, Copy)]
pub(crate) enum TaskCompletion {
    Exited(usize),
    Faulted(usize),
}

struct HelloLaunch {
    handle: usize,
    request: Option<Message>,
    reply: Option<Message>,
    completion: Option<TaskCompletion>,
}

pub(crate) enum WaitResult {
    NoLaunch,
    Blocked,
    Completed(TaskCompletion),
}

pub(crate) enum ReceiveResult {
    Error,
    Blocked,
    Empty,
    Received { length: usize, sender: usize },
}

pub(crate) struct Message {
    bytes: [u8; MESSAGE_MAX_BYTES],
    length: usize,
    sender: usize,
}

impl Message {
    pub(crate) fn new(
        sender: usize,
        length: usize,
        copy: impl FnOnce(&mut [u8]) -> bool,
    ) -> Option<Self> {
        if length == 0 {
            return None;
        }
        let mut bytes = [0; MESSAGE_MAX_BYTES];
        if !copy(bytes.get_mut(..length)?) {
            return None;
        }
        Some(Self {
            bytes,
            length,
            sender,
        })
    }
}

struct Task {
    frame: TrapFrame,
    address_space: AddressSpace,
    fs: TaskFs,
    state: TaskState,
    timer_fired: bool,
}

pub(crate) struct Scheduler {
    tasks: [Task; 2],
    hello_start: TrapFrame,
    hello_launch: Option<HelloLaunch>,
    current: usize,
}

impl Scheduler {
    pub(crate) fn new(programs: [(AddressSpace, usize); 2], sstatus: usize) -> Self {
        let tasks = core::array::from_fn(|index| {
            let (address_space, entry) = programs[index];
            Task {
                frame: TrapFrame::new_user(entry, USER_STACK_TOP, index + 1, sstatus),
                address_space,
                fs: TaskFs::new(),
                state: TaskState::Runnable,
                timer_fired: false,
            }
        });
        Self {
            hello_start: tasks[HELLO_TASK_INDEX].frame,
            hello_launch: None,
            tasks,
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
        fs.rmdir(&self.tasks[self.current].fs, path, live_task_fs)
    }

    pub(crate) fn spawn_hello(
        &mut self,
        fs: &mut RamFs,
        directory_fd: Option<usize>,
        request: Option<Message>,
    ) -> Option<usize> {
        if self.current != SHELL_TASK_INDEX
            || self.tasks[HELLO_TASK_INDEX].state != TaskState::Stopped
        {
            return None;
        }
        // Keep the previous handle after the shell has collected its status and
        // reply so this slot never reuses a launch handle during the same boot.
        let previous = self
            .hello_launch
            .as_ref()
            .map_or(NO_CHILD_HANDLE, |launch| launch.handle);
        let handle = previous
            .checked_add(1)
            .filter(|&handle| handle != SYSCALL_ERROR)?;
        let (task_fs, grant) = match directory_fd {
            Some(fd) => {
                let (task_fs, child_fd) = self.tasks[self.current].fs.grant_directory(fs, fd)?;
                (task_fs, Some(child_fd))
            }
            None => (TaskFs::new(), None),
        };

        let task = &mut self.tasks[HELLO_TASK_INDEX];
        // SAFETY: This slot is stopped and its private stack frame is mapped
        // writable in the shared supervisor identity map.
        unsafe { task.address_space.clear_user_stack() };
        task.frame = self.hello_start;
        if let Some(fd) = grant {
            task.frame.set_startup_args(fd, 1);
        }
        task.fs = task_fs;
        task.timer_fired = false;
        self.hello_launch = Some(HelloLaunch {
            handle,
            request,
            reply: None,
            completion: None,
        });
        task.state = TaskState::Runnable;
        Some(handle)
    }

    pub(crate) fn wait_hello(&mut self, handle: usize, frame: &mut TrapFrame) -> WaitResult {
        let Some(launch) = self.hello_for_shell(handle) else {
            return WaitResult::NoLaunch;
        };
        if let Some(completion) = launch.completion.take() {
            return WaitResult::Completed(completion);
        }
        if self.tasks[HELLO_TASK_INDEX].state == TaskState::Runnable
            && self.block_shell(frame, TaskState::WaitingForHello)
        {
            WaitResult::Blocked
        } else {
            WaitResult::NoLaunch
        }
    }

    pub(crate) fn send_reply(
        &mut self,
        length: usize,
        copy: impl FnOnce(&mut [u8]) -> bool,
    ) -> Option<usize> {
        if self.current != HELLO_TASK_INDEX
            || self.tasks[SHELL_TASK_INDEX].state == TaskState::Stopped
        {
            return None;
        }

        let sender = self.current_id();
        let launch = self.hello_launch.as_mut()?;
        if launch.reply.is_some() {
            return None;
        }
        launch.reply = Some(Message::new(sender, length, copy)?);
        let shell = &mut self.tasks[SHELL_TASK_INDEX];
        if shell.state == TaskState::WaitingForMessage {
            shell.state = TaskState::Runnable;
        }
        Some(length)
    }

    pub(crate) fn receive_message(
        &mut self,
        handle: usize,
        frame: &mut TrapFrame,
        copy: impl FnOnce(&[u8]) -> bool,
    ) -> ReceiveResult {
        let pending = match self.current {
            SHELL_TASK_INDEX => self.hello_for_shell(handle).map(|launch| &mut launch.reply),
            HELLO_TASK_INDEX if handle == NO_CHILD_HANDLE => {
                self.hello_launch.as_mut().map(|launch| &mut launch.request)
            }
            _ => None,
        };
        let Some(pending) = pending else {
            return ReceiveResult::Error;
        };
        if let Some(message) = pending.as_ref() {
            if !copy(&message.bytes[..message.length]) {
                return ReceiveResult::Error;
            }
            let result = ReceiveResult::Received {
                length: message.length,
                sender: message.sender,
            };
            *pending = None;
            return result;
        }
        if self.current == SHELL_TASK_INDEX
            && self.tasks[HELLO_TASK_INDEX].state == TaskState::Runnable
            && self.block_shell(frame, TaskState::WaitingForMessage)
        {
            ReceiveResult::Blocked
        } else {
            ReceiveResult::Empty
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
        let task = &mut self.tasks[self.current];
        task.state = TaskState::Stopped;
        task.fs.close_all(fs);
        if let Some(launch) = &mut self.hello_launch {
            if self.current == HELLO_TASK_INDEX {
                launch.request = None;
                launch.completion = Some(completion);
                let shell = &mut self.tasks[SHELL_TASK_INDEX];
                if matches!(
                    shell.state,
                    TaskState::WaitingForHello | TaskState::WaitingForMessage
                ) {
                    shell.state = TaskState::Runnable;
                }
            } else {
                launch.reply = None;
            }
        }
        self.switch(frame)
    }

    fn hello_for_shell(&mut self, handle: usize) -> Option<&mut HelloLaunch> {
        if self.current != SHELL_TASK_INDEX {
            return None;
        }
        self.hello_launch
            .as_mut()
            .filter(|launch| launch.handle == handle)
    }

    fn block_shell(&mut self, frame: &mut TrapFrame, state: TaskState) -> bool {
        // Keep the ECALL's PC so the shell retries its syscall after waking.
        self.tasks[SHELL_TASK_INDEX].frame = *frame;
        if !self.switch(frame) {
            return false;
        }
        self.tasks[SHELL_TASK_INDEX].state = state;
        true
    }

    fn switch(&mut self, frame: &mut TrapFrame) -> bool {
        let next = 1 - self.current;
        if self.tasks[next].state != TaskState::Runnable {
            return false;
        }

        self.current = next;
        *frame = self.tasks[next].frame;
        true
    }
}
