pub use clv_abi::agent::{AgentLaunch, AgentStatus, ControlResult};
use clv_abi::{
    SYS_AGENT_RESUME, SYS_AGENT_RUN, SYS_AGENT_RUN_SUSPENDED, SYS_AGENT_STATUS, SYS_AGENT_SUSPEND,
};

use super::{ecall, syscall, syscall_result};

/// Creates an agent in a ready goal and starts its child in one call.
pub fn run(goal: usize, program: usize) -> Option<AgentLaunch> {
    start(SYS_AGENT_RUN, goal, program)
}

/// Creates the same launch with its child paused at the program entry point.
pub fn run_suspended(goal: usize, program: usize) -> Option<AgentLaunch> {
    start(SYS_AGENT_RUN_SUSPENDED, goal, program)
}

fn start(call: usize, goal: usize, program: usize) -> Option<AgentLaunch> {
    let (agent, child, ..) = ecall(call, goal, program, 0, 0, 0);
    Some(AgentLaunch {
        agent: syscall_result(agent)?,
        child,
    })
}

pub fn status(agent: usize) -> Option<AgentStatus> {
    let (state, child, value, goal) = ecall(SYS_AGENT_STATUS, agent, 0, 0, 0, 0);
    AgentStatus::from_registers(state, child, value, goal)
}

pub fn suspend(agent: usize) -> Option<ControlResult> {
    control(SYS_AGENT_SUSPEND, agent)
}

pub fn resume(agent: usize) -> Option<ControlResult> {
    control(SYS_AGENT_RESUME, agent)
}

fn control(call: usize, agent: usize) -> Option<ControlResult> {
    match syscall(call, agent, 0, 0) {
        n if n == ControlResult::Applied as usize => Some(ControlResult::Applied),
        n if n == ControlResult::AlreadyFinished as usize => Some(ControlResult::AlreadyFinished),
        _ => None,
    }
}
