mod handles;

pub(crate) use handles::{FileGrant, GrantArgument, TaskFs};

const WRITABLE_FILE_COUNT: usize = 4;
const DIRECTORY_COUNT: usize = 4;
const NAME_LIMIT: usize = 64;
const CWD_PATH_CAPACITY: usize = DIRECTORY_COUNT * (NAME_LIMIT + 1);
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

#[derive(Clone, Copy, PartialEq, Eq)]
enum DirectoryId {
    Root,
    Child(usize),
}

enum PathBase {
    WorkingDirectory(DirectoryId),
    Confined(DirectoryId),
}

#[derive(Clone, Copy)]
enum Node {
    Directory(DirectoryId),
    BootFile(usize),
    WritableFile(usize),
}

enum ResolvedPath<'a> {
    Directory(DirectoryId),
    Entry(DirectoryId, &'a [u8]),
}

struct Name {
    bytes: [u8; NAME_LIMIT],
    length: usize,
}

impl Name {
    fn new(name: &[u8]) -> Option<Self> {
        if name.is_empty()
            || name == b"."
            || name == b".."
            || name.iter().any(|&byte| byte == b'/' || byte == 0)
        {
            return None;
        }
        let mut bytes = [0; NAME_LIMIT];
        bytes.get_mut(..name.len())?.copy_from_slice(name);
        Some(Self {
            bytes,
            length: name.len(),
        })
    }

    fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.length]
    }
}

struct Directory {
    parent: DirectoryId,
    name: Name,
}

struct WritableFile {
    parent: DirectoryId,
    name: Name,
    contents: [u8; WRITABLE_FILE_CAPACITY],
    length: usize,
    linked: bool,
    open_handles: usize,
}

impl WritableFile {
    fn new(parent: DirectoryId, name: &[u8]) -> Option<Self> {
        Some(Self {
            parent,
            name: Name::new(name)?,
            contents: [0; WRITABLE_FILE_CAPACITY],
            length: 0,
            linked: true,
            open_handles: 0,
        })
    }
}

pub(crate) struct RamFs {
    directories: [Option<Directory>; DIRECTORY_COUNT],
    writable: [Option<WritableFile>; WRITABLE_FILE_COUNT],
    next_handle_id: usize,
}

impl RamFs {
    pub(crate) const fn new() -> Self {
        Self {
            directories: [const { None }; DIRECTORY_COUNT],
            writable: [const { None }; WRITABLE_FILE_COUNT],
            next_handle_id: 0,
        }
    }

    fn open(&mut self, base: PathBase, path: &[u8], writable: bool) -> Option<Node> {
        let (parent, name) = match self.resolve_path(base, path)? {
            ResolvedPath::Directory(directory) => {
                return (!writable).then_some(Node::Directory(directory));
            }
            ResolvedPath::Entry(parent, name) => (parent, name),
        };
        let requires_directory = path.ends_with(b"/");
        if let Some(node) = self.lookup(parent, name) {
            return match node {
                Node::Directory(_) if !writable => Some(node),
                Node::BootFile(_) if !writable && !requires_directory => Some(node),
                Node::WritableFile(index) if !requires_directory => {
                    let file = self.writable[index].as_mut()?;
                    if writable {
                        file.contents.fill(0);
                        file.length = 0;
                    }
                    Some(node)
                }
                _ => None,
            };
        }
        if !writable || requires_directory {
            return None;
        }

        let index = self.writable.iter().position(Option::is_none)?;
        self.writable[index] = Some(WritableFile::new(parent, name)?);
        Some(Node::WritableFile(index))
    }

    fn mkdir(&mut self, cwd: DirectoryId, path: &[u8]) -> Option<usize> {
        let ResolvedPath::Entry(parent, name) =
            self.resolve_path(PathBase::WorkingDirectory(cwd), path)?
        else {
            return None;
        };
        if self.lookup(parent, name).is_some() {
            return None;
        }
        let index = self.directories.iter().position(Option::is_none)?;
        self.directories[index] = Some(Directory {
            parent,
            name: Name::new(name)?,
        });
        Some(0)
    }

    fn rmdir(
        &mut self,
        cwd: DirectoryId,
        path: &[u8],
        in_use: impl FnOnce(DirectoryId) -> bool,
    ) -> Option<usize> {
        let ResolvedPath::Entry(parent, name) =
            self.resolve_path(PathBase::WorkingDirectory(cwd), path)?
        else {
            return None;
        };
        let Node::Directory(directory @ DirectoryId::Child(index)) = self.lookup(parent, name)?
        else {
            return None;
        };
        if self.next_entry(directory, 0).is_some() || in_use(directory) {
            return None;
        }
        self.directories[index] = None;
        Some(0)
    }

    fn unlink(&mut self, cwd: DirectoryId, path: &[u8]) -> Option<usize> {
        if path.ends_with(b"/") {
            return None;
        }
        let ResolvedPath::Entry(parent, name) =
            self.resolve_path(PathBase::WorkingDirectory(cwd), path)?
        else {
            return None;
        };
        let Node::WritableFile(index) = self.lookup(parent, name)? else {
            return None;
        };
        let slot = &mut self.writable[index];
        let file = slot.as_mut()?;
        if file.open_handles == 0 {
            *slot = None;
        } else {
            file.linked = false;
        }
        Some(0)
    }

