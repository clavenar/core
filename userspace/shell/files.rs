use clv::{fs, print};
use clv_abi::{OPEN_READ, OPEN_WRITE_TRUNCATE};

use super::{LINE_LIMIT, help::Entry, split_word};

/// Owns one RamFS descriptor in the shell's handle table until it leaves scope.
pub(super) struct OpenHandle(usize);

impl OpenHandle {
    pub(super) fn open(path: &[u8], mode: usize) -> Option<Self> {
        fs::open(path, mode).map(Self)
    }

    pub(super) fn id(&self) -> usize {
        self.0
    }
}

impl Drop for OpenHandle {
    fn drop(&mut self) {
        fs::close(self.0);
    }
}

pub(super) fn cd(path: &[u8]) {
    path_command(fs::chdir, path, Entry::Cd, b"cannot change directory\r\n");
}

pub(super) fn mkdir(path: &[u8]) {
    path_command(
        fs::mkdir,
        path,
        Entry::Mkdir,
        b"cannot create directory\r\n",
    );
}

pub(super) fn rmdir(path: &[u8]) {
    path_command(
        fs::rmdir,
        path,
        Entry::Rmdir,
        b"cannot remove directory\r\n",
    );
}

pub(super) fn rm(path: &[u8]) {
    path_command(fs::unlink, path, Entry::Rm, b"cannot remove path\r\n");
}

fn path_command(operation: fn(&[u8]) -> bool, path: &[u8], entry: Entry, error: &[u8]) {
    if path.is_empty() {
        entry.usage();
    } else if !operation(path) {
        print(error);
    }
}

pub(super) fn pwd() {
    // The absolute path can span several separately entered components.
    let mut buffer = [0; 512];
    if let Some(path) = fs::getcwd(&mut buffer) {
        print(path);
        print(b"\r\n");
    } else {
        print(b"cannot get current directory\r\n");
    }
}

pub(super) fn ls(path: &[u8]) {
    show(if path.is_empty() { b"." } else { path }, true);
}

pub(super) fn cat(path: &[u8]) {
    if path.is_empty() {
        Entry::Cat.usage();
    } else {
        show(path, false);
    }
}

fn show(path: &[u8], list_entries: bool) {
    let Some(handle) = OpenHandle::open(path, OPEN_READ) else {
        return print(b"cannot open path\r\n");
    };
    let mut buffer = [0; 64];
    let read = if list_entries { fs::readdir } else { fs::read };
    loop {
        match read(handle.id(), &mut buffer) {
            Some([]) => break,
            Some(bytes) => {
                clv::write_console(bytes);
                if list_entries {
                    print(b"\r\n");
                }
            }
            None => {
                print(b"cannot read path\r\n");
                break;
            }
        }
    }
}

pub(super) fn write_file(arguments: &[u8]) {
    let (path, text) = split_word(arguments);
    if text.is_empty() {
        return Entry::Write.usage();
    }
    let mut buffer = [0; LINE_LIMIT + 2];
    buffer[..text.len()].copy_from_slice(text);
    let length = text.len() + 2;
    buffer[text.len()..length].copy_from_slice(b"\r\n");
    let Some(handle) = OpenHandle::open(path, OPEN_WRITE_TRUNCATE) else {
        return print(b"cannot open path\r\n");
    };
    if fs::write(handle.id(), &buffer[..length]) != Some(length) {
        print(b"cannot write path\r\n");
    }
}
