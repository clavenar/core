use crate::memory::sv39::{AddressSpace, USER_STACK_TOP};

#[derive(Clone, Copy)]
#[repr(C, align(16))]
pub(crate) struct TrapFrame {
    pub(crate) registers: [usize; 32],
    pub(crate) sepc: usize,
    pub(crate) sstatus: usize,
}

struct Task {
    frame: TrapFrame,
    address_space: AddressSpace,
    runnable: bool,
    timer_fired: bool,
}

pub(crate) struct Scheduler {
    tasks: [Task; 2],
    current: usize,
}

impl Scheduler {
    pub(crate) fn new(spaces: [AddressSpace; 2], entry: usize, sstatus: usize) -> Self {
        Self {
            tasks: core::array::from_fn(|index| {
                let mut registers = [0; 32];
                registers[2] = USER_STACK_TOP;
                registers[10] = index + 1;
                Task {
                    frame: TrapFrame {
                        registers,
                        sepc: entry,
                        sstatus,
                    },
                    address_space: spaces[index],
                    runnable: true,
                    timer_fired: false,
                }
            }),
            current: 0,
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

    pub(crate) fn preempt(&mut self, frame: &mut TrapFrame) -> bool {
        let task = &mut self.tasks[self.current];
        task.timer_fired = true;
        task.frame = *frame;
        self.switch(frame)
    }

    pub(crate) fn stop_current(&mut self, frame: &mut TrapFrame) -> bool {
        self.tasks[self.current].runnable = false;
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
