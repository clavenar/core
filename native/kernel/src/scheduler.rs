use clv_abi::{
    GrantKind, MESSAGE_MAX_BYTES, NO_CHILD_HANDLE, PROGRAM_ECHO, PROGRAM_HELLO, RequestKind,
};

use crate::arch::riscv64::trap::TrapFrame;
use crate::fs::ramfs::{RamFs, TaskFs};
use crate::memory::sv39::{AddressSpace, USER_STACK_TOP};

pub(crate) const TASK_COUNT: usize = 3;
const HELLO_TASK_INDEX: usize = 0;
const SHELL_TASK_INDEX: usize = 1;
const ECHO_TASK_INDEX: usize = 2;

#[derive(Clone, Copy, PartialEq, Eq)]
enum TaskState {
    Runnable,
    WaitingForChild(usize),
    WaitingForMessage(usize),
    Stopped,
}

#[derive(Clone, Copy)]
pub(crate) enum TaskCompletion {
    Exited(usize),
    Faulted(usize),
}

struct ChildLaunch {
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
    Ready {
        length: usize,
        sender: usize,
        request_kind: Option<RequestKind>,
    },
}

pub(crate) struct Message {
    bytes: [u8; MESSAGE_MAX_BYTES],
    length: usize,
    sender: usize,
    request_kind: Option<RequestKind>,
}

impl Message {
    pub(crate) fn new(
        sender: usize,
        request_kind: Option<RequestKind>,
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
            request_kind,
        })
    }
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
    next_child_handle: usize,
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
            next_child_handle: 1,
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
        grant: Option<(GrantKind, usize)>,
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
        let handle = self.next_child_handle;
        let next_handle = handle.checked_add(1)?;
        let mut frame = TrapFrame::new_user(
            self.tasks[index].entry,
            USER_STACK_TOP,
            index + 1,
            self.initial_status,
        );
        let task_fs = match grant {
            Some((kind, fd)) => {
                let (task_fs, child_fd) = self.tasks[self.current].fs.grant(fs, fd, kind)?;
                frame.set_startup_args(child_fd, kind as usize);
                task_fs
            }
            None => TaskFs::empty(),
        };

        let task = &mut self.tasks[index];
        // SAFETY: This slot is stopped and its private stack frame is mapped
        // writable in the shared supervisor identity map.
        unsafe { task.address_space.clear_user_stack() };
        task.frame = frame;
        task.fs = task_fs;
        task.timer_fired = false;
        task.launch = Some(ChildLaunch {
            handle,
            request,
            reply: None,
            completion: None,
        });
        task.state = TaskState::Runnable;
        self.next_child_handle = next_handle;
        Some(handle)
    }

    pub(crate) fn wait_child(&mut self, handle: usize, frame: &mut TrapFrame) -> WaitResult {
        let Some((launch, state)) = self.launch_for_shell(handle) else {
            return WaitResult::NoLaunch;
        };
        if let Some(completion) = launch.completion.take() {
            return WaitResult::Completed(completion);
        }
        if state == TaskState::Runnable
            && self.block_shell(frame, TaskState::WaitingForChild(handle))
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
        if self.tasks[SHELL_TASK_INDEX].state == TaskState::Stopped {
            return None;
        }

        let sender = self.current_id();
        let launch = self.tasks[self.current].launch.as_mut()?;
        if launch.reply.is_some() {
            return None;
        }
        launch.reply = Some(Message::new(sender, None, length, copy)?);
        let handle = launch.handle;
        let shell = &mut self.tasks[SHELL_TASK_INDEX];
        if shell.state == TaskState::WaitingForMessage(handle) {
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
        let mailbox = match self.current {
            SHELL_TASK_INDEX => self
                .launch_for_shell(handle)
                .map(|(launch, state)| (&mut launch.reply, state == TaskState::Runnable)),
            index if handle == NO_CHILD_HANDLE => self.tasks[index]
                .launch
                .as_mut()
                .map(|launch| (&mut launch.request, false)),
            _ => None,
        };
        let Some((pending, child_running)) = mailbox else {
            return ReceiveResult::Error;
        };
        if let Some(message) = pending.as_ref() {
            if !copy(&message.bytes[..message.length]) {
                return ReceiveResult::Error;
            }
            let result = ReceiveResult::Ready {
                length: message.length,
                sender: message.sender,
                request_kind: message.request_kind,
            };
            *pending = None;
            return result;
        }
        if child_running && self.block_shell(frame, TaskState::WaitingForMessage(handle)) {
            ReceiveResult::Blocked
        } else {
            ReceiveResult::Ready {
                length: 0,
                sender: 0,
                request_kind: None,
            }
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
        if let Some(launch) = &mut task.launch {
            launch.request = None;
            launch.completion = Some(completion);
            let handle = launch.handle;
            let shell = &mut self.tasks[SHELL_TASK_INDEX];
            if shell.state == TaskState::WaitingForChild(handle)
                || shell.state == TaskState::WaitingForMessage(handle)
            {
                shell.state = TaskState::Runnable;
            }
        } else if self.current == SHELL_TASK_INDEX {
            for launch in self
                .tasks
                .iter_mut()
                .filter_map(|task| task.launch.as_mut())
            {
                launch.reply = None;
            }
        }
        self.switch(frame)
    }

    fn launch_for_shell(&mut self, handle: usize) -> Option<(&mut ChildLaunch, TaskState)> {
        if self.current != SHELL_TASK_INDEX {
            return None;
        }
        self.tasks.iter_mut().find_map(|task| {
            let launch = task.launch.as_mut()?;
            (launch.handle == handle).then_some((launch, task.state))
        })
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
