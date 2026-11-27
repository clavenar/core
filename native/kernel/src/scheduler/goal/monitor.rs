use clv_abi::audit::{PublicationObservation, ReadDenialEvent, ReadDenialSnapshot};
use clv_abi::goal::{GoalMonitorKind, GoalObservation, MAX_READ_DENIAL_THRESHOLD};
use clv_abi::{LaunchParent, Program, ReplyKind, RequestKind, TaskCompletion};

use crate::arch::riscv64::trap::TrapFrame;
use crate::fs::ramfs::{RamFs, TaskFs};
use crate::ipc::{Message, MessageKind};
use crate::programs::{GOAL_MONITOR_TASK_INDEX, SHELL_TASK_INDEX};
use crate::scheduler::{LaunchGrant, Scheduler, TaskState, WaitResult};

#[derive(Clone, Copy)]
pub(in crate::scheduler) enum MonitorReport {
    Goal(GoalObservation),
    Publication(PublicationObservation),
}

pub(in crate::scheduler) struct ReadDenialWatch {
    goal: usize,
    threshold: u32,
    first: Option<ReadDenialEvent>,
    denials: u32,
    last_sample: u32,
}

impl ReadDenialWatch {
    fn threshold_met(&self) -> bool {
        self.denials >= self.threshold
    }
}

impl Scheduler {
    pub(crate) fn launch_goal_monitor(
        &mut self,
        goal: usize,
        kind: GoalMonitorKind,
        threshold: usize,
    ) -> Option<usize> {
        let watched_goal = self.shell_goal(goal)?;
        let threshold = threshold.max(1);
        if kind == GoalMonitorKind::ReadDenial
            && (!watched_goal.is_active() || threshold > MAX_READ_DENIAL_THRESHOLD)
        {
            return None;
        }
        // The owner can collect across goal closure, in either reply/status order.
        if self.tasks[GOAL_MONITOR_TASK_INDEX]
            .launch
            .as_ref()
            .is_some_and(|previous| previous.reply.is_some() || previous.completion.is_some())
        {
            return None;
        }
        let launch = self
            .prepare_launch(
                Program::Echo,
                None,
                LaunchParent::Shell,
                Some(LaunchGrant::Monitor(kind)),
            )
            .ok()?;
        let threshold_bytes = [threshold as u8];
        let (request_kind, payload): (RequestKind, &[u8]) = match kind {
            GoalMonitorKind::Finish => (RequestKind::ObserveGoal, &[]),
            GoalMonitorKind::Publication => (RequestKind::ObservePublication, &[]),
            GoalMonitorKind::ReadDenial => (RequestKind::WatchReadDenials, &threshold_bytes),
        };
        let request = Message::new(
            self.current_id(),
            MessageKind::Request(request_kind),
            payload.len(),
            |request| {
                request.copy_from_slice(payload);
                true
            },
        )?;
        let handle = self.install_child(
            launch,
            TaskFs::empty(),
            Some(request),
            Default::default(),
            TaskState::WaitingForGoal(goal),
        );
        self.read_denial_watch = (kind == GoalMonitorKind::ReadDenial).then_some(ReadDenialWatch {
            goal,
            threshold: threshold as u32,
            first: None,
            denials: 0,
            last_sample: 0,
        });
        // Finish and publication can report a snapshot from before the launch.
        self.wake_goal_monitor();
        Some(handle)
    }

    pub(in crate::scheduler) fn wake_goal_monitor(&mut self) {
        let task = &mut self.tasks[GOAL_MONITOR_TASK_INDEX];
        let TaskState::WaitingForGoal(handle) = task.state else {
            return;
        };
        let Some(LaunchGrant::Monitor(kind)) = task.launch.as_ref().and_then(|launch| launch.grant)
        else {
            return;
        };
        let Some(goal) = self.goal.as_ref().filter(|goal| goal.handle == handle) else {
            return;
        };
        let report = match kind {
            GoalMonitorKind::Finish if !goal.is_active() => {
                Some(MonitorReport::Goal(goal.observation()))
            }
            GoalMonitorKind::Publication => goal.publication_report.map(MonitorReport::Publication),
            _ => None,
        };
        if let Some(report) = report {
            self.monitor_report = Some(report);
            task.state = TaskState::Runnable;
        }
    }

    pub(crate) fn observe_goal(&mut self, deliver: impl FnOnce(&[u8]) -> bool) -> Option<usize> {
        let grant = self.tasks[self.current].launch.as_ref()?.grant?;
        let report = match grant {
            LaunchGrant::ObserveGoal(handle) => MonitorReport::Goal(
                self.goal
                    .as_ref()
                    .filter(|goal| goal.handle == handle)?
                    .observation(),
            ),
            LaunchGrant::Monitor(GoalMonitorKind::Finish | GoalMonitorKind::Publication) => {
                self.monitor_report?
            }
            _ => return None,
        };
        let (kind, record) = match report {
            MonitorReport::Goal(snapshot) => (ReplyKind::GoalObservation, snapshot.encode()),
            MonitorReport::Publication(snapshot) => {
                (ReplyKind::PublicationObserved, snapshot.encode())
            }
        };
        let bytes = record.as_flattened();
        // The observer and its parent see the same kernel sample. A failed copy or
        // occupied mailbox leaves the grant available for a retry.
        let count = self.send_reply(Some(kind), bytes.len(), |reply| {
            reply.copy_from_slice(bytes);
            deliver(reply)
        })?;
        self.tasks[self.current].launch.as_mut()?.grant = None;
        if matches!(grant, LaunchGrant::Monitor(_)) {
            self.monitor_report = None;
        }
        Some(count)
    }

