use clv_abi::PipeStream;

use super::{ChildLaunch, Scheduler, TaskState};
use crate::arch::riscv64::trap::TrapFrame;
use crate::ipc::pipe::{Pipe, PipeResult};

#[derive(Default)]
pub(super) struct LaunchPipes {
    pub(super) stdin: Option<Pipe>,
    pub(super) stdout: Option<Pipe>,
}

impl LaunchPipes {
    pub(super) fn stdio() -> Self {
        Self {
            stdin: Some(Pipe::new()),
            stdout: Some(Pipe::new()),
        }
    }

    fn get_mut(&mut self, stream: PipeStream) -> Option<&mut Pipe> {
        match stream {
            PipeStream::Stdin => self.stdin.as_mut(),
            PipeStream::Stdout => self.stdout.as_mut(),
        }
    }

    fn close(&mut self, stream: PipeStream, reader: bool) -> bool {
        self.get_mut(stream).is_some_and(if reader {
            Pipe::close_reader
        } else {
            Pipe::close_writer
        })
    }
}

impl ChildLaunch {
    pub(super) fn close_parent_pipes(&mut self, child_state: &mut TaskState) {
        self.pipes.close(PipeStream::Stdin, false);
        self.pipes.close(PipeStream::Stdout, true);
        if child_state.waits_for_pipe(self.handle) {
            *child_state = TaskState::Runnable;
        }
    }

    pub(super) fn close_child_pipes(&mut self) {
        self.pipes.close(PipeStream::Stdin, true);
        self.pipes.close(PipeStream::Stdout, false);
    }
}

struct PipeAccess {
    child: usize,
    peer: Option<usize>,
    is_parent: bool,
}

impl Scheduler {
    pub(crate) fn has_active_pipe_reader(&self) -> bool {
        self.tasks
            .iter()
            .filter_map(|task| task.launch.as_ref()?.pipes.stdout.as_ref())
            .any(Pipe::is_reading)
    }

    pub(crate) fn read_pipe(
        &mut self,
        handle: usize,
        capacity: usize,
        frame: &mut TrapFrame,
        copy: impl FnOnce(&[u8]) -> bool,
    ) -> PipeResult {
        let Some(access) = self.pipe_access(handle) else {
            return PipeResult::Error;
        };
        let stream = if access.is_parent {
            PipeStream::Stdout
        } else {
            PipeStream::Stdin
        };
        let Some(pipe) = self.pipes(access.child).get_mut(stream) else {
            return PipeResult::Error;
        };
        let result = pipe.read(capacity, copy);
        match result {
            PipeResult::Ready(count) if count > 0 => {
                self.wake_pipe_peer(access.peer, TaskState::WaitingForPipeSpace(handle));
                result
            }
            PipeResult::Wait => {
                self.wait_for_pipe(frame, access, handle, TaskState::WaitingForPipeData(handle))
            }
            _ => result,
        }
    }

    pub(crate) fn write_pipe(
        &mut self,
        handle: usize,
        length: usize,
        frame: &mut TrapFrame,
        copy: impl FnOnce(&mut [u8]) -> bool,
    ) -> PipeResult {
        let Some(access) = self.pipe_access(handle) else {
            return PipeResult::Error;
        };
        let stream = if access.is_parent {
            PipeStream::Stdin
        } else {
            PipeStream::Stdout
        };
        let launch = self.tasks[access.child].launch.as_mut().unwrap();
        let Some(pipe) = launch.pipes.get_mut(stream) else {
            return PipeResult::Error;
        };
        let goal = &mut self.goal;
        // The copied prefix becomes visible only after the budget check succeeds.
        let result = pipe.write(length, |bytes| {
            copy(bytes)
                && (access.is_parent
                    || goal
                        .as_mut()
                        .is_none_or(|goal| goal.charge_reply(handle, bytes.len()).is_some()))
        });
        match result {
            PipeResult::Ready(count) if count > 0 => {
                self.wake_pipe_peer(access.peer, TaskState::WaitingForPipeData(handle));
                result
            }
            PipeResult::Wait => self.wait_for_pipe(
                frame,
                access,
                handle,
                TaskState::WaitingForPipeSpace(handle),
            ),
            _ => result,
        }
    }

    pub(crate) fn close_pipe(&mut self, handle: usize, stream: PipeStream) -> bool {
        let Some(access) = self.pipe_access(handle) else {
            return false;
        };
        let reader = match stream {
            PipeStream::Stdout => access.is_parent,
            PipeStream::Stdin => !access.is_parent,
        };
        if !self.pipes(access.child).close(stream, reader) {
            return false;
        }
        let wait = if reader {
            TaskState::WaitingForPipeSpace(handle)
        } else {
            TaskState::WaitingForPipeData(handle)
        };
        self.wake_pipe_peer(access.peer, wait);
        true
    }

    fn pipe_access(&self, handle: usize) -> Option<PipeAccess> {
        let child = self.launch_index(handle)?;
        let parent = self.live_parent(self.tasks[child].launch.as_ref()?.parent);
        if self.current == child {
            Some(PipeAccess {
                child,
                peer: parent,
                is_parent: false,
            })
        } else if parent == Some(self.current) {
            Some(PipeAccess {
                child,
                peer: Some(child),
                is_parent: true,
            })
        } else {
            None
        }
    }

    fn pipes(&mut self, child: usize) -> &mut LaunchPipes {
        &mut self.tasks[child].launch.as_mut().unwrap().pipes
    }

    fn wake_pipe_peer(&mut self, peer: Option<usize>, state: TaskState) {
        if let Some(peer) = peer {
            self.tasks[peer].state.wake_from(state);
        }
    }

    fn wait_for_pipe(
        &mut self,
        frame: &mut TrapFrame,
        access: PipeAccess,
        handle: usize,
        wait: TaskState,
    ) -> PipeResult {
        let Some(peer) = access.peer else {
            return PipeResult::Error;
        };
        let state = &mut self.tasks[peer].state;
        // Blocking here would leave both ends of this launch waiting.
        if state.waits_for_pipe(handle) {
            return PipeResult::Error;
        }
        // Let a waiting parent service this pipe before collecting results.
        if !access.is_parent && state.waits_for_result(handle) {
            *state = TaskState::Runnable;
        }
        if self.block_current(frame, wait) {
            PipeResult::Wait
        } else {
            PipeResult::Error
        }
    }
}
