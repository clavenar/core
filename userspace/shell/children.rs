use clv::{ChildReply, ChildStatus, agent, pipe, print};
use clv_abi::bridge::BridgeFailure;
use clv_abi::goal::{GoalMonitorKind, MAX_READ_DENIAL_THRESHOLD};
use clv_abi::{
    ChildEvent, CompletionKind, MESSAGE_MAX_BYTES, NO_CHILD_HANDLE, OPEN_READ, PIPE_CAPACITY,
    PROGRAM_HELLO, PROGRAMS, Program, ReplyKind, RequestKind,
};

use super::{
    command::GoalFlag,
    files::OpenHandle,
    goal::{GoalCommand, GoalOptions},
    help::Entry,
    hex, number, print_handle, print_line, split_literal, split_word,
};

pub(super) struct Children {
    handles: [usize; PROGRAMS.len()],
    echo_report_handle: usize,
    monitor: Option<GoalMonitor>,
}

struct GoalMonitor {
    handle: usize,
    reply_kind: ReplyKind,
    collected: bool,
    restart: Option<FinishMonitorRestart>,
}

struct FinishMonitorRestart {
    goal: usize,
    attempted: bool,
}

struct Helper<'a> {
    program: Program,
    reply_bytes: usize,
    with_file: bool,
    argument: Option<&'a [u8]>,
}

impl<'a> Helper<'a> {
    fn parse(arguments: &'a [u8]) -> Option<Self> {
        let (options, rest) = GoalOptions::parse(arguments, GoalCommand::AgentDelegate)?;
        let (name, args) = split_word(rest);
        let program = Program::named(name)?;
        let argument = match (program, options.with_file, args) {
            (Program::Grep, true, _) => {
                let (pattern, b"") = grep_pattern(args)? else {
                    return None;
                };
                Some(pattern)
            }
            (Program::Grep, false, _) => return None,
            (_, _, b"") => None,
            _ => return None,
        };
        Some(Self {
            program,
            reply_bytes: options.reply_bytes?,
            with_file: options.with_file,
            argument,
        })
    }

    fn file_request(&self) -> Option<agent::FileDelegation<'a>> {
        Some(agent::FileDelegation {
            program: self.program,
            reply_bytes: self.reply_bytes,
            argument: self.argument?,
        })
    }
}

impl Children {
    pub(super) fn new() -> Self {
        Self {
            handles: [NO_CHILD_HANDLE; PROGRAMS.len()],
            echo_report_handle: NO_CHILD_HANDLE,
            monitor: None,
        }
    }

    fn collect_reply(&mut self, program: Program, handle: usize) {
        self.handles[program as usize] = handle;
        print_reply(handle);
    }

    pub(super) fn run(&mut self, arguments: &[u8]) {
        let (name, directory) = split_word(arguments);
        match Program::named(name) {
            Some(program) if directory.is_empty() => {
                if let Some(handle) = clv::spawn(program as usize, None, None) {
                    self.handles[program as usize] = handle;
                } else {
                    print(name);
                    print(b" is already running\r\n");
                }
            }
            Some(Program::Hello) => {
                if let Some(handle) = spawn_scoped(Program::Hello, directory, |program, fd| {
                    clv::spawn(program, Some(fd), None)
                }) {
                    self.handles[PROGRAM_HELLO] = handle;
                }
            }
            _ => Entry::Run.usage(),
        }
    }

    // Keep goal option parsing off the shell command loop's stack frame.
    #[inline(never)]
    pub(super) fn run_in_goal(&mut self, goal: Option<usize>, arguments: &[u8]) {
        let Some((options, program, b"")) = goal_program(arguments, GoalCommand::Run) else {
            return Entry::GoalRun.usage();
        };
        let Some(goal) = goal else {
            return print(b"no current goal\r\n");
        };
        let result = match options.reply_bytes {
            Some(bytes) => clv::goal::run_with_reply_budget(goal, program as usize, bytes),
            None => clv::goal::run(goal, program as usize),
        };
        if let Some(handle) = result {
            self.handles[program as usize] = handle;
            print_handle(b"goal child=", handle);
        } else {
            print(b"cannot start goal child\r\n");
        }
    }

