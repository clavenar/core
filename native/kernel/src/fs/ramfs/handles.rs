use clv_abi::GrantKind;

use super::{DirectoryId, Node, PathBase, RamFs};

const HANDLE_COUNT: usize = 4;

struct Handle {
    id: usize,
    node: Node,
    cursor: usize,
    writable: bool,
}

impl Handle {
    fn allocate(
        fs: &mut RamFs,
        writable: bool,
        open: impl FnOnce(&mut RamFs) -> Option<Node>,
    ) -> Option<Self> {
        // Reserve usize::MAX for syscall errors. Check before opening, since
        // opening a writable file can truncate it.
        let id = fs.next_handle_id;
        let next = id.checked_add(1)?;
        let node = open(fs)?;
        fs.retain(node);
        fs.next_handle_id = next;
        Some(Self {
            id,
            node,
            cursor: 0,
            writable,
        })
    }
}

pub(crate) struct TaskFs {
    // Ordinary path lookups require a working directory.
    cwd: Option<DirectoryId>,
    slots: [Option<Handle>; HANDLE_COUNT],
}

impl TaskFs {
    pub(crate) const fn empty() -> Self {
        Self {
            cwd: None,
            slots: [const { None }; HANDLE_COUNT],
        }
    }

    pub(crate) const fn at_root() -> Self {
        Self {
            cwd: Some(DirectoryId::Root),
            ..Self::empty()
        }
    }

    pub(crate) fn grant(
        &self,
        fs: &mut RamFs,
        fd: usize,
        kind: GrantKind,
    ) -> Option<(Self, usize)> {
        let node = self.node_for_handle(fd)?;
        let node_kind = match node {
            Node::Directory(_) => GrantKind::Directory,
            Node::BootFile(_) | Node::WritableFile(_) => GrantKind::File,
        };
        if node_kind != kind {
            return None;
        }
        let handle = Handle::allocate(fs, false, |_| Some(node))?;
        let child_fd = handle.id;
        let mut child = Self::empty();
        child.slots[0] = Some(handle);
        Some((child, child_fd))
    }

    pub(crate) fn open(&mut self, fs: &mut RamFs, path: &[u8], writable: bool) -> Option<usize> {
        self.open_path(fs, PathBase::WorkingDirectory(self.cwd?), path, writable)
    }

    pub(crate) fn open_at(
        &mut self,
        fs: &mut RamFs,
        directory_fd: usize,
        path: &[u8],
    ) -> Option<usize> {
        let Node::Directory(directory) = self.node_for_handle(directory_fd)? else {
            return None;
        };
        self.open_path(fs, PathBase::Confined(directory), path, false)
    }

    fn node_for_handle(&self, fd: usize) -> Option<Node> {
        let slot = self.handle_slot(fd)?;
        Some(self.slots[slot].as_ref()?.node)
    }

    fn handle_slot(&self, fd: usize) -> Option<usize> {
        self.slots
            .iter()
            .position(|slot| slot.as_ref().is_some_and(|handle| handle.id == fd))
    }

    fn open_path(
        &mut self,
        fs: &mut RamFs,
        base: PathBase,
        path: &[u8],
        writable: bool,
    ) -> Option<usize> {
        let slot = self.slots.iter().position(Option::is_none)?;
        let handle = Handle::allocate(fs, writable, |fs| fs.open(base, path, writable))?;
        let id = handle.id;
        self.slots[slot] = Some(handle);
        Some(id)
    }

    pub(crate) fn mkdir(&self, fs: &mut RamFs, path: &[u8]) -> Option<usize> {
        fs.mkdir(self.cwd?, path)
    }

    pub(crate) fn rmdir<'a>(
        &self,
        fs: &mut RamFs,
        path: &[u8],
        mut live_task_fs: impl Iterator<Item = &'a TaskFs>,
    ) -> Option<usize> {
        fs.rmdir(self.cwd?, path, |directory| {
            live_task_fs.any(|task_fs| task_fs.uses_directory(directory))
        })
    }

    pub(crate) fn unlink(&self, fs: &mut RamFs, path: &[u8]) -> Option<usize> {
        fs.unlink(self.cwd?, path)
    }

    fn uses_directory(&self, directory: DirectoryId) -> bool {
        self.cwd == Some(directory)
            || self
                .slots
                .iter()
                .flatten()
                .any(|handle| matches!(handle.node, Node::Directory(open) if open == directory))
    }

    pub(crate) fn chdir(&mut self, fs: &RamFs, path: &[u8]) -> Option<usize> {
        self.cwd = Some(fs.resolve_directory(self.cwd?, path)?);
        Some(0)
    }

    pub(crate) fn getcwd(&self, fs: &RamFs, deliver: impl FnOnce(&[u8]) -> bool) -> Option<usize> {
        fs.absolute_path(self.cwd?, deliver)
    }

    pub(crate) fn read(
        &mut self,
        fs: &RamFs,
        fd: usize,
        length: usize,
        deliver: impl FnOnce(&[u8]) -> bool,
    ) -> Option<usize> {
        let slot = self.handle_slot(fd)?;
        let handle = self.slots[slot].as_mut()?;
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
        let slot = self.handle_slot(fd)?;
        let handle = self.slots[slot].as_mut()?;
        if !handle.writable {
            return None;
        }
        let count = fs.write_at(handle.node, handle.cursor, bytes)?;
        handle.cursor += count;
        Some(count)
    }

    pub(crate) fn readdir(
        &mut self,
        fs: &RamFs,
        fd: usize,
        deliver: impl FnOnce(&[u8]) -> bool,
    ) -> Option<usize> {
        let slot = self.handle_slot(fd)?;
        let handle = self.slots[slot].as_mut()?;
        let Node::Directory(directory) = handle.node else {
            return None;
        };
        let (next, name) = fs
            .next_entry(directory, handle.cursor)
            .unwrap_or((handle.cursor, &[]));
        if !deliver(name) {
            return None;
        }
        handle.cursor = next;
        Some(name.len())
    }

    pub(crate) fn close(&mut self, fs: &mut RamFs, fd: usize) -> Option<usize> {
        let slot = self.handle_slot(fd)?;
        let handle = self.slots[slot].take()?;
        fs.release(handle.node);
        Some(0)
    }

    pub(crate) fn close_all(&mut self, fs: &mut RamFs) {
        for handle in self.slots.iter_mut().filter_map(Option::take) {
            fs.release(handle.node);
        }
    }
}
