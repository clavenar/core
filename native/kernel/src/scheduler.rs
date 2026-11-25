use clv_abi::MESSAGE_MAX_BYTES;

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

enum HelloRun {
    Untracked,
    Running,
    Completed(TaskCompletion),
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

pub(crate) enum HelloLaunch {
    Ordinary,
    Directory(usize),
    Request(Message),
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
    hello_run: HelloRun,
    hello_request: Option<Message>,
    hello_reply: Option<Message>,
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
            hello_run: HelloRun::Untracked,
            hello_request: None,
            hello_reply: None,
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

    pub(crate) fn spawn_hello(&mut self, fs: &mut RamFs, launch: HelloLaunch) -> Option<usize> {
        if self.current != SHELL_TASK_INDEX
            || self.tasks[HELLO_TASK_INDEX].state != TaskState::Stopped
        {
            return None;
        }
        let (task_fs, grant, request) = match launch {
            HelloLaunch::Ordinary => (TaskFs::new(), None, None),
            HelloLaunch::Directory(fd) => {
                let (task_fs, child_fd) = self.tasks[self.current].fs.grant_directory(fs, fd)?;
                (task_fs, Some(child_fd), None)
            }
            HelloLaunch::Request(message) => (TaskFs::new(), None, Some(message)),
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
        self.hello_request = request;
        self.hello_reply = None;
        self.hello_run = HelloRun::Running;
        task.state = TaskState::Runnable;
        Some(0)
    }

    pub(crate) fn wait_hello(&mut self, frame: &mut TrapFrame) -> WaitResult {
        if self.current != SHELL_TASK_INDEX {
            return WaitResult::NoLaunch;
        }
        match self.hello_run {
            HelloRun::Untracked => WaitResult::NoLaunch,
            HelloRun::Completed(completion) => {
                self.hello_run = HelloRun::Untracked;
                WaitResult::Completed(completion)
            }
            HelloRun::Running => {
                if self.block_shell(frame, TaskState::WaitingForHello) {
                    WaitResult::Blocked
                } else {
                    WaitResult::NoLaunch
                }
            }
        }
    }

    pub(crate) fn send_reply(
        &mut self,
        length: usize,
        copy: impl FnOnce(&mut [u8]) -> bool,
    ) -> Option<usize> {
        if self.current != HELLO_TASK_INDEX
            || !matches!(self.hello_run, HelloRun::Running)
            || self.tasks[SHELL_TASK_INDEX].state == TaskState::Stopped
            || self.hello_reply.is_some()
        {
            return None;
        }

        self.hello_reply = Some(Message::new(self.current_id(), length, copy)?);
        let shell = &mut self.tasks[SHELL_TASK_INDEX];
        if shell.state == TaskState::WaitingForMessage {
            shell.state = TaskState::Runnable;
        }
        Some(length)
    }

    pub(crate) fn receive_message(
        &mut self,
        frame: &mut TrapFrame,
        copy: impl FnOnce(&[u8]) -> bool,
    ) -> ReceiveResult {
        let pending = match self.current {
            SHELL_TASK_INDEX => &mut self.hello_reply,
            HELLO_TASK_INDEX if matches!(self.hello_run, HelloRun::Running) => {
                &mut self.hello_request
            }
            _ => return ReceiveResult::Error,
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
            && matches!(self.hello_run, HelloRun::Running)
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
        if self.current == HELLO_TASK_INDEX && matches!(self.hello_run, HelloRun::Running) {
            self.hello_request = None;
            self.hello_run = HelloRun::Completed(completion);
            let shell = &mut self.tasks[SHELL_TASK_INDEX];
            if matches!(
                shell.state,
                TaskState::WaitingForHello | TaskState::WaitingForMessage
            ) {
                shell.state = TaskState::Runnable;
            }
        } else if self.current == SHELL_TASK_INDEX {
            self.hello_reply = None;
        }
        self.switch(frame)
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
