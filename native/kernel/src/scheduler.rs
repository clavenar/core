mod goal;
mod pipe;

use self::goal::Goal;
use clv_abi::agent::AgentLaunch;
use clv_abi::audit::DelegationDenial;
use clv_abi::{
    ChildEvent, CompletionKind, GrantKind, Program, ReplyKind, RequestKind, TaskCompletion,
};

use crate::arch::riscv64::trap::TrapFrame;
use crate::fs::ramfs::{RamFs, TaskFs};
use crate::ipc::pipe::Pipe;
use crate::ipc::{Message, MessageKind};
use crate::memory::sv39::AddressSpace;
use crate::programs::{SHELL_TASK_INDEX, TASK_COUNT, task_index};

#[derive(Clone, Copy, PartialEq, Eq)]
enum TaskState {
    Runnable,
    Suspended,
    WaitingForChild(usize),
    WaitingForEvent,
    WaitingForMessage(usize),
    WaitingForPipeData(usize),
    WaitingForPipeSpace,
    Stopped,
}

impl TaskState {
    fn for_agent(suspended: bool) -> Self {
        if suspended {
            Self::Suspended
        } else {
            Self::Runnable
        }
    }

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

    // An agent blocked on its helper can still finish while its own parent waits.
    fn can_finish(self) -> bool {
        matches!(
            self,
            Self::Runnable | Self::WaitingForChild(_) | Self::WaitingForMessage(_)
        )
    }
}

#[derive(Clone, Copy)]
struct PendingEvent {
    sequence: usize,
    kind: CompletionKind,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum LaunchParent {
    Shell,
    /// The agent's original child launch handle, independent of program slot reuse.
    Agent(usize),
}

struct ChildLaunch {
    handle: usize,
    parent: LaunchParent,
    request: Option<Message>,
    reply: Option<Message>,
    legacy_file_reply: bool,
    pipe: Option<Pipe>,
    completion: Option<TaskCompletion>,
    event: Option<PendingEvent>,
}

struct PreparedLaunch {
    program: Program,
    handle: usize,
    next_handle: usize,
    parent: LaunchParent,
}

#[derive(Clone, Copy)]
pub(crate) enum LaunchScope {
    Plain,
    Directory(usize),
    File(usize),
    FileWithPipe(usize),
    Goal {
        handle: usize,
        reply_bytes: Option<usize>,
    },
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

impl Task {
    fn boot(index: usize, address_space: AddressSpace, entry: usize, sstatus: usize) -> Self {
        Self {
            entry,
            frame: TrapFrame::new_user(entry, address_space.stack.top(), index + 1, sstatus),
            address_space,
            fs: if index == SHELL_TASK_INDEX {
                TaskFs::at_root()
            } else {
                TaskFs::empty()
            },
            state: if index == SHELL_TASK_INDEX || index == task_index(Program::Hello) {
                TaskState::Runnable
            } else {
                TaskState::Stopped
            },
            timer_fired: false,
            launch: None,
        }
    }

