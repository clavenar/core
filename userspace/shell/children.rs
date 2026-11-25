use clv::{ChildStatus, fs, print};
use clv_abi::{
    FILE_REPLY_ERROR, FILE_REPLY_OK, MESSAGE_MAX_BYTES, NO_CHILD_HANDLE, OPEN_READ, PROGRAM_ECHO,
    PROGRAM_HELLO, RequestKind,
};

use super::split_word;

struct Child {
    name: &'static [u8],
    selector: usize,
    handle: usize,
}

impl Child {
    fn collect_reply(&mut self, handle: usize, kind: RequestKind) {
        self.handle = handle;
        if kind == RequestKind::Echo {
            return print_reply(handle);
        }
        let mut buffer = [0; MESSAGE_MAX_BYTES];
        match clv::receive_message(handle, &mut buffer) {
            Some(([FILE_REPLY_OK], _)) => print(b"empty file\r\n"),
            Some(([FILE_REPLY_OK, contents @ ..], _)) => {
                clv::write_console(contents);
                if !contents.ends_with(b"\n") {
                    print(b"\r\n");
                }
            }
            Some(([FILE_REPLY_ERROR], _)) => print(b"cannot read requested file\r\n"),
            Some(([], _)) => print(b"no message\r\n"),
            _ => print(b"recv failed\r\n"),
        }
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
        child.collect_reply(handle, kind);
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
            child.collect_reply(handle, kind);
        }
    }

    pub(super) fn ask_file(&mut self, arguments: &[u8]) {
        let (name, path) = split_word(arguments);
        let child = match self.named(name) {
            Some(child) if child.selector == PROGRAM_HELLO && !path.is_empty() => child,
            _ => return print(b"usage: askfile hello <path>\r\n"),
        };
        if let Some(handle) = spawn_scoped_hello(path, |fd| clv::spawn_with_file(PROGRAM_HELLO, fd))
        {
            child.collect_reply(handle, RequestKind::ReadFile);
        }
    }

    pub(super) fn recv(&mut self, name: &[u8]) {
        match self.named(name) {
            Some(child) => print_reply(child.handle),
            None => print(b"usage: recv <hello|echo>\r\n"),
        }
    }

    pub(super) fn wait(&mut self, name: &[u8]) {
        let Some(child) = self.named(name) else {
            return print(b"usage: wait <hello|echo>\r\n");
        };
        let (status, mut value): (&[u8], usize) = match clv::wait(child.handle) {
            Some(ChildStatus::Exited(code)) => (b" exited 0x", code),
            Some(ChildStatus::Faulted(cause)) => (b" fault scause=0x", cause),
            None => return print(b"no pending launch\r\n"),
        };
        print(child.name);
        print(status);
        let mut line = *b"0000000000000000\r\n";
        for digit in line[..16].iter_mut().rev() {
            *digit = b"0123456789abcdef"[value & 0xf];
            value >>= 4;
        }
        clv::write_console(&line);
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
    match clv::receive_message(handle, &mut buffer) {
        Some(([], _)) => print(b"no message\r\n"),
        Some((message, _)) => {
            clv::write_console(message);
            print(b"\r\n");
        }
        None => print(b"recv failed\r\n"),
    }
}
