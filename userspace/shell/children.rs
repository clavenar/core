use clv::{ChildReply, ChildStatus, agent, fs, pipe, print};
use clv_abi::{
    ChildEvent, CompletionKind, MESSAGE_MAX_BYTES, NO_CHILD_HANDLE, OPEN_READ, PIPE_CAPACITY,
    PROGRAM_ECHO, PROGRAM_HELLO, Program, ReplyKind, RequestKind,
};

use super::{
    goal::{GoalCommand, GoalOptions},
    hex, print_handle, print_line, split_word,
};

struct Child {
    name: &'static [u8],
    selector: usize,
    handle: usize,
}

impl Child {
    fn collect_reply(&mut self, handle: usize) {
        self.handle = handle;
        print_reply(handle);
    }
}

pub(super) struct Children {
    programs: [Child; 2],
}

impl Children {
    pub(super) fn new() -> Self {
        Self {
            programs: [
                Child {
                    name: b"hello",
                    selector: PROGRAM_HELLO,
                    handle: NO_CHILD_HANDLE,
                },
                Child {
                    name: b"echo",
                    selector: PROGRAM_ECHO,
                    handle: NO_CHILD_HANDLE,
                },
            ],
        }
    }

    fn named(&mut self, name: &[u8]) -> Option<&mut Child> {
        self.programs.iter_mut().find(|child| child.name == name)
    }

    pub(super) fn program(&self, name: &[u8]) -> Option<Program> {
        self.programs
            .iter()
            .find(|child| child.name == name)
            .and_then(|child| Program::from_raw(child.selector))
    }