    pub(super) fn record_read_denial(&mut self, event: ReadDenialEvent) {
        let Some(watch) = self
            .read_denial_watch
            .as_mut()
            .filter(|watch| watch.goal == event.goal)
        else {
            return;
        };
        watch.first.get_or_insert(event);
        watch.denials = watch.denials.saturating_add(1);
        self.tasks[GOAL_MONITOR_TASK_INDEX]
            .state
            .wake_from(TaskState::WaitingForGoal(event.goal));
    }

    fn active_read_denial_watch(&mut self) -> Option<&mut ReadDenialWatch> {
        let launch = self.tasks[self.current].launch.as_ref()?;
        let watch = self.read_denial_watch.as_mut()?;
        (launch.grant == Some(LaunchGrant::Monitor(GoalMonitorKind::ReadDenial))
            && launch.request.is_none()
            && self.goal.as_ref()?.handle == watch.goal)
            .then_some(watch)
    }

    pub(crate) fn wait_read_denial_snapshot(
        &mut self,
        frame: &mut TrapFrame,
        deliver: impl FnOnce(&[u8]) -> bool,
    ) -> WaitResult<usize> {
        let Some(watch) = self.active_read_denial_watch() else {
            return WaitResult::Unavailable;
        };
        // Once the threshold is met, the owner's wait requires no further denials.
        if watch.last_sample >= watch.threshold {
            return WaitResult::Unavailable;
        }
        if watch.denials > watch.last_sample
            && let Some(first) = watch.first
        {
            let snapshot = ReadDenialSnapshot {
                first,
                denials: watch.denials,
            }
            .encode();
            let bytes = snapshot.as_flattened();
            return if deliver(bytes) {
                watch.last_sample = watch.denials;
                WaitResult::Ready(bytes.len())
            } else {
                WaitResult::Unavailable
            };
        }
        let goal = watch.goal;
        if self.block_current(frame, TaskState::WaitingForGoal(goal)) {
            WaitResult::Blocked
        } else {
            WaitResult::Unavailable
        }
    }

    pub(in crate::scheduler) fn can_wait_for_monitor(&self, handle: usize) -> bool {
        // The owner may need to launch the child that produces the next denial.
        !self.tasks[GOAL_MONITOR_TASK_INDEX].has_launch(handle)
            || self
                .read_denial_watch
                .as_ref()
                .is_none_or(ReadDenialWatch::threshold_met)
    }

    pub(crate) fn send_read_denial_alert(
        &mut self,
        length: usize,
        copy: impl FnOnce(&mut [u8]) -> bool,
    ) -> Option<usize> {
        let watch = self.active_read_denial_watch()?;
        if watch.last_sample < watch.threshold {
            return None;
        }
        let event = watch.first?.encode();
        let count = self.send_reply(Some(ReplyKind::ReadDenialAlert), length, |reply| {
            copy(reply) && reply == event.as_flattened()
        })?;
        self.tasks[self.current].launch.as_mut()?.grant = None;
        Some(count)
    }

    pub(super) fn revoke_read_denial_watch(&mut self, fs: &mut RamFs) -> bool {
        let armed = self.tasks[GOAL_MONITOR_TASK_INDEX]
            .launch
            .as_ref()
            .is_some_and(|launch| {
                launch.grant == Some(LaunchGrant::Monitor(GoalMonitorKind::ReadDenial))
            });
        if armed {
            self.stop_task(GOAL_MONITOR_TASK_INDEX, fs, TaskCompletion::Cancelled);
        }
        armed
    }

    pub(crate) fn disarm_read_denial_watch(
        &mut self,
        handle: usize,
        fs: &mut RamFs,
    ) -> Option<usize> {
        (self.current == SHELL_TASK_INDEX
            && self.tasks[GOAL_MONITOR_TASK_INDEX].has_launch(handle)
            && self.revoke_read_denial_watch(fs))
        .then_some(0)
    }

    pub(crate) fn take_read_denial_suppressed(&mut self, handle: usize) -> Option<usize> {
        let task = &self.tasks[GOAL_MONITOR_TASK_INDEX];
        let launch = task.launch.as_ref()?;
        if self.current != SHELL_TASK_INDEX
            || launch.handle != handle
            || task.state != TaskState::Stopped
            || launch.reply.is_some()
            || launch.completion.is_some()
        {
            return None;
        }
        self.read_denial_watch
            .take()
            .map(|watch| watch.denials.saturating_sub(1) as usize)
    }
}
