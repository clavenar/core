use crate::memory::usercopy::{copy_from_user_stack, copy_to_user_stack};

const HANDLE_COUNT: usize = 4;
const PATH_LIMIT: usize = 64;

struct File {
    name: &'static [u8],
    contents: &'static [u8],
}

const FILES: [File; 2] = [
    File {
        name: b"hello.txt",
        contents: b"Hello from RamFS\r\n",
    },
    File {
        name: b"about.txt",
        contents:
            b"Clavenar is a Rust operating system for bounded, capability-based autonomy.\r\n",
    },
];

#[derive(Clone, Copy)]
enum Node {
    Root,
    File(usize),
}

#[derive(Clone, Copy)]
struct Handle {
    node: Node,
    cursor: usize,
}

pub(crate) struct FileHandles {
    slots: [Option<Handle>; HANDLE_COUNT],
}

impl FileHandles {
    pub(crate) const fn new() -> Self {
        Self {
            slots: [None; HANDLE_COUNT],
        }
    }

    pub(crate) fn open(&mut self, address: usize, length: usize) -> Option<usize> {
        let mut buffer = [0; PATH_LIMIT];
        let path = buffer.get_mut(..length)?;
        if !copy_from_user_stack(address, path) {
            return None;
        }

        let node = if path == b"/" {
            Node::Root
        } else {
            let name = path.strip_prefix(b"/").unwrap_or(path);
            Node::File(FILES.iter().position(|file| file.name == name)?)
        };
        let fd = self.slots.iter().position(Option::is_none)?;
        self.slots[fd] = Some(Handle { node, cursor: 0 });
        Some(fd)
    }

    pub(crate) fn read(&mut self, fd: usize, address: usize, length: usize) -> Option<usize> {
        let handle = self.slots.get_mut(fd)?.as_mut()?;
        let Node::File(index) = handle.node else {
            return None;
        };
        let remaining = &FILES[index].contents[handle.cursor..];
        let bytes = &remaining[..remaining.len().min(length)];
        if !copy_to_user_stack(bytes, address, length) {
            return None;
        }
        handle.cursor += bytes.len();
        Some(bytes.len())
    }

    pub(crate) fn readdir(&mut self, fd: usize, address: usize, length: usize) -> Option<usize> {
        let handle = self.slots.get_mut(fd)?.as_mut()?;
        if !matches!(handle.node, Node::Root) {
            return None;
        }
        let name = FILES.get(handle.cursor).map_or(&[][..], |file| file.name);
        if !copy_to_user_stack(name, address, length) {
            return None;
        }
        if !name.is_empty() {
            handle.cursor += 1;
        }
        Some(name.len())
    }

    pub(crate) fn close(&mut self, fd: usize) -> Option<usize> {
        self.slots.get_mut(fd)?.take()?;
        Some(0)
    }
}
