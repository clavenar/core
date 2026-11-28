pub use clv_abi::agent::{
    AgentLaunch, AgentStatus, CURRENT_AGENT, CheckpointReply, CheckpointResumeError, ControlResult,
    DefinitionId, DefinitionPin, FIND_GREETING_V1, FIND_GREETING_V2, FIND_GREETING_V3,
    FIND_GREETING_V4, FIND_GREETING_V5, FIND_GREETING_V6, FIND_GREETING_V7, FIND_GREETING_V8,
    FileDelegation, FileToolInvocation, PlanIntent, ResumeReceipt,
};
use clv_abi::agent::{
    EncodedDefinitionPin, LAUNCH_REQUEST_MAX_BYTES, RECV_CHECKPOINT, RUN_CHECKPOINT,
    RUN_HTTPS_PROBE, RUN_MCP_INIT, RUN_PLAN_DEFINITION, RUN_PLAN_INTENT, RUN_REPLY_LIMIT,
    RUN_STAGE_RENAME, RUN_STAGE_RENAME_PAIR, RUN_SUSPENDED, encode_rename_pair,
};
use clv_abi::tool::{FILE_TOOL_MCP, FILE_TOOL_STDIO};
use clv_abi::{
    MESSAGE_MAX_BYTES, NO_CHILD_HANDLE, PATH_MAX_BYTES, Program, SYS_AGENT_DELEGATE,
    SYS_AGENT_DELEGATE_WITH_FILE, SYS_AGENT_DELEGATE_WITH_FILE_ARG, SYS_AGENT_INVOKE_FILE_TOOL,
    SYS_AGENT_LAUNCH_SUPERVISOR, SYS_AGENT_RESUME, SYS_AGENT_RESUME_GRANTED, SYS_AGENT_RUN,
    SYS_AGENT_RUN_SUSPENDED, SYS_AGENT_RUN_WITH_REPLY_BUDGET, SYS_AGENT_RUN_WITH_REQUEST,
    SYS_AGENT_STAGE_RENAME, SYS_AGENT_STATUS, SYS_AGENT_STATUS_WITH_DEFINITION, SYS_AGENT_SUSPEND,
    SYS_REPLY_RECV,
};

use super::{ecall, ecall6, syscall, syscall_result, with_stack_bytes};

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

/// Queues a file delegation request as part of the agent launch.
pub fn run_with_request(
    goal: usize,
    program: usize,
    suspended: bool,
    reply_bytes: Option<usize>,
    request: &FileDelegation<'_>,
) -> Option<AgentLaunch> {
    launch(
        goal,
        program,
        suspended,
        reply_bytes,
        Some(RunRequest::DelegateWithFile(*request)),
    )
}