    pub(super) fn observe_goal(&mut self, goal: Option<usize>, arguments: &[u8]) {
        let monitor = match split_word(arguments) {
            (b"--collect", b"") => return self.collect_goal_monitor(),
            (b"--disarm", b"") => return self.disarm_read_denial_watch(),
            (b"--on-finish", arguments) => {
                let restart_once = match split_word(arguments) {
                    (b"echo", b"") => false,
                    (b"echo", b"--restart-once") => true,
                    _ => return Entry::GoalObserve.usage(),
                };
                Some((GoalMonitorKind::Finish, 0, restart_once))
            }
            (b"--on-publication", b"echo") => Some((GoalMonitorKind::Publication, 0, false)),
            (b"--on-read-denial", arguments) => {
                let (b"echo", options) = split_word(arguments) else {
                    return Entry::GoalObserve.usage();
                };
                let threshold = match split_word(options) {
                    (b"", b"") => 1,
                    (b"--after-denials", digits) => {
                        let Some(threshold) = number(digits)
                            .filter(|value| (1..=MAX_READ_DENIAL_THRESHOLD).contains(value))
                        else {
                            return print(b"--after-denials must be between 1 and 16\r\n");
                        };
                        threshold
                    }
                    _ => return Entry::GoalObserve.usage(),
                };
                Some((GoalMonitorKind::ReadDenial, threshold, false))
            }
            (b"echo", b"") => None,
            _ => return Entry::GoalObserve.usage(),
        };
        let Some(goal) = goal else {
            return print(b"no current goal\r\n");
        };
        if let Some((monitor, threshold, restart_once)) = monitor {
            let (launch, reply_kind) = match monitor {
                GoalMonitorKind::Finish => (
                    clv::goal::launch_finish_monitor(goal),
                    ReplyKind::GoalObservation,
                ),
                GoalMonitorKind::Publication => (
                    clv::goal::launch_publication_monitor(goal),
                    ReplyKind::PublicationObserved,
                ),
                GoalMonitorKind::ReadDenial => (
                    clv::goal::launch_read_denial_watch_after(goal, threshold),
                    ReplyKind::ReadDenialAlert,
                ),
            };
            let Some(handle) = launch else {
                return print(b"cannot start goal monitor\r\n");
            };
            self.monitor = Some(GoalMonitor {
                handle,
                reply_kind,
                collected: false,
                restart: restart_once.then_some(FinishMonitorRestart {
                    goal,
                    attempted: false,
                }),
            });
            return print_handle(b"goal monitor=", handle);
        }
        let Some(handle) = clv::goal::launch_observer(goal, Program::Echo) else {
            return print(b"cannot start goal observer\r\n");
        };
        if !self.collect_echo_report(handle, ReplyKind::GoalObservation) {
            print(b"goal observation unavailable\r\n");
        }
    }

    fn disarm_read_denial_watch(&mut self) {
        let Some(monitor) = self.monitor.as_mut().filter(|monitor| {
            !monitor.collected && monitor.reply_kind == ReplyKind::ReadDenialAlert
        }) else {
            return print(b"no armed read-denial watch\r\n");
        };
        if !clv::goal::disarm_read_denial_watch(monitor.handle) {
            return print(b"no armed read-denial watch\r\n");
        }
        let _ = clv::wait(monitor.handle);
        monitor.collected = true;
        print(b"read-denial watch disarmed\r\n");
    }

