pub use clv_abi::agent::{AgentLaunch, AgentStatus, ControlResult};
use clv_abi::{
    SYS_AGENT_DELEGATE, SYS_AGENT_DELEGATE_WITH_FILE, SYS_AGENT_RESUME, SYS_AGENT_RUN,
    SYS_AGENT_RUN_SUSPENDED, SYS_AGENT_RUN_WITH_REPLY_BUDGET, SYS_AGENT_STATUS, SYS_AGENT_SUSPEND,
};

use super::{ecall, syscall, syscall_result};

/// Creates the goal's agent and starts its child in one call.
pub fn run(goal: usize, program: usize) -> Option<AgentLaunch> {
    run_with_options(goal, program, false, None)
}

/// Creates the same launch with its child paused at the program entry point.
pub fn run_suspended(goal: usize, program: usize) -> Option<AgentLaunch> {
    run_with_options(goal, program, true, None)
}

/// Reserves the requested bytes, or inherits the goal's available allowance.
pub fn run_with_options(
    goal: usize,
    program: usize,
    suspended: bool,
    reply_bytes: Option<usize>,
) -> Option<AgentLaunch> {
    let call = match reply_bytes {
        Some(_) => SYS_AGENT_RUN_WITH_REPLY_BUDGET,
        None if suspended => SYS_AGENT_RUN_SUSPENDED,
        None => SYS_AGENT_RUN,
    };
    let (agent, child, ..) = ecall(
        call,
        goal,
        program,
        reply_bytes.unwrap_or(0),
        usize::from(suspended),
        0,
    );
    Some(AgentLaunch {
        agent: syscall_result(agent)?,
        child,
    })
}

/// Starts an ordinary helper using bytes from the live agent's reservation.
pub fn delegate(agent: usize, program: usize, reply_bytes: usize) -> Option<usize> {
    syscall_result(syscall(SYS_AGENT_DELEGATE, agent, program, reply_bytes))
}

/// Gives the helper a fresh read-only handle to the agent's open delegable file.
pub fn delegate_with_file(agent: usize, program: usize, reply_bytes: usize) -> Option<usize> {
    syscall_result(syscall(
        SYS_AGENT_DELEGATE_WITH_FILE,
        agent,
        program,
        reply_bytes,
    ))
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