pub enum RunRequest<'a> {
    DelegateWithFile(FileDelegation<'a>),
    Plan(PlanIntent),
    Definition(DefinitionId),
    Checkpoint(DefinitionId),
    StageRename(&'a [u8]),
    StageRenamePair([&'a [u8]; 2]),
    HttpsProbe,
    McpInit,
}

/// Launches an agent with an optional typed request in its mailbox.
pub fn launch(
    goal: usize,
    program: usize,
    suspended: bool,
    reply_bytes: Option<usize>,
    request: Option<RunRequest<'_>>,
) -> Option<AgentLaunch> {
    let Some(request) = request else {
        return run_with_options(goal, program, suspended, reply_bytes);
    };
    let mut buffer = [0; LAUNCH_REQUEST_MAX_BYTES];
    let mut flags = if suspended { RUN_SUSPENDED } else { 0 };
    if reply_bytes.is_some() {
        flags |= RUN_REPLY_LIMIT;
    }
    let bytes = match request {
        RunRequest::DelegateWithFile(request) => request.encode(buffer.first_chunk_mut()?)?,
        RunRequest::Plan(intent) => {
            flags |= RUN_PLAN_INTENT;
            buffer[0] = intent as u8;
            &buffer[..1]
        }
        RunRequest::Definition(id) => {
            flags |= RUN_PLAN_DEFINITION;
            buffer[..2].copy_from_slice(&id.encode());
            &buffer[..2]
        }
        RunRequest::Checkpoint(id) => {
            flags |= RUN_PLAN_DEFINITION | RUN_CHECKPOINT;
            buffer[..2].copy_from_slice(&id.encode());
            &buffer[..2]
        }
        RunRequest::StageRename(name) => {
            flags |= RUN_STAGE_RENAME;
            let bytes = buffer[..PATH_MAX_BYTES].get_mut(..name.len())?;
            bytes.copy_from_slice(name);
            bytes
        }
        RunRequest::StageRenamePair(names) => {
            flags |= RUN_STAGE_RENAME_PAIR;
            encode_rename_pair(names, &mut buffer)?
        }
        RunRequest::HttpsProbe => {
            flags |= RUN_HTTPS_PROBE;
            &[]
        }
        RunRequest::McpInit => {
            flags |= RUN_MCP_INIT;
            &[]
        }
    };
    let (agent, child, ..) = ecall6(
        SYS_AGENT_RUN_WITH_REQUEST,
        goal,
        program,
        flags,
        reply_bytes.unwrap_or(0),
        bytes.as_ptr().expose_provenance(),
        bytes.len(),
    );
    Some(AgentLaunch {
        agent: syscall_result(agent)?,
        child,
    })
}

/// Proposes a basename for the file granted to the calling goal agent.
pub fn stage_rename(fd: usize, new_name: &[u8]) -> bool {
    super::fs::submit_rename(new_name, |address, length| {
        syscall(SYS_AGENT_STAGE_RENAME, fd, address, length)
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

/// Delegates the agent's file and transfers a copied argument with the launch.
pub fn delegate_with_file_arg(
    agent: usize,
    program: usize,
    reply_bytes: usize,
    argument: &[u8],
) -> Option<usize> {
    delegate_literal(
        SYS_AGENT_DELEGATE_WITH_FILE_ARG,
        agent,
        program,
        reply_bytes,
        argument,
        0,
    )
}

/// Requests the exact registered file tool using the agent's live grant and reservation.
pub fn invoke_file_tool(agent: usize, request: &FileToolInvocation<'_>) -> Option<usize> {
    delegate_literal(
        SYS_AGENT_INVOKE_FILE_TOOL,
        agent,
        request.tool.0.try_into().ok()?,
        request.reply_bytes,
        request.argument,
        0,
    )
}

/// Invokes the file tool with stdin and stdout using its approved literal.
pub fn invoke_file_tool_stdio(agent: usize, request: &FileToolInvocation<'_>) -> Option<usize> {
    delegate_literal(
        SYS_AGENT_INVOKE_FILE_TOOL,
        agent,
        request.tool.0.try_into().ok()?,
        request.reply_bytes,
        request.argument,
        FILE_TOOL_STDIO,
    )
}

/// Starts the registered file tool as a local MCP server on its stdio pipes.
pub fn invoke_file_tool_mcp(agent: usize, request: &FileToolInvocation<'_>) -> Option<usize> {
    delegate_literal(
        SYS_AGENT_INVOKE_FILE_TOOL,
        agent,
        request.tool.0.try_into().ok()?,
        request.reply_bytes,
        request.argument,
        FILE_TOOL_MCP,
    )
}

fn delegate_literal(
    call: usize,
    agent: usize,
    selector: usize,
    reply_bytes: usize,
    argument: &[u8],
    mode: usize,
) -> Option<usize> {
    with_stack_bytes::<MESSAGE_MAX_BYTES>(argument, |address, length| {
        ecall6(call, agent, selector, reply_bytes, address, length, mode).0
    })
}

/// A running agent delegates its own file and receives the helper's results.
pub fn delegate_from_current(request: &FileDelegation<'_>) -> Option<usize> {
    delegate_with_file_arg(
        CURRENT_AGENT,
        request.program as usize,
        request.reply_bytes,
        request.argument,
    )
}

pub fn status(agent: usize) -> Option<AgentStatus> {
    let (state, child, value, goal) = ecall(SYS_AGENT_STATUS, agent, 0, 0, 0, 0);
    AgentStatus::from_registers(state, child, value, goal)
}

/// Reads an agent's original launch state and its optional definition pin together.
pub fn status_with_definition(
    agent: usize,
    bytes: &mut EncodedDefinitionPin,
) -> Option<(AgentStatus, Option<DefinitionPin<'_>>)> {
    let (state, child, value, goal) = ecall(
        SYS_AGENT_STATUS_WITH_DEFINITION,
        agent,
        bytes.as_mut_ptr().expose_provenance(),
        bytes.len(),
        0,
        0,
    );
    let status = AgentStatus::from_registers(state, child, value, goal)?;
    let pin = if bytes[0] == 0 {
        None
    } else {
        Some(DefinitionPin::decode(bytes)?)
    };
    Some((status, pin))
}

pub fn suspend(agent: usize) -> Option<ControlResult> {
    control_result(syscall(SYS_AGENT_SUSPEND, agent, 0, 0))
}

pub fn resume(agent: usize) -> Result<ControlResult, CheckpointResumeError> {
    let (value, ..) = resume_call(agent)?;
    control_result(value).ok_or(CheckpointResumeError::Unavailable)
}

/// Completes a recovered checkpoint and returns its new goal and launch handles.
pub fn resume_checkpoint() -> Result<(usize, AgentLaunch), CheckpointResumeError> {
    let (agent, child, goal) = resume_call(CURRENT_AGENT)?;
    Ok((goal, AgentLaunch { agent, child }))
}

fn resume_call(agent: usize) -> Result<(usize, usize, usize), CheckpointResumeError> {
    loop {
        let (value, child, goal, status) = ecall(SYS_AGENT_RESUME, agent, 0, 0, 0, 0);
        if let Some(value) = syscall_result(value) {
            return Ok((value, child, goal));
        }
        let reason = CheckpointResumeError::from_raw(status);
        if reason != CheckpointResumeError::Waiting {
            return Err(reason);
        }
        core::hint::spin_loop();
    }
}

/// Inspects the completed checkpoint reply and its final byte charges.
pub fn receive_checkpoint_reply(buffer: &mut [u8]) -> Option<CheckpointReply<'_>> {
    let (length, read_spent, reply_spent, metadata) = ecall(
        SYS_REPLY_RECV,
        buffer.as_mut_ptr().expose_provenance(),
        buffer.len(),
        NO_CHILD_HANDLE,
        RECV_CHECKPOINT,
        0,
    );
    CheckpointReply::from_registers(buffer.get(..length)?, read_spent, reply_spent, metadata)
}

/// Starts a shell-owned supervisor for this agent's current suspension.
pub fn launch_supervisor(agent: usize, program: Program) -> Option<usize> {
    syscall_result(syscall(
        SYS_AGENT_LAUNCH_SUPERVISOR,
        agent,
        program as usize,
        0,
    ))
}

/// Resumes the child bound to this launch and queues a receipt for the owner.
pub fn resume_granted() -> bool {
    syscall(SYS_AGENT_RESUME_GRANTED, 0, 0, 0) == 0
}

fn control_result(result: usize) -> Option<ControlResult> {
    match result {
        n if n == ControlResult::Applied as usize => Some(ControlResult::Applied),
        n if n == ControlResult::AlreadyFinished as usize => Some(ControlResult::AlreadyFinished),
        _ => None,
    }
}
