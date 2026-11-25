use super::{ChildLaunch, SHELL_TASK_INDEX, Scheduler, TaskState};
use crate::arch::riscv64::trap::TrapFrame;
use crate::ipc::pipe::{Pipe, PipeResult};

impl ChildLaunch {
    pub(super) fn close_pipe_reader(&mut self, child_state: &mut TaskState) -> bool {
        let closed = self.pipe.as_mut().is_some_and(Pipe::close_reader);
        if closed {
            child_state.wake_from(TaskState::WaitingForPipeSpace);
        }
        closed
    }
}

impl Scheduler {
    pub(crate) fn has_active_pipe_reader(&self) -> bool {
        self.tasks
            .iter()
            .filter_map(|task| task.launch.as_ref()?.pipe.as_ref())
            .any(Pipe::is_reading)
    }

    pub(crate) fn read_pipe(
        &mut self,
        handle: usize,
        capacity: usize,
        frame: &mut TrapFrame,
        copy: impl FnOnce(&[u8]) -> bool,
    ) -> PipeResult {
        let Some((launch, child_state)) = self.launch_for_shell(handle) else {
            return PipeResult::Error;
        };
        let Some(pipe) = launch.pipe.as_mut() else {
            return PipeResult::Error;
        };
        match pipe.read(capacity, copy) {
            PipeResult::Ready(count) if count > 0 => {
                child_state.wake_from(TaskState::WaitingForPipeSpace);
                PipeResult::Ready(count)
            }
            PipeResult::Wait => self.wait_for_pipe(frame, TaskState::WaitingForPipeData(handle)),
            result => result,
        }
    }

    pub(crate) fn write_pipe(
        &mut self,
        handle: usize,
        length: usize,
        frame: &mut TrapFrame,
        copy: impl FnOnce(&mut [u8]) -> bool,
    ) -> PipeResult {
        let Some(pipe) = self.writer_pipe(handle) else {
            return PipeResult::Error;
        };
        match pipe.write(length, copy) {
            PipeResult::Ready(count) if count > 0 => {
                self.tasks[SHELL_TASK_INDEX]
                    .state
                    .wake_from(TaskState::WaitingForPipeData(handle));
                PipeResult::Ready(count)
            }
            PipeResult::Wait => {
                // A parent waiting for this child's result must be able to drain or close the pipe.
                let shell = &mut self.tasks[SHELL_TASK_INDEX];
                if shell.state.waits_for_result(handle) {
                    shell.state = TaskState::Runnable;
                }
                self.wait_for_pipe(frame, TaskState::WaitingForPipeSpace)
            }
            result => result,
        }
    }

    pub(crate) fn close_pipe(&mut self, handle: usize) -> bool {
        if self.current == SHELL_TASK_INDEX {
            return self
                .launch_for_shell(handle)
                .is_some_and(|(launch, state)| launch.close_pipe_reader(state));
        }
        let closed = self.writer_pipe(handle).is_some_and(Pipe::close_writer);
        if closed {
            self.tasks[SHELL_TASK_INDEX]
                .state
                .wake_from(TaskState::WaitingForPipeData(handle));
        }
        closed
    }

    fn writer_pipe(&mut self, handle: usize) -> Option<&mut Pipe> {
        self.tasks[self.current]
            .launch
            .as_mut()
            .filter(|launch| launch.handle == handle)?
            .pipe
            .as_mut()
    }

    fn wait_for_pipe(&mut self, frame: &mut TrapFrame, state: TaskState) -> PipeResult {
        if self.block_current(frame, state) {
            PipeResult::Wait
        } else {
            PipeResult::Error
        }
    }
}
