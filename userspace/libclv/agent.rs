pub use clv_abi::agent::{AgentLaunch, AgentStatus};
use clv_abi::goal::GoalStatus;
use clv_abi::{SYS_AGENT_RUN, SYS_AGENT_STATUS};

use super::{ecall, syscall_result};

/// Creates an agent in a ready goal and starts its child in one call.
pub fn run(goal: usize, program: usize) -> Option<AgentLaunch> {
    let (agent, child, ..) = ecall(SYS_AGENT_RUN, goal, program, 0, 0, 0);
    Some(AgentLaunch {
        agent: syscall_result(agent)?,
        child,
    })
}

pub fn status(agent: usize) -> Option<AgentStatus> {
    let (state, child, value, goal) = ecall(SYS_AGENT_STATUS, agent, 0, 0, 0, 0);
    Some(AgentStatus {
        goal,
        status: GoalStatus::from_registers(state, child, value)?,
    })
}
