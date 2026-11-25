use crate::goal::GoalStatus;

/// The two handles returned by a successful agent run.
pub struct AgentLaunch {
    pub agent: usize,
    pub child: usize,
}

/// The owning goal and the state of the agent's original child launch.
pub struct AgentStatus {
    pub goal: usize,
    pub status: GoalStatus,
}