    fn collect_goal_monitor(&mut self) {
        let Some(monitor) = self.monitor.as_mut().filter(|monitor| !monitor.collected) else {
            return print(b"no goal monitor to collect\r\n");
        };
        let mut buffer = [0; MESSAGE_MAX_BYTES];
        loop {
            let Some(status) = clv::wait(monitor.handle) else {
                return print(b"pending\r\n");
            };
            monitor.collected = true;
            let reply = clv::receive_reply(monitor.handle, &mut buffer);
            let fault = match status {
                ChildStatus::Faulted(cause) if monitor.restart.is_some() => Some(cause),
                _ => None,
            };
            if let Some(ChildReply::Received { kind, bytes, .. }) = reply
                && kind == monitor.reply_kind
                && (matches!(status, ChildStatus::Exited(0)) || fault.is_some())
            {
                return display_reply(monitor.handle, kind, bytes);
            }
            if let Some(cause) = fault
                && let Some(restart) = &mut monitor.restart
            {
                if matches!(reply, Some(ChildReply::Empty)) && !restart.attempted {
                    restart.attempted = true;
                    if let Some(replacement) = clv::goal::restart_finish_monitor(restart.goal) {
                        print_line(&[
                            b"goal monitor=",
                            &hex(monitor.handle),
                            b" faulted scause=",
                            &hex(cause),
                            b" restarted as ",
                            &hex(replacement),
                        ]);
                        monitor.handle = replacement;
                        monitor.collected = false;
                        continue;
                    }
                }
                return print_line(&[
                    b"goal monitor=",
                    &hex(monitor.handle),
                    b" faulted scause=",
                    &hex(cause),
                ]);
            }
            return print(match status {
                ChildStatus::Cancelled => b"goal monitor cancelled\r\n",
                _ => b"goal observation unavailable\r\n",
            });
        }
    }

    pub(super) fn resume_via_echo(&mut self, agent: usize) {
        let launch = agent::launch_supervisor(agent, Program::Echo);
        if !launch.is_some_and(|handle| self.collect_echo_report(handle, ReplyKind::AgentResumed)) {
            print(b"cannot resume agent via echo\r\n");
        }
    }

    fn collect_echo_report(&mut self, handle: usize, expected: ReplyKind) -> bool {
        let mut buffer = [0; MESSAGE_MAX_BYTES];
        let Some(bytes) = self.wait_echo_report(handle, expected, &mut buffer) else {
            return false;
        };
        display_reply(handle, expected, bytes);
        true
    }

    pub(super) fn review_rename(&mut self, goal: usize) -> Option<clv::goal::RenameReviewReceipt> {
        let handle = clv::goal::launch_rename_reviewer(goal)?;
        let mut buffer = [0; MESSAGE_MAX_BYTES];
        let bytes = self.wait_echo_report(handle, ReplyKind::RenameReviewed, &mut buffer)?;
        let receipt = clv::goal::RenameReviewReceipt::decode(bytes)?;
        (receipt.reviewer == handle && receipt.goal == goal).then_some(receipt)
    }

