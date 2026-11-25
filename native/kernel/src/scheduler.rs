use crate::arch::riscv64::trap::TrapFrame;
use crate::fs::ramfs::{RamFs, TaskFs};
use crate::memory::sv39::{AddressSpace, USER_STACK_TOP};

const HELLO_TASK_INDEX: usize = 0;

struct Task {
    frame: TrapFrame,
    address_space: AddressSpace,
    fs: TaskFs,
    runnable: bool,
    timer_fired: bool,
}

pub(crate) struct Scheduler {
    tasks: [Task; 2],
    hello_start: TrapFrame,
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
                runnable: true,
                timer_fired: false,
            }
        });
        Self {
            hello_start: tasks[HELLO_TASK_INDEX].frame,
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
            .filter(|task| task.runnable)
            .map(|task| &task.fs);
        fs.rmdir(&self.tasks[self.current].fs, path, live_task_fs)
    }

    pub(crate) fn spawn_hello(&mut self) -> Option<usize> {
        let task = &mut self.tasks[HELLO_TASK_INDEX];
        if task.runnable {
            return None;
        }

        // SAFETY: This slot is stopped and its private stack frame is mapped
        // writable in the shared supervisor identity map.
        unsafe { task.address_space.clear_user_stack() };
        task.frame = self.hello_start;
        task.fs = TaskFs::new();
        task.timer_fired = false;
        task.runnable = true;
        Some(0)
    }

    pub(crate) fn preempt(&mut self, frame: &mut TrapFrame) -> bool {
        let task = &mut self.tasks[self.current];
        task.timer_fired = true;
        task.frame = *frame;
        self.switch(frame)
    }

    pub(crate) fn stop_current(&mut self, fs: &mut RamFs, frame: &mut TrapFrame) -> bool {
        let task = &mut self.tasks[self.current];
        task.runnable = false;
        task.fs.close_all(fs);
        self.switch(frame)
    }

    fn switch(&mut self, frame: &mut TrapFrame) -> bool {
        let next = 1 - self.current;
        if !self.tasks[next].runnable {
            return false;
        }

        self.current = next;
        *frame = self.tasks[next].frame;
        true
    }
}