    fn has_launch(&self, handle: usize) -> bool {
        self.launch
            .as_ref()
            .is_some_and(|launch| launch.handle == handle)
    }
}

pub(crate) struct Scheduler {
    tasks: [Task; TASK_COUNT],
    initial_status: usize,
    goal: Option<Goal>,
    // Child launches, goals and agents share a boot-unique handle sequence.
    next_handle: usize,
    next_event_sequence: usize,
    current: usize,
}

impl Scheduler {
    pub(crate) fn new(programs: [(AddressSpace, usize); TASK_COUNT], sstatus: usize) -> Self {
        Self {
            tasks: core::array::from_fn(|index| {
                let (space, entry) = programs[index];
                Task::boot(index, space, entry, sstatus)
            }),
            initial_status: sstatus,
            goal: None,
            next_handle: 1,
            next_event_sequence: 0,
            current: task_index(Program::Hello),
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
        self.start_child(fs, program, scope, request, None, TaskState::Runnable)
    }

    pub(crate) fn run_agent(
        &mut self,
        fs: &mut RamFs,
        goal: usize,
        program: usize,
        suspended: bool,
        reply_bytes: Option<usize>,
        request: Option<Message>,
    ) -> Option<AgentLaunch> {
        let agent = self.next_handle;
        let child = self.start_child(
            fs,
            program,
            LaunchScope::Goal {
                handle: goal,
                reply_bytes,
            },
            request,
            Some(agent),
            TaskState::for_agent(suspended),
        )?;
        Some(AgentLaunch { agent, child })
    }

    fn prepare_launch(
        &self,
        program: Program,
        agent: Option<usize>,
        parent: LaunchParent,
    ) -> Result<PreparedLaunch, DelegationDenial> {
        let task = &self.tasks[task_index(program)];
        // Relaunching can discard one's own results, but never another live parent's.
        let other_results = task.launch.as_ref().is_some_and(|previous| {
            previous.parent != parent
                && (previous.reply.is_some()
                    || previous.completion.is_some()
                    || previous.event.is_some()
                    || previous.pipe.as_ref().is_some_and(Pipe::reader_open))
                && self.live_parent(previous.parent).is_some()
        });
        if task.state != TaskState::Stopped || other_results {
            return Err(DelegationDenial::ProgramBusy);
        }
        // An agent takes the first handle of the pair. Commit the sequence only on success.
        let handle = match agent {
            Some(agent) => agent
                .checked_add(1)
                .ok_or(DelegationDenial::LaunchHandlesExhausted)?,
            None => self.next_handle,
        };
        let next_handle = handle
            .checked_add(1)
            .ok_or(DelegationDenial::LaunchHandlesExhausted)?;
        Ok(PreparedLaunch {
            program,
            handle,
            next_handle,
            parent,
        })
    }

    fn start_child(
        &mut self,
        fs: &mut RamFs,
        program: usize,
        scope: LaunchScope,
        request: Option<Message>,
        agent: Option<usize>,
        initial_state: TaskState,
    ) -> Option<usize> {
        if self.current != SHELL_TASK_INDEX {
            return None;
        }
        let launch = self
            .prepare_launch(Program::from_raw(program)?, agent, LaunchParent::Shell)
            .ok()?;
        let task_fs = match scope {
            LaunchScope::Plain => TaskFs::empty(),
            LaunchScope::Goal {
                handle: goal,
                reply_bytes,
            } => self
                .goal
                .as_mut()?
                .start(goal, launch.handle, agent, reply_bytes, fs)?,
            LaunchScope::Directory(fd) => {
                self.tasks[self.current]
                    .fs
                    .grant(fs, fd, GrantKind::Directory)?
            }
            LaunchScope::File(fd) | LaunchScope::FileWithPipe(fd) => {
                self.tasks[self.current].fs.grant(fs, fd, GrantKind::File)?
            }
        };
        let pipe = matches!(scope, LaunchScope::FileWithPipe(_)).then(Pipe::new);
        Some(self.install_child(launch, task_fs, request, pipe, initial_state))
    }

    fn install_child(
        &mut self,
        launch: PreparedLaunch,
        task_fs: TaskFs,
        request: Option<Message>,
        pipe: Option<Pipe>,
        initial_state: TaskState,
    ) -> usize {
        let PreparedLaunch {
            program,
            handle,
            next_handle,
            parent,
        } = launch;
        let index = task_index(program);
        let task = &mut self.tasks[index];
        let mut frame = TrapFrame::new_user(
            task.entry,
            task.address_space.stack.top(),
            index + 1,
            self.initial_status,
        );
        let grant = task_fs.startup_grant();
        if let Some((fd, kind)) = grant {
            let writer = if pipe.is_some() { handle } else { 0 };
            frame.set_startup_args(fd, kind as usize, writer);
        }
        // SAFETY: This slot is stopped and its private stack frame is mapped
        // writable in the shared supervisor identity map.
        unsafe { task.address_space.clear_user_stack() };
        task.frame = frame;
        task.fs = task_fs;
        task.timer_fired = false;
        // Older children use an in-band status byte to answer a file read.
        let legacy_file_reply = matches!(grant, Some((_, GrantKind::File)))
            || request
                .as_ref()
                .is_some_and(|message| message.kind == MessageKind::Request(RequestKind::ReadFile));
        task.launch = Some(ChildLaunch {
            handle,
            parent,
            request,
            reply: None,
            legacy_file_reply,
            pipe,
            completion: None,
            event: None,
        });
        task.state = initial_state;
        self.next_handle = next_handle;
        handle
    }

    pub(crate) fn wait_child(
        &mut self,
        handle: usize,
        frame: &mut TrapFrame,
    ) -> WaitResult<TaskCompletion> {
        let Some((launch, state)) = self.launch_for_parent(handle) else {
            return WaitResult::Unavailable;
        };
        if let Some(completion) = launch.completion.take() {
            return WaitResult::Ready(completion);
        }
        if state.can_finish() && self.block_current(frame, TaskState::WaitingForChild(handle)) {
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
        let can_finish = self.tasks.iter().any(|task| {
            task.state.can_finish()
                && task
                    .launch
                    .as_ref()
                    .is_some_and(|launch| launch.parent == LaunchParent::Shell)
        });
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
        let parent = self.live_parent(self.tasks[self.current].launch.as_ref()?.parent)?;
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
        if let Some(goal) = &mut self.goal {
            goal.charge_reply(launch.handle, length)?;
        }
        launch.reply = Some(reply);
        let handle = launch.handle;
        self.tasks[parent]
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
                .launch_for_parent(handle)
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
            Some((handle, state))
                if state.can_finish()
                    && self.block_current(frame, TaskState::WaitingForMessage(handle)) =>
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
        self.stop_task(self.current, fs, completion);
        if self.current == SHELL_TASK_INDEX {
            self.release_shell_goal(fs);
            for task in &mut self.tasks {
                if let Some(launch) = &mut task.launch {
                    launch.reply = None;
                    launch.event = None;
                    launch.close_pipe_reader(&mut task.state);
                }
            }
        } else if let Some(launch) = &self.tasks[self.current].launch {
            self.cancel_delegated_helper(launch.handle, fs);
        }
        self.switch(frame)
    }

    fn stop_task(&mut self, index: usize, fs: &mut RamFs, completion: TaskCompletion) {
        let shell_live = self.tasks[SHELL_TASK_INDEX].state != TaskState::Stopped;
        let task = &mut self.tasks[index];
        task.state = TaskState::Stopped;
        task.fs.close_all(fs);
        if let Some(launch) = &mut task.launch {
            launch.request = None;
            launch.completion = Some(completion);
            if let Some(goal) = &mut self.goal {
                goal.finish_child(launch.handle, completion, fs);
            }
            if shell_live && launch.parent == LaunchParent::Shell {
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
            let owner = launch.parent;
            if let Some(parent) = self.live_parent(owner) {
                let state = &mut self.tasks[parent].state;
                if state.waits_for_result(handle) || *state == TaskState::WaitingForPipeData(handle)
                {
                    *state = TaskState::Runnable;
                }
            }
        }
    }

    fn launch_index(&self, handle: usize) -> Option<usize> {
        self.tasks.iter().position(|task| task.has_launch(handle))
    }

    fn live_parent(&self, parent: LaunchParent) -> Option<usize> {
        let index = match parent {
            LaunchParent::Shell => SHELL_TASK_INDEX,
            LaunchParent::Agent(handle) => self.launch_index(handle)?,
        };
        (self.tasks[index].state != TaskState::Stopped).then_some(index)
    }

    fn launch_for_parent(&mut self, handle: usize) -> Option<(&mut ChildLaunch, &mut TaskState)> {
        let index = self.launch_index(handle)?;
        if self.live_parent(self.tasks[index].launch.as_ref()?.parent) != Some(self.current) {
            return None;
        }
        let task = &mut self.tasks[index];
        Some((task.launch.as_mut()?, &mut task.state))
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
