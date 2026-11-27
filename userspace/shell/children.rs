use clv::{ChildReply, ChildStatus, agent, pipe, print};
use clv_abi::{
    ChildEvent, CompletionKind, MESSAGE_MAX_BYTES, NO_CHILD_HANDLE, OPEN_READ, PIPE_CAPACITY,
    PROGRAM_HELLO, PROGRAMS, Program, ReplyKind, RequestKind,
};

use super::{
    command::GoalFlag,
    files::OpenHandle,
    goal::{GoalCommand, GoalOptions},
    help::Entry,
    hex, print_handle, print_line, split_literal, split_word,
};

pub(super) struct Children {
    handles: [usize; PROGRAMS.len()],
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

    pub(super) fn observe_goal(&mut self, goal: usize) {
        let Some(handle) = clv::goal::launch_observer(goal, Program::Echo) else {
            return print(b"cannot start goal observer\r\n");
        };
        self.handles[Program::Echo as usize] = handle;
        let mut buffer = [0; MESSAGE_MAX_BYTES];
        if matches!(clv::wait(handle), Some(ChildStatus::Exited(0)))
            && let Some(ChildReply::Received {
                kind: ReplyKind::GoalObservation,
                bytes,
                ..
            }) = clv::receive_reply(handle, &mut buffer)
        {
            super::goal::print_observation(bytes);
        } else {
            print(b"goal observation unavailable\r\n");
        }
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

    pub(super) fn recv(&self, name: &[u8]) {
        match Program::named(name) {
            Some(program) => print_reply(self.handles[program as usize]),
            None => Entry::Recv.usage(),
        }
    }

    pub(super) fn event(&self) {
        let (handle, kind) = match clv::event_wait() {
            Some(ChildEvent::Completed { handle, kind }) => (handle, kind),
            Some(ChildEvent::Empty) => return print(b"no child event\r\n"),
            None => return print(b"event wait failed\r\n"),
        };
        let Some(index) = self.handles.iter().position(|&saved| saved == handle) else {
            return print(b"unknown child handle\r\n");
        };
        let status: &[u8] = match kind {
            CompletionKind::Exited => b" exited",
            CompletionKind::Faulted => b" faulted",
            CompletionKind::Cancelled => b" cancelled",
        };
        print_line(&[PROGRAMS[index].1.as_bytes(), status]);
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
    let request = if rest.is_empty() {
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
            GoalFlag::Mock => agent::RunRequest::Definition(mock_definition(args)?),
            GoalFlag::StageRename => {
                let (name, b"") = split_literal(args)? else {
                    return None;
                };
                agent::RunRequest::StageRename(name)
            }
            _ => return None,
        })
    };
    Some((options, program, request))
}

fn mock_definition(name: &[u8]) -> Option<agent::DefinitionId> {
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

fn print_reply(handle: usize) {
    if handle == NO_CHILD_HANDLE {
        return print(b"no message\r\n");
    }
    let mut buffer = [0; MESSAGE_MAX_BYTES];
    match clv::receive_reply(handle, &mut buffer) {
        Some(ChildReply::Empty) => print(b"no message\r\n"),
        Some(ChildReply::Received { kind, bytes, .. }) => match kind {
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
            ReplyKind::GoalObservation => super::goal::print_observation(bytes),
        },
        None => print(b"recv failed\r\n"),
    }
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
