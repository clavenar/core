const HANDLE_COUNT: usize = 4;
const WRITABLE_FILE_COUNT: usize = 4;
const FILE_NAME_LIMIT: usize = 64;
pub(crate) const WRITABLE_FILE_CAPACITY: usize = 256;

struct BootFile {
    name: &'static [u8],
    contents: &'static [u8],
}

const BOOT_FILES: [BootFile; 2] = [
    BootFile {
        name: b"hello.txt",
        contents: b"Hello from RamFS\r\n",
    },
    BootFile {
        name: b"about.txt",
        contents:
            b"Clavenar is a Rust operating system for bounded, capability-based autonomy.\r\n",
    },
];

#[derive(Clone, Copy)]
enum Node {
    Root,
    BootFile(usize),
    WritableFile(usize),
}

struct WritableFile {
    name: [u8; FILE_NAME_LIMIT],
    name_length: usize,
    contents: [u8; WRITABLE_FILE_CAPACITY],
    length: usize,
}

impl WritableFile {
    fn new(name: &[u8]) -> Option<Self> {
        if name.is_empty()
            || name == b"."
            || name == b".."
            || name.iter().any(|&byte| byte == b'/' || byte == 0)
        {
            return None;
        }
        let mut bytes = [0; FILE_NAME_LIMIT];
        bytes.get_mut(..name.len())?.copy_from_slice(name);
        Some(Self {
            name: bytes,
            name_length: name.len(),
            contents: [0; WRITABLE_FILE_CAPACITY],
            length: 0,
        })
    }

    fn name(&self) -> &[u8] {
        &self.name[..self.name_length]
    }
}

pub(crate) struct RamFs {
    writable: [Option<WritableFile>; WRITABLE_FILE_COUNT],
}

impl RamFs {
    pub(crate) const fn new() -> Self {
        Self {
            writable: [const { None }; WRITABLE_FILE_COUNT],
        }
    }

    fn open(&mut self, path: &[u8], writable: bool) -> Option<Node> {
        if path == b"/" {
            return (!writable).then_some(Node::Root);
        }
        let name = path.strip_prefix(b"/").unwrap_or(path);
        if let Some(index) = BOOT_FILES.iter().position(|file| file.name == name) {
            return (!writable).then_some(Node::BootFile(index));
        }
        if let Some(index) = self
            .writable
            .iter()
            .position(|slot| slot.as_ref().is_some_and(|file| file.name() == name))
        {
            if writable {
                let file = self.writable[index].as_mut()?;
                file.contents.fill(0);
                file.length = 0;
            }
            return Some(Node::WritableFile(index));
        }
        if !writable {
            return None;
        }

        let index = self.writable.iter().position(Option::is_none)?;
        self.writable[index] = Some(WritableFile::new(name)?);
        Some(Node::WritableFile(index))
    }

    fn contents(&self, node: Node) -> Option<&[u8]> {
        match node {
            Node::Root => None,
            Node::BootFile(index) => Some(BOOT_FILES[index].contents),
            Node::WritableFile(index) => {
                let file = self.writable[index].as_ref()?;
                Some(&file.contents[..file.length])
            }
        }
    }

    fn entry_name(&self, index: usize) -> Option<&[u8]> {
        if let Some(file) = BOOT_FILES.get(index) {
            return Some(file.name);
        }
        self.writable
            .iter()
            .flatten()
            .nth(index - BOOT_FILES.len())
            .map(WritableFile::name)
    }
}

#[derive(Clone, Copy)]
struct Handle {
    node: Node,
    cursor: usize,
    writable: bool,
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

    pub(crate) fn open(&mut self, fs: &mut RamFs, path: &[u8], writable: bool) -> Option<usize> {
        let fd = self.slots.iter().position(Option::is_none)?;
        let node = fs.open(path, writable)?;
        self.slots[fd] = Some(Handle {
            node,
            cursor: 0,
            writable,
        });
        Some(fd)
    }

    pub(crate) fn read(
        &mut self,
        fs: &RamFs,
        fd: usize,
        length: usize,
        deliver: impl FnOnce(&[u8]) -> bool,
    ) -> Option<usize> {
        let handle = self.slots.get_mut(fd)?.as_mut()?;
        // Another handle can truncate the file below this cursor.
        let remaining = fs
            .contents(handle.node)?
            .get(handle.cursor..)
            .unwrap_or(&[]);
        let bytes = &remaining[..remaining.len().min(length)];
        if !deliver(bytes) {
            return None;
        }
        handle.cursor += bytes.len();
        Some(bytes.len())
    }

    pub(crate) fn write(&mut self, fs: &mut RamFs, fd: usize, bytes: &[u8]) -> Option<usize> {
        let handle = self.slots.get_mut(fd)?.as_mut()?;
        if !handle.writable {
            return None;
        }
        let Node::WritableFile(index) = handle.node else {
            return None;
        };
        if bytes.is_empty() {
            return Some(0);
        }
        let file = fs.writable[index].as_mut()?;
        let end = handle.cursor.checked_add(bytes.len())?;
        file.contents
            .get_mut(handle.cursor..end)?
            .copy_from_slice(bytes);
        file.length = file.length.max(end);
        handle.cursor = end;
        Some(bytes.len())
    }

    pub(crate) fn readdir(
        &mut self,
        fs: &RamFs,
        fd: usize,
        deliver: impl FnOnce(&[u8]) -> bool,
    ) -> Option<usize> {
        let handle = self.slots.get_mut(fd)?.as_mut()?;
        if !matches!(handle.node, Node::Root) {
            return None;
        }
        let name = fs.entry_name(handle.cursor).unwrap_or(&[]);
        if !deliver(name) {
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
