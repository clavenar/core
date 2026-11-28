use clv_abi::goal::GoalDeadline;
use clv_abi::net::{HTTPS_PROBE_TIMEOUT_NS, HttpsRequest, HttpsStatus};

use super::super::{Scheduler, TaskState};
use crate::arch::monotonic_nanos;
use crate::programs::NETD_TASK_INDEX;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Queued,
    Running,
    Finished(HttpsStatus),
}

pub(in crate::scheduler) struct HttpsExchange {
    goal: usize,
    launch: usize,
    id: usize,
    kind: HttpsRequest,
    deadline: u64,
    phase: Phase,
}

impl Scheduler {
    pub(crate) fn submit_https(
        &mut self,
        kind: HttpsRequest,
        network_available: bool,
    ) -> Result<usize, HttpsStatus> {
        let (_, member, _) = self.current_agent_member().ok_or(HttpsStatus::Denied)?;
        let launch = member.handle;
        let goal = self.goal.as_mut().ok_or(HttpsStatus::Denied)?;
        let grant = match kind {
            HttpsRequest::Probe => &mut goal.https_available,
            HttpsRequest::McpInit => &mut goal.mcp_init_available,
            HttpsRequest::McpList => &mut goal.mcp_list_available,
        };
        if !*grant {
            return Err(HttpsStatus::Denied);
        }
        let now = monotonic_nanos();
        let limit = now.saturating_add(HTTPS_PROBE_TIMEOUT_NS);
        let deadline = match goal.deadline {
            GoalDeadline::Unlimited => limit,
            GoalDeadline::At(at) if at > now => limit.min(at),
            _ => return Err(HttpsStatus::TimedOut),
        };
        if !network_available
            || !cfg!(feature = "tls-probe")
            || self.tasks[NETD_TASK_INDEX].state == TaskState::Stopped
            || self.https.is_some()
        {
            return Err(HttpsStatus::Unavailable);
        }
        let id = match kind {
            HttpsRequest::Probe => launch,
            HttpsRequest::McpInit | HttpsRequest::McpList => {
                let id = self.next_handle;
                self.next_handle = id.checked_add(1).ok_or(HttpsStatus::Unavailable)?;
                id
            }
        };
        self.https = Some(HttpsExchange {
            goal: goal.handle,
            launch,
            id,
            kind,
            deadline,
            phase: Phase::Queued,
        });
        *grant = false;
        self.wake_netd(true, false);
        Ok(id)
    }

    pub(crate) fn poll_https(&mut self, request: usize) -> Option<usize> {
        let exchange = self.https.as_ref()?;
        if exchange.id != request
            || !self
                .current_agent_member()
                .is_some_and(|(goal, member, _)| {
                    goal.handle == exchange.goal && member.handle == exchange.launch
                })
        {
            return None;
        }
        let status = match exchange.phase {
            Phase::Finished(status) => status,
            Phase::Queued if monotonic_nanos() >= exchange.deadline => HttpsStatus::TimedOut,
            _ => return Some(0),
        };
        self.https = None;
        Some(status as usize)
    }

    pub(crate) fn https_queued(&self) -> bool {
        self.https
            .as_ref()
            .is_some_and(|exchange| exchange.phase == Phase::Queued)
    }

    pub(crate) fn take_https(&mut self) -> Option<(usize, u64, HttpsRequest)> {
        let exchange = self
            .https
            .as_mut()
            .filter(|exchange| exchange.phase == Phase::Queued)?;
        if monotonic_nanos() >= exchange.deadline {
            self.finish_https(HttpsStatus::TimedOut);
            return None;
        }
        exchange.phase = Phase::Running;
        Some((exchange.id, exchange.deadline, exchange.kind))
    }

    pub(crate) fn https_pending(&self) -> bool {
        self.https
            .as_ref()
            .is_some_and(|exchange| !matches!(exchange.phase, Phase::Finished(_)))
    }

    pub(crate) fn https_deadline(&self, request: usize) -> Option<u64> {
        let exchange = self.https.as_ref()?;
        (exchange.id == request && exchange.phase == Phase::Running).then_some(exchange.deadline)
    }

    pub(crate) fn complete_https(&mut self, request: usize, status: HttpsStatus) -> Option<()> {
        let deadline = self.https_deadline(request)?;
        self.finish_https(if monotonic_nanos() >= deadline {
            HttpsStatus::TimedOut
        } else {
            status
        });
        Some(())
    }

    fn finish_https(&mut self, status: HttpsStatus) {
        let Some(exchange) = &mut self.https else {
            return;
        };
        exchange.phase = Phase::Finished(status);
    }

    pub(in crate::scheduler) fn stop_https_task(&mut self, index: usize) {
        let Some(exchange) = &self.https else {
            return;
        };
        if self.tasks[index].has_launch(exchange.launch) {
            self.https = None;
            self.wake_netd(true, true);
        } else if index == NETD_TASK_INDEX && !matches!(exchange.phase, Phase::Finished(_)) {
            self.finish_https(HttpsStatus::Unavailable);
        }
    }
}