    fn wait_echo_report<'a>(
        &mut self,
        handle: usize,
        expected: ReplyKind,
        buffer: &'a mut [u8],
    ) -> Option<&'a [u8]> {
        // Keep the report handle for its completion event. Named waits use the ordinary child.
        self.echo_report_handle = handle;
        receive_report(handle, clv::wait(handle)?, expected, buffer)
    }

    pub(super) fn run_agent(
        &mut self,
        goal: usize,
        arguments: &[u8],
    ) -> Option<agent::AgentLaunch> {
        let Some((options, program, request)) = agent_run_args(arguments) else {
            Entry::AgentRun.usage();
            return None;
        };
        let Some(launch) = agent::launch(
            goal,
            program as usize,
            options.suspended,
            options.reply_bytes,
            request,
        ) else {
            print(b"cannot start agent\r\n");
            return None;
        };
        self.handles[program as usize] = launch.child;
        Some(launch)
    }

    pub(super) fn resume_checkpoint(
        &mut self,
    ) -> Result<(usize, agent::AgentLaunch), agent::CheckpointResumeError> {
        let (goal, launch) = agent::resume_checkpoint()?;
        self.handles[Program::Hello as usize] = launch.child;
        Ok((goal, launch))
    }

    pub(super) fn delegate(&mut self, agent: Option<usize>, arguments: &[u8]) {
        let Some(helper) = Helper::parse(arguments) else {
            return Entry::Delegate.usage();
        };
        let Some(agent) = agent else {
            return print(b"no current agent\r\n");
        };
        let program = helper.program as usize;
        let launch = match helper.argument {
            Some(argument) => {
                agent::delegate_with_file_arg(agent, program, helper.reply_bytes, argument)
            }
            None if helper.with_file => {
                agent::delegate_with_file(agent, program, helper.reply_bytes)
            }
            None => agent::delegate(agent, program, helper.reply_bytes),
        };
        if let Some(handle) = launch {
            self.handles[program] = handle;
            print_handle(b"delegated child=", handle);
        } else {
            print(b"cannot delegate child\r\n");
        }
    }

    pub(super) fn grep(&mut self, arguments: &[u8]) {
        let Some((pattern, path)) = grep_pattern(arguments).filter(|(_, path)| !path.is_empty())
        else {
            return Entry::Grep.usage();
        };
        if let Some(handle) = spawn_scoped(Program::Grep, path, |program, fd| {
            clv::spawn_with_file_arg(program, fd, pattern)
        }) {
            self.collect_reply(Program::Grep, handle);
        }
    }

    pub(super) fn ask(&mut self, arguments: &[u8]) {
        if let (b"--stdio", rest) = split_word(arguments) {
            return self.ask_stdio(rest);
        }
        let (kind, arguments) = request_kind(arguments, RequestKind::Echo);
        let (name, text) = split_word(arguments);
        let program = match Program::named(name) {
            Some(program) if !text.is_empty() => program,
            _ => return Entry::Ask.usage(),
        };
        let Some(handle) = clv::spawn_request(program as usize, None, kind, text) else {
            print(b"cannot ask ");
            print(name);
            print(b"\r\n");
            return;
        };
        self.collect_reply(program, handle);
    }

    fn ask_stdio(&mut self, arguments: &[u8]) {
        let (b"echo", text) = split_word(arguments) else {
            return Entry::Ask.usage();
        };
        let program = Program::Echo as usize;
        let Some(handle) = clv::spawn_stdio(program) else {
            return print(b"cannot start stdio echo\r\n");
        };
        self.handles[program] = handle;
        let exchanged = print_echo_stream(handle, text).is_some();
        let stdin_closed = pipe::close_stdin(handle);
        let eof =
            exchanged && stdin_closed && pipe::read(handle, &mut [0]).is_some_and(<[u8]>::is_empty);
        pipe::close(handle);
        print(b"\r\n");
        if !eof {
            print(b"stdio exchange failed\r\n");
        }
    }

    pub(super) fn ask_at(&mut self, arguments: &[u8]) {
        let (kind, arguments) = request_kind(arguments, RequestKind::ReadFile);
        let (name, arguments) = split_word(arguments);
        let (directory, payload) = split_word(arguments);
        if Program::named(name) != Some(Program::Hello) || payload.is_empty() {
            return Entry::AskAt.usage();
        }
        if let Some(handle) = spawn_scoped(Program::Hello, directory, |program, fd| {
            clv::spawn_request(program, Some(fd), kind, payload)
        }) {
            self.collect_reply(Program::Hello, handle);
        }
    }

    pub(super) fn ask_file(&mut self, arguments: &[u8]) {
        let (first, rest) = split_word(arguments);
        let stream = first == b"--stream";
        let (name, path) = if stream {
            split_word(rest)
        } else {
            (first, rest)
        };
        if Program::named(name) != Some(Program::Hello) || path.is_empty() {
            return Entry::AskFile.usage();
        }
        let spawn = if stream {
            clv::spawn_with_file_pipe
        } else {
            clv::spawn_with_file
        };
        if let Some(handle) = spawn_scoped(Program::Hello, path, spawn) {
            self.handles[PROGRAM_HELLO] = handle;
            if stream {
                print_stream(handle);
            } else {
                print_reply(handle);
            }
        }
    }

    pub(super) fn recv(&self, arguments: &[u8]) {
        match split_word(arguments) {
            (b"--checkpoint", b"hello") => super::goal::recv_checkpoint(),
            (name, b"") => match Program::named(name) {
                Some(program) => print_reply(self.handles[program as usize]),
                None => Entry::Recv.usage(),
            },
            _ => Entry::Recv.usage(),
        }
    }

    pub(super) fn event(&self) {
        let (handle, kind) = match clv::event_wait() {
            Some(ChildEvent::Completed { handle, kind }) => (handle, kind),
            Some(ChildEvent::Empty) => return print(b"no child event\r\n"),
            None => return print(b"event wait failed\r\n"),
        };
        let name = if handle == self.echo_report_handle {
            b"echo report".as_slice()
        } else if self
            .monitor
            .as_ref()
            .is_some_and(|monitor| monitor.handle == handle)
        {
            b"goal monitor".as_slice()
        } else if let Some(index) = self.handles.iter().position(|&saved| saved == handle) {
            PROGRAMS[index].1.as_bytes()
        } else {
            return print(b"unknown child handle\r\n");
        };
        let status: &[u8] = match kind {
            CompletionKind::Exited => b" exited",
            CompletionKind::Faulted => b" faulted",
            CompletionKind::Cancelled => b" cancelled",
        };
        print_line(&[name, status]);
    }

    pub(super) fn wait(&self, name: &[u8]) {
        let Some(program) = Program::named(name) else {
            return Entry::Wait.usage();
        };
        let (status, value): (&[u8], usize) = match clv::wait(self.handles[program as usize]) {
            Some(ChildStatus::Exited(code)) => (b" exited ", code),
            Some(ChildStatus::Faulted(cause)) => (b" fault scause=", cause),
            Some(ChildStatus::Cancelled) => return print_line(&[name, b" cancelled"]),
            None => return print(b"no pending launch\r\n"),
        };
        print_line(&[name, status, &hex(value)]);
    }
}

