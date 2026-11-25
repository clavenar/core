use crate::goal::GoalStatus;

const SUSPENDED: usize = 6;

/// The two handles returned by a successful agent run.
pub struct AgentLaunch {
    pub agent: usize,
    pub child: usize,
}

/// The owning goal and the state of the agent's original child launch.
pub struct AgentStatus {
    pub goal: usize,
    /// A live agent's goal stays running while the child is suspended.
    pub status: GoalStatus,
    /// True while the scheduler holds the original child at its saved frame.
    pub suspended: bool,
}

impl AgentStatus {
    /// RV64 state, child and detail in a0–a2. The owning goal is returned in a3.
    pub const fn registers(&self) -> (usize, usize, usize) {
        match self.status {
            GoalStatus::Running { child } if self.suspended => (SUSPENDED, child, 0),
            status => status.registers(),
        }
    }

    pub fn from_registers(state: usize, child: usize, value: usize, goal: usize) -> Option<Self> {
        let suspended = state == SUSPENDED;
        let status = if suspended {
            GoalStatus::Running { child }
        } else {
            GoalStatus::from_registers(state, child, value)?
        };
        Some(Self {
            goal,
            status,
            suspended,
        })
    }
}

#[repr(usize)]
pub enum ControlResult {
    Applied = 0,
    AlreadyFinished = 1,
}