    pub(super) fn program_name(&self, selector: usize) -> Option<&'static [u8]> {
        self.programs
            .iter()
            .find(|child| child.selector == selector)
            .map(|child| child.name)
    }

    pub(super) fn run(&mut self, arguments: &[u8]) {
        let (name, directory) = split_word(arguments);
        match self.named(name) {
            Some(child) if directory.is_empty() => {
                if let Some(handle) = clv::spawn(child.selector, None, None) {
                    child.handle = handle;
                } else {
                    print(child.name);
                    print(b" is already running\r\n");
                }
            }
            Some(child) if child.selector == PROGRAM_HELLO => {
                if let Some(handle) =
                    spawn_scoped_hello(directory, |fd| clv::spawn(PROGRAM_HELLO, Some(fd), None))
                {
                    child.handle = handle;
                }
            }
            _ => print(b"usage: run hello [dir] or run echo\r\n"),
        }
    }

    fn goal_child(
        &mut self,
        arguments: &[u8],
        command: GoalCommand,
    ) -> Option<(&mut Child, GoalOptions)> {
        let (options, name) = GoalOptions::parse(arguments, command)?;
        Some((self.named(name)?, options))
    }

    pub(super) fn run_in_goal(&mut self, goal: Option<usize>, arguments: &[u8]) {
        let Some((child, options)) = self.goal_child(arguments, GoalCommand::Run) else {
            return print(b"usage: goal run [--reply-bytes N] <hello|echo>\r\n");
        };
        let Some(goal) = goal else {
            return print(b"no current goal\r\n");
        };
        let result = match options.reply_bytes {
            Some(bytes) => clv::goal::run_with_reply_budget(goal, child.selector, bytes),
            None => clv::goal::run(goal, child.selector),
        };
        if let Some(handle) = result {
            child.handle = handle;
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
        let Some((child, options)) = self.goal_child(arguments, GoalCommand::AgentRun) else {
            print(b"usage: agent run [--suspended] [--reply-bytes N] <hello|echo>\r\n");
            return None;
        };
        let Some(launch) =
            agent::run_with_options(goal, child.selector, options.suspended, options.reply_bytes)
        else {
            print(b"cannot start agent\r\n");
            return None;
        };
        child.handle = launch.child;
        Some(launch)
    }

    pub(super) fn delegate(&mut self, agent: Option<usize>, arguments: &[u8]) {
        let Some((
            child,
            GoalOptions {
                reply_bytes: Some(bytes),
                with_file,
                ..
            },
        )) = self.goal_child(arguments, GoalCommand::AgentDelegate)
        else {
            return print(b"usage: agent delegate [--with-file] --reply-bytes N <hello|echo>\r\n");
        };
        let Some(agent) = agent else {
            return print(b"no current agent\r\n");
        };
        let delegate = if with_file {
            agent::delegate_with_file
        } else {
            agent::delegate
        };
        if let Some(handle) = delegate(agent, child.selector, bytes) {
            child.handle = handle;
            print_handle(b"delegated child=", handle);
        } else {
            print(b"cannot delegate child\r\n");
        }
    }

    pub(super) fn ask(&mut self, arguments: &[u8]) {
        let (kind, arguments) = request_kind(arguments, RequestKind::Echo);
        let (name, text) = split_word(arguments);
        let child = match self.named(name) {
            Some(child) if !text.is_empty() => child,
            _ => return print(b"usage: ask [--read] <hello|echo> <text>\r\n"),
        };
        let Some(handle) = clv::spawn_request(child.selector, None, kind, text) else {
            print(b"cannot ask ");
            print(child.name);
            print(b"\r\n");
            return;
        };
        child.collect_reply(handle);
    }

    pub(super) fn ask_at(&mut self, arguments: &[u8]) {
        let (kind, arguments) = request_kind(arguments, RequestKind::ReadFile);
        let (name, arguments) = split_word(arguments);
        let (directory, payload) = split_word(arguments);
        let child = match self.named(name) {
            Some(child) if child.selector == PROGRAM_HELLO && !payload.is_empty() => child,
            _ => return print(b"usage: askat [--echo] hello <dir> <payload>\r\n"),
        };
        if let Some(handle) = spawn_scoped_hello(directory, |fd| {
            clv::spawn_request(PROGRAM_HELLO, Some(fd), kind, payload)
        }) {
            child.collect_reply(handle);
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
        let child = match self.named(name) {
            Some(child) if child.selector == PROGRAM_HELLO && !path.is_empty() => child,
            _ => return print(b"usage: askfile [--stream] hello <path>\r\n"),
        };
        let spawn = if stream {
            clv::spawn_with_file_pipe
        } else {
            clv::spawn_with_file
        };
        if let Some(handle) = spawn_scoped_hello(path, |fd| spawn(PROGRAM_HELLO, fd)) {
            child.handle = handle;
            if stream {
                print_stream(handle);
            } else {
                print_reply(handle);
            }
        }
    }

    pub(super) fn recv(&mut self, name: &[u8]) {
        match self.named(name) {
            Some(child) => print_reply(child.handle),
            None => print(b"usage: recv <hello|echo>\r\n"),
        }
    }

    pub(super) fn event(&self) {
        let (handle, kind) = match clv::event_wait() {
            Some(ChildEvent::Completed { handle, kind }) => (handle, kind),
            Some(ChildEvent::Empty) => return print(b"no child event\r\n"),
            None => return print(b"event wait failed\r\n"),
        };
        let Some(child) = self.programs.iter().find(|child| child.handle == handle) else {
            return print(b"unknown child handle\r\n");
        };
        let status: &[u8] = match kind {
            CompletionKind::Exited => b" exited",
            CompletionKind::Faulted => b" faulted",
            CompletionKind::Cancelled => b" cancelled",
        };
        print_line(&[child.name, status]);
    }

    pub(super) fn wait(&mut self, name: &[u8]) {
        let Some(child) = self.named(name) else {
            return print(b"usage: wait <hello|echo>\r\n");
        };
        let (status, value): (&[u8], usize) = match clv::wait(child.handle) {
            Some(ChildStatus::Exited(code)) => (b" exited ", code),
            Some(ChildStatus::Faulted(cause)) => (b" fault scause=", cause),
            Some(ChildStatus::Cancelled) => return print_line(&[child.name, b" cancelled"]),
            None => return print(b"no pending launch\r\n"),
        };
        print_line(&[child.name, status, &hex(value)]);
    }
}

fn request_kind(arguments: &[u8], default: RequestKind) -> (RequestKind, &[u8]) {
    match split_word(arguments) {
        (b"--echo", rest) => (RequestKind::Echo, rest),
        (b"--read", rest) => (RequestKind::ReadFile, rest),
        _ => (default, arguments),
    }
}

fn spawn_scoped_hello(path: &[u8], spawn: impl FnOnce(usize) -> Option<usize>) -> Option<usize> {
    let Some(fd) = fs::open(path, OPEN_READ) else {
        print(b"cannot open path\r\n");
        return None;
    };
    let handle = spawn(fd);
    fs::close(fd);
    if handle.is_none() {
        print(b"cannot run scoped hello\r\n");
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
            ReplyKind::Echo | ReplyKind::FileData => {
                clv::write_console(bytes);
                if kind == ReplyKind::Echo || !bytes.ends_with(b"\n") {
                    print(b"\r\n");
                }
            }
            ReplyKind::Denied => print(b"read denied: no directory grant\r\n"),
            ReplyKind::ReadFailed => print(b"cannot read requested file\r\n"),
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