fn agent_run_args(
    arguments: &[u8],
) -> Option<(GoalOptions<'_>, Program, Option<agent::RunRequest<'_>>)> {
    let (options, program, rest) = goal_program(arguments, GoalCommand::AgentRun)?;
    let mut request = if rest.is_empty() {
        None
    } else {
        let (flag, args) = split_word(rest);
        if program != Program::Hello {
            return None;
        }
        Some(match GoalFlag::parse(flag) {
            GoalFlag::Delegate => {
                agent::RunRequest::DelegateWithFile(Helper::parse(args)?.file_request()?)
            }
            option @ (GoalFlag::Mock | GoalFlag::Plan) => {
                let definition = greeting_definition(args)?;
                let live = matches!(
                    definition,
                    agent::FIND_GREETING_V6 | agent::FIND_GREETING_V7 | agent::FIND_GREETING_V8
                );
                if matches!(option, GoalFlag::Plan) != live {
                    return None;
                }
                agent::RunRequest::Definition(definition)
            }
            GoalFlag::StageRename => {
                let (first, rest) = split_literal(args)?;
                if rest.is_empty() {
                    agent::RunRequest::StageRename(first)
                } else {
                    let (flag, args) = split_word(rest);
                    if !matches!(GoalFlag::parse(flag), GoalFlag::StageRename) {
                        return None;
                    }
                    let (second, b"") = split_literal(args)? else {
                        return None;
                    };
                    agent::RunRequest::StageRenamePair([first, second])
                }
            }
            GoalFlag::HttpsProbe if args.is_empty() => agent::RunRequest::HttpsProbe,
            GoalFlag::McpInit if args.is_empty() => agent::RunRequest::McpInit,
            GoalFlag::McpList if args.is_empty() => agent::RunRequest::McpList,
            GoalFlag::McpCall if args.is_empty() => agent::RunRequest::McpCall,
            _ => return None,
        })
    };
    if options.checkpoint {
        let Some(agent::RunRequest::Definition(id)) = request else {
            return None;
        };
        if !matches!(id, agent::FIND_GREETING_V2 | agent::FIND_GREETING_V7)
            || (id == agent::FIND_GREETING_V7 && options.suspended)
        {
            return None;
        }
        request = Some(agent::RunRequest::Checkpoint(id));
    }
    Some((options, program, request))
}