    fn absolute_path(
        &self,
        mut directory: DirectoryId,
        deliver: impl FnOnce(&[u8]) -> bool,
    ) -> Option<usize> {
        let mut buffer = [0; CWD_PATH_CAPACITY];
        let mut start = buffer.len();
        while let DirectoryId::Child(index) = directory {
            let entry = self.directories[index].as_ref()?;
            let name = entry.name.as_bytes();
            let end = start;
            start = start.checked_sub(name.len() + 1)?;
            buffer[start] = b'/';
            buffer[start + 1..end].copy_from_slice(name);
            directory = entry.parent;
        }
        if start == buffer.len() {
            start -= 1;
            buffer[start] = b'/';
        }
        let path = &buffer[start..];
        deliver(path).then_some(path.len())
    }

    fn resolve_directory(&self, cwd: DirectoryId, path: &[u8]) -> Option<DirectoryId> {
        match self.resolve_path(PathBase::WorkingDirectory(cwd), path)? {
            ResolvedPath::Directory(directory) => Some(directory),
            ResolvedPath::Entry(parent, name) => match self.lookup(parent, name)? {
                Node::Directory(directory) => Some(directory),
                _ => None,
            },
        }
    }

    fn resolve_path<'a>(&self, base: PathBase, path: &'a [u8]) -> Option<ResolvedPath<'a>> {
        if path.is_empty() {
            return None;
        }
        let absolute = path.starts_with(b"/");
        let (mut directory, boundary) = match base {
            PathBase::WorkingDirectory(_) if absolute => (DirectoryId::Root, None),
            PathBase::WorkingDirectory(directory) => (directory, None),
            PathBase::Confined(directory) if !absolute => (directory, Some(directory)),
            PathBase::Confined(_) => return None,
        };
        let mut components = path
            .split(|&byte| byte == b'/')
            .filter(|part| !part.is_empty())
            .peekable();
        while let Some(component) = components.next() {
            match component {
                b"." => {}
                b".." => {
                    if boundary == Some(directory) {
                        return None;
                    }
                    if let DirectoryId::Child(index) = directory {
                        directory = self.directories[index].as_ref()?.parent;
                    }
                }
                name if components.peek().is_none() => {
                    return Some(ResolvedPath::Entry(directory, name));
                }
                name => {
                    let Node::Directory(next) = self.lookup(directory, name)? else {
                        return None;
                    };
                    directory = next;
                }
            }
        }
        Some(ResolvedPath::Directory(directory))
    }

    fn lookup(&self, parent: DirectoryId, name: &[u8]) -> Option<Node> {
        if parent == DirectoryId::Root {
            if let Some(index) = BOOT_FILES.iter().position(|file| file.name == name) {
                return Some(Node::BootFile(index));
            }
        }
        if let Some(index) = self.directories.iter().position(|slot| {
            slot.as_ref()
                .is_some_and(|dir| dir.parent == parent && dir.name.as_bytes() == name)
        }) {
            return Some(Node::Directory(DirectoryId::Child(index)));
        }
        self.writable
            .iter()
            .position(|slot| {
                slot.as_ref().is_some_and(|file| {
                    file.linked && file.parent == parent && file.name.as_bytes() == name
                })
            })
            .map(Node::WritableFile)
    }

    fn retain(&mut self, node: Node) {
        if let Node::WritableFile(index) = node {
            let file = self.writable[index]
                .as_mut()
                .expect("open handle refers to a writable file");
            file.open_handles += 1;
        }
    }

    fn release(&mut self, node: Node) {
        if let Node::WritableFile(index) = node {
            let slot = &mut self.writable[index];
            let file = slot
                .as_mut()
                .expect("open handle refers to a writable file");
            file.open_handles -= 1;
            if file.open_handles == 0 && !file.linked {
                *slot = None;
            }
        }
    }

    fn contents(&self, node: Node) -> Option<&[u8]> {
        match node {
            Node::Directory(_) => None,
            Node::BootFile(index) => Some(BOOT_FILES[index].contents),
            Node::WritableFile(index) => {
                let file = self.writable[index].as_ref()?;
                Some(&file.contents[..file.length])
            }
        }
    }

    fn write_at(&mut self, node: Node, offset: usize, bytes: &[u8]) -> Option<usize> {
        let Node::WritableFile(index) = node else {
            return None;
        };
        if bytes.is_empty() {
            return Some(0);
        }
        let file = self.writable[index].as_mut()?;
        let end = offset.checked_add(bytes.len())?;
        file.contents.get_mut(offset..end)?.copy_from_slice(bytes);
        file.length = file.length.max(end);
        Some(bytes.len())
    }

    fn next_entry(&self, parent: DirectoryId, cursor: usize) -> Option<(usize, &[u8])> {
        let boot_names = BOOT_FILES
            .iter()
            .map(|file| (parent == DirectoryId::Root).then_some(file.name));
        let file_names = self.writable.iter().map(|slot| {
            let file = slot
                .as_ref()
                .filter(|file| file.linked && file.parent == parent)?;
            Some(file.name.as_bytes())
        });
        let directory_names = self.directories.iter().map(|slot| {
            let dir = slot.as_ref().filter(|dir| dir.parent == parent)?;
            Some(dir.name.as_bytes())
        });
        // Keep cursor positions tied to slots across removals.
        boot_names
            .chain(file_names)
            .chain(directory_names)
            .enumerate()
            .skip(cursor)
            .find_map(|(index, name)| name.map(|name| (index + 1, name)))
    }
}
