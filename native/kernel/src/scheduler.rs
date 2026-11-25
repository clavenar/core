use crate::arch::riscv64::trap::TrapFrame;
use crate::fs::ramfs::{RamFs, TaskFs};
use crate::memory::sv39::{AddressSpace, USER_STACK_TOP};

const HELLO_TASK_INDEX: usize = 0;
const SHELL_TASK_INDEX: usize = 1;

#[derive(PartialEq, Eq)]
enum TaskState {
    Runnable,
    WaitingForHello,
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

    pub(crate) fn spawn_hello(&mut self) -> Option<usize> {
        let task = &mut self.tasks[HELLO_TASK_INDEX];
        if task.state != TaskState::Stopped {
            return None;
        }

        // SAFETY: This slot is stopped and its private stack frame is mapped
        // writable in the shared supervisor identity map.
        unsafe { task.address_space.clear_user_stack() };
        task.frame = self.hello_start;
        task.fs = TaskFs::new();
        task.timer_fired = false;
        task.state = TaskState::Runnable;
        self.hello_run = HelloRun::Running;
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
                // Retry the ECALL after the child completes.
                self.tasks[SHELL_TASK_INDEX].frame = *frame;
                if self.switch(frame) {
                    self.tasks[SHELL_TASK_INDEX].state = TaskState::WaitingForHello;
                    WaitResult::Blocked
                } else {
                    WaitResult::NoLaunch
                }
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
        if self.current == HELLO_TASK_INDEX && matches!(self.hello_run, HelloRun::Running) {
            self.hello_run = HelloRun::Completed(completion);
            let shell = &mut self.tasks[SHELL_TASK_INDEX];
            if shell.state == TaskState::WaitingForHello {
                shell.state = TaskState::Runnable;
            }
        }
        self.switch(frame)
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