fn greeting_definition(name: &[u8]) -> Option<agent::DefinitionId> {
    let default = agent::FIND_GREETING_V1;
    let suffix = name.strip_prefix(default.intent.name().as_bytes())?;
    let revision = match suffix {
        b"" => default.revision,
        [b'@', digits @ ..] => u8::try_from(super::number(digits)?).ok()?,
        _ => return None,
    };
    Some(agent::DefinitionId {
        revision,
        ..default
    })
}

fn goal_program(
    arguments: &[u8],
    command: GoalCommand,
) -> Option<(GoalOptions<'_>, Program, &[u8])> {
    let (options, rest) = GoalOptions::parse(arguments, command)?;
    let (name, rest) = split_word(rest);
    Some((options, Program::named(name)?, rest))
}

fn request_kind(arguments: &[u8], default: RequestKind) -> (RequestKind, &[u8]) {
    match split_word(arguments) {
        (b"--echo", rest) => (RequestKind::Echo, rest),
        (b"--read", rest) => (RequestKind::ReadFile, rest),
        _ => (default, arguments),
    }
}

fn grep_pattern(arguments: &[u8]) -> Option<(&[u8], &[u8])> {
    let (b"-F", rest) = split_word(arguments) else {
        return None;
    };
    split_literal(rest)
}

fn spawn_scoped(
    program: Program,
    path: &[u8],
    spawn: impl FnOnce(usize, usize) -> Option<usize>,
) -> Option<usize> {
    let Some(file) = OpenHandle::open(path, OPEN_READ) else {
        print(b"cannot open path\r\n");
        return None;
    };
    let handle = spawn(program as usize, file.id());
    if handle.is_none() {
        print_line(&[b"cannot run scoped ", program.name().as_bytes()]);
    }
    handle
}

fn receive_report(
    handle: usize,
    status: ChildStatus,
    expected: ReplyKind,
    buffer: &mut [u8],
) -> Option<&[u8]> {
    let reply = clv::receive_reply(handle, buffer);
    if matches!(status, ChildStatus::Exited(0))
        && let Some(ChildReply::Received { kind, bytes, .. }) = reply
        && kind == expected
    {
        Some(bytes)
    } else {
        None
    }
}

fn print_reply(handle: usize) {
    if handle == NO_CHILD_HANDLE {
        return print(b"no message\r\n");
    }
    let mut buffer = [0; MESSAGE_MAX_BYTES];
    match clv::receive_reply(handle, &mut buffer) {
        Some(ChildReply::Empty) => print(b"no message\r\n"),
        Some(ChildReply::Received { kind, bytes, .. }) => display_reply(handle, kind, bytes),
        None => print(b"recv failed\r\n"),
    }
}

