use clv::{ChildReply, ChildStatus, agent, fs, pipe, print};
use clv_abi::{
    ChildEvent, CompletionKind, MESSAGE_MAX_BYTES, NO_CHILD_HANDLE, OPEN_READ, PIPE_CAPACITY,
    PROGRAM_HELLO, PROGRAMS, Program, ReplyKind, RequestKind,
};

use super::{
    goal::{GoalCommand, GoalOptions},
    hex, print_handle, print_line, split_word,
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
            _ => print(b"usage: run <program> or run hello <dir>\r\n"),
        }
    }

    pub(super) fn run_in_goal(&mut self, goal: Option<usize>, arguments: &[u8]) {
        let Some((options, program, b"")) = goal_program(arguments, GoalCommand::Run) else {
            return print(b"usage: goal run [--reply-bytes N] <program>\r\n");
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

    pub(super) fn run_agent(
        &mut self,
        goal: usize,
        arguments: &[u8],
    ) -> Option<agent::AgentLaunch> {
        let Some((options, program, request)) = agent_run_args(arguments) else {
            print(b"usage: agent run [--suspended] [--reply-bytes N] <program> [--delegate --with-file --reply-bytes N grep -F <pattern>]\r\n");
            return None;
        };
        let result = match request {
            Some(request) => agent::run_with_request(
                goal,
                program as usize,
                options.suspended,
                options.reply_bytes,
                &request,
            ),
            None => agent::run_with_options(
                goal,
                program as usize,
                options.suspended,
                options.reply_bytes,
            ),
        };
        let Some(launch) = result else {
            print(b"cannot start agent\r\n");
            return None;
        };
        self.handles[program as usize] = launch.child;
        Some(launch)
    }

    pub(super) fn delegate(&mut self, agent: Option<usize>, arguments: &[u8]) {
        const USAGE: &[u8] =
            b"usage: agent delegate [--with-file] --reply-bytes N <program> [-F <pattern>]\r\n";
        let Some(helper) = Helper::parse(arguments) else {
            return print(USAGE);
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
            return print(b"usage: grep -F <pattern> <path>\r\n");
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
            _ => return print(b"usage: ask [--read] <program> <text>\r\n"),
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
            return print(b"usage: askat [--echo] hello <dir> <payload>\r\n");
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
            return print(b"usage: askfile [--stream] hello <path>\r\n");
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
            None => print(b"usage: recv <program>\r\n"),
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
            return print(b"usage: wait <program>\r\n");
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
) -> Option<(GoalOptions, Program, Option<agent::FileDelegation<'_>>)> {
    let (options, program, rest) = goal_program(arguments, GoalCommand::AgentRun)?;
    let request = match (program, split_word(rest)) {
        (_, (b"", _)) => None,
        (Program::Hello, (b"--delegate", args)) => Some(Helper::parse(args)?.file_request()?),
        _ => return None,
    };
    Some((options, program, request))
}

fn goal_program(arguments: &[u8], command: GoalCommand) -> Option<(GoalOptions, Program, &[u8])> {
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

/// Quotes allow spaces and the empty pattern across all grep launch paths.
fn grep_pattern(arguments: &[u8]) -> Option<(&[u8], &[u8])> {
    let (b"-F", rest) = split_word(arguments) else {
        return None;
    };
    let (&first, tail) = rest.split_first()?;
    if first != b'\'' && first != b'"' {
        return Some(split_word(rest));
    }
    let end = tail.iter().position(|&byte| byte == first)?;
    let (pattern, rest) = tail.split_at(end);
    let rest = &rest[1..];
    (rest.is_empty() || rest.starts_with(b" ")).then(|| (pattern, rest.trim_ascii_start()))
}

fn spawn_scoped(
    program: Program,
    path: &[u8],
    spawn: impl FnOnce(usize, usize) -> Option<usize>,
) -> Option<usize> {
    let Some(fd) = fs::open(path, OPEN_READ) else {
        print(b"cannot open path\r\n");
        return None;
    };
    let handle = spawn(program as usize, fd);
    fs::close(fd);
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