pub(super) fn display_reply(handle: usize, kind: ReplyKind, bytes: &[u8]) {
    match kind {
        ReplyKind::FileData if bytes.is_empty() => print(b"empty file\r\n"),
        ReplyKind::Matches if bytes.is_empty() => {}
        ReplyKind::Echo | ReplyKind::FileData | ReplyKind::Matches => {
            clv::write_console(bytes);
            if kind == ReplyKind::Echo || !bytes.ends_with(b"\n") {
                print(b"\r\n");
            }
        }
        ReplyKind::Denied => print(b"read denied: no directory grant\r\n"),
        ReplyKind::ReadFailed => print(b"cannot read requested file\r\n"),
        ReplyKind::ResultTooLarge => print(b"grep result exceeds 64 bytes\r\n"),
        ReplyKind::DelegationFailed => print(b"cannot delegate child\r\n"),
        ReplyKind::HttpsFailed
        | ReplyKind::McpInitFailed
        | ReplyKind::McpListFailed
        | ReplyKind::McpCallFailed => {
            use clv_abi::net::HttpsStatus::{self, *};
            let status = bytes
                .first()
                .and_then(|&byte| HttpsStatus::from_raw(byte.into()));
            let operation: &[u8] = match kind {
                ReplyKind::McpInitFailed => b"MCP init",
                ReplyKind::McpListFailed => b"MCP list",
                ReplyKind::McpCallFailed => b"MCP call",
                _ => b"HTTPS probe",
            };
            let reason: &[u8] = match status {
                Some(Denied) => b" grant required",
                Some(Unavailable) => b" unavailable",
                Some(TimedOut) => b" timed out",
                Some(DnsFailed) => b" DNS lookup failed",
                Some(AddressDenied) => b" destination denied",
                Some(TlsFailed) => b" TLS failed",
                _ => b" invalid response",
            };
            print_line(&[operation, reason]);
        }
        ReplyKind::HostPlanFailed | ReplyKind::HostToolFailed => {
            use BridgeFailure::*;
            use ReplyKind::HostToolFailed;

            let reason = bytes
                .first()
                .and_then(|&byte| BridgeFailure::from_raw(byte.into()));
            print(match (kind, reason) {
                (HostToolFailed, Some(ToolNotAllowed)) => b"host tool grant required\r\n",
                (HostToolFailed, Some(Unavailable)) => b"host MCP tool unavailable\r\n",
                (HostToolFailed, Some(TimedOut)) => b"host MCP tool timed out\r\n",
                (HostToolFailed, Some(BudgetExhausted)) => b"host tool budget exhausted\r\n",
                (HostToolFailed, _) => b"invalid host tool reply\r\n",
                (_, Some(Unavailable)) => b"host bridge unavailable\r\n",
                (_, Some(TimedOut)) => b"host plan timed out\r\n",
                (_, Some(BudgetExhausted)) => b"host plan budget exhausted\r\n",
                (_, Some(Skipped)) => b"host plan skipped the search\r\n",
                (_, Some(ModelUnavailable)) => b"host model unavailable\r\n",
                (_, Some(ToolNotAllowed)) => b"plan tool grants required\r\n",
                _ => b"invalid host plan reply\r\n",
            });
        }
        ReplyKind::GoalObservation => super::goal::print_observation(bytes),
        ReplyKind::PublicationObserved => super::goal::print_publication_observation(bytes),
        ReplyKind::ReadDenialAlert => super::goal::print_read_denial_alert(handle, bytes),
        ReplyKind::AgentResumed => super::goal::print_resume_receipt(bytes),
        ReplyKind::RenameReviewed => {
            if let Some(receipt) = clv::goal::RenameReviewReceipt::decode(bytes) {
                super::goal::print_reviewer(&receipt);
                super::goal::print_review(receipt.review);
            } else {
                print(b"invalid rename review receipt\r\n");
            }
        }
    }
}

fn print_echo_stream(handle: usize, mut input: &[u8]) -> Option<()> {
    let mut buffer = [0; PIPE_CAPACITY];
    while !input.is_empty() {
        let mut remaining = pipe::write(handle, input)?;
        if remaining == 0 {
            return None;
        }
        input = input.get(remaining..)?;
        // Read the submitted bytes before filling stdin again.
        while remaining > 0 {
            let bytes = pipe::read(handle, buffer.get_mut(..remaining)?)?;
            if bytes.is_empty() || clv::write_console(bytes) != Some(bytes.len()) {
                return None;
            }
            remaining -= bytes.len();
        }
    }
    Some(())
}

fn print_stream(handle: usize) {
    let mut buffer = [0; PIPE_CAPACITY];
    let ok = loop {
        match pipe::read(handle, &mut buffer) {
            Some([]) => break true,
            Some(bytes) if clv::write_console(bytes) == Some(bytes.len()) => {}
            _ => break false,
        }
    };
    pipe::close(handle);
    if !ok {
        print(b"cannot read file stream\r\n");
    }
}
