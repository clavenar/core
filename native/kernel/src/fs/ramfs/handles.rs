use clv_abi::audit::{DelegationDenial, RenameDenial};
use clv_abi::goal::FileRecipient;
use clv_abi::{GrantKind, MESSAGE_MAX_BYTES, Program};

use super::{CWD_PATH_CAPACITY, DirectoryId, Node, PathBase, RamFs, RenamePolicy, StagedRename};

const HANDLE_COUNT: usize = 4;

impl Node {
    fn grant_kind(self) -> GrantKind {
        match self {
            Self::Directory(_) => GrantKind::Directory,
            Self::BootFile(_) | Self::WritableFile(_) => GrantKind::File,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum FileGrantMode {
    Read,
    Delegate(FileRecipient),
    StageRename,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Access {
    ReadOnly,
    ReadWrite,
    /// Every derived handle retains its goal ID for revocation.
    GoalFile {
        goal: usize,
        mode: FileGrantMode,
    },
}

struct Handle {
    // Reusing a table slot assigns it a fresh ID, so a closed descriptor stays invalid.
    id: usize,
    node: Node,
    cursor: usize,
    access: Access,
}

impl Handle {
    fn allocate(
        fs: &mut RamFs,
        access: Access,
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
            access,
        })
    }
}

/// An owned copy of the exact helper argument, which may be empty.
pub(crate) struct GrantArgument {
    bytes: [u8; MESSAGE_MAX_BYTES],
    length: usize,
}

impl GrantArgument {
    pub(crate) fn copy_from(length: usize, copy: impl FnOnce(&mut [u8]) -> bool) -> Option<Self> {
        let mut bytes = [0; MESSAGE_MAX_BYTES];
        if !copy(bytes.get_mut(..length)?) {
            return None;
        }
        Some(Self { bytes, length })
    }

    fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.length]
    }
}

/// Pins the shell-authorized file and its grant rules while the goal can admit a child.
pub(crate) struct FileGrant {
    node: Node,
    goal: usize,
    mode: FileGrantMode,
    argument: Option<GrantArgument>,
}

impl FileGrant {
    pub(crate) fn is_for_rename(&self) -> bool {
        self.mode == FileGrantMode::StageRename
    }

    pub(crate) fn same_file(&self, other: &Self) -> bool {
        self.node == other.node
    }

    pub(crate) fn delegates_boot_file(
        &self,
        name: &[u8],
        helper: Program,
        argument: &[u8],
    ) -> bool {
        matches!(self.node, Node::BootFile(index) if super::BOOT_FILES[index].name == name)
            && self.mode == FileGrantMode::Delegate(FileRecipient::Program(helper))
            && self
                .argument
                .as_ref()
                .is_some_and(|pinned| pinned.as_bytes() == argument)
    }

    pub(crate) fn open_for_child(
        &self,
        fs: &mut RamFs,
        is_agent: bool,
        second: Option<&Self>,
    ) -> Option<TaskFs> {
        // Check the entire ID range before installing any handles or taking references.
        fs.next_handle_id
            .checked_add(1 + usize::from(second.is_some()))?;
        let mut child = TaskFs::empty();
        for (slot, grant) in child
            .slots
            .iter_mut()
            .zip(core::iter::once(self).chain(second))
        {
            let access = Access::GoalFile {
                goal: grant.goal,
                mode: if is_agent {
                    grant.mode
                } else {
                    FileGrantMode::Read
                },
            };
            *slot = Some(
                Handle::allocate(fs, access, |_| Some(grant.node))
                    .expect("file grant handle IDs were checked"),
            );
        }
        Some(child)
    }

    pub(crate) fn delegate(
        &self,
        fs: &mut RamFs,
        agent: &TaskFs,
        helper: Program,
        argument: Option<&[u8]>,
    ) -> Result<TaskFs, DelegationDenial> {
        let FileGrantMode::Delegate(recipient) = self.mode else {
            return Err(DelegationDenial::NoDelegableFile);
        };
        let required = Access::GoalFile {
            goal: self.goal,
            mode: self.mode,
        };
        if !agent
            .slots
            .iter()
            .flatten()
            .any(|handle| handle.access == required && handle.node == self.node)
        {
            return Err(DelegationDenial::NoDelegableFile);
        }
        if let FileRecipient::Program(allowed) = recipient
            && allowed != helper
        {
            return Err(DelegationDenial::FileRecipientNotAllowed);
        }
        if let Some(expected) = &self.argument
            && argument != Some(expected.as_bytes())
        {
            return Err(DelegationDenial::ArgumentNotAllowed);
        }
        self.open_for_child(fs, false, None)
            .ok_or(DelegationDenial::FileHandlesExhausted)
    }

    pub(crate) fn release(self, fs: &mut RamFs) {
        fs.release(self.node);
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

    pub(crate) fn grant(&self, fs: &mut RamFs, fd: usize, kind: GrantKind) -> Option<Self> {
        Self::from_grant(fs, self.grant_node(fd, kind)?, Access::ReadOnly)
    }

    pub(crate) fn retain_file(
        &self,
        fs: &mut RamFs,
        goal: usize,
        fd: usize,
        mode: FileGrantMode,
        argument: Option<GrantArgument>,
    ) -> Option<FileGrant> {
        let node = self.grant_node(fd, GrantKind::File)?;
        if mode == FileGrantMode::StageRename {
            fs.linked_file(node)?;
        }
        fs.retain(node);
        Some(FileGrant {
            node,
            goal,
            mode,
            argument,
        })
    }

    fn grant_node(&self, fd: usize, kind: GrantKind) -> Option<Node> {
        let node = self.node_for_handle(fd)?;
        (node.grant_kind() == kind).then_some(node)
    }

    pub(crate) fn stage_rename(
        &self,
        fs: &mut RamFs,
        fd: usize,
        new_name: &[u8],
        granting_goal: Option<usize>,
        policy: &RenamePolicy,
    ) -> Result<StagedRename, RenameDenial> {
        let handle = self.handle(fd);
        let source = if let Some(goal) = granting_goal {
            let required = Access::GoalFile {
                goal,
                mode: FileGrantMode::StageRename,
            };
            handle
                .filter(|handle| handle.access == required)
                .ok_or(RenameDenial::NoRenameGrant)?
        } else {
            handle.ok_or(RenameDenial::InvalidSource)?
        };
        fs.stage_rename(source.node, new_name, policy)
    }

    fn from_grant(fs: &mut RamFs, node: Node, access: Access) -> Option<Self> {
        let handle = Handle::allocate(fs, access, |_| Some(node))?;
        let mut child = Self::empty();
        child.slots[0] = Some(handle);
        Some(child)
    }

    /// Startup handles follow grant order. Only a rename pair fills both leading slots.
    pub(crate) fn startup_grant(&self) -> Option<(usize, GrantKind, Option<usize>)> {
        let first = self.slots[0].as_ref()?;
        match &self.slots[1] {
            Some(second) => Some((first.id, GrantKind::RenamePair, Some(second.id))),
            None => Some((first.id, first.node.grant_kind(), None)),
        }
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
        let directory = self.directory_for_handle(directory_fd)?;
        self.open_path(fs, PathBase::Confined(directory), path, false)
    }

    pub(crate) fn directory_for_handle(&self, fd: usize) -> Option<DirectoryId> {
        let Node::Directory(directory) = self.node_for_handle(fd)? else {
            return None;
        };
        Some(directory)
    }

    pub(super) fn node_for_handle(&self, fd: usize) -> Option<Node> {
        Some(self.handle(fd)?.node)
    }

    fn handle(&self, fd: usize) -> Option<&Handle> {
        self.slots.iter().flatten().find(|handle| handle.id == fd)
    }

    fn handle_mut(&mut self, fd: usize) -> Option<&mut Handle> {
        self.slots
            .iter_mut()
            .flatten()
            .find(|handle| handle.id == fd)
    }

    fn open_path(
        &mut self,
        fs: &mut RamFs,
        base: PathBase,
        path: &[u8],
        writable: bool,
    ) -> Option<usize> {
        let slot = self.slots.iter().position(Option::is_none)?;
        let access = if writable {
            Access::ReadWrite
        } else {
            Access::ReadOnly
        };
        let handle = Handle::allocate(fs, access, |fs| fs.open(base, path, writable))?;
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
        goal_pins: impl FnOnce(DirectoryId) -> bool,
    ) -> Option<usize> {
        fs.rmdir(self.cwd?, path, |directory| {
            goal_pins(directory) || live_task_fs.any(|task_fs| task_fs.uses_directory(directory))
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
        let mut buffer = [0; CWD_PATH_CAPACITY];
        let path = fs.absolute_path(self.cwd?, None, &mut buffer)?;
        deliver(path).then_some(path.len())
    }

    pub(crate) fn read(
        &mut self,
        fs: &RamFs,
        fd: usize,
        length: usize,
        deliver: impl FnOnce(Option<DirectoryId>, &[u8]) -> bool,
    ) -> Option<usize> {
        let handle = self.handle_mut(fd)?;
        // Another handle can truncate the file below this cursor.
        let (contents, parent) = fs.readable_file(handle.node)?;
        let remaining = contents.get(handle.cursor..).unwrap_or(&[]);
        let bytes = &remaining[..remaining.len().min(length)];
        if !deliver(parent, bytes) {
            return None;
        }
        handle.cursor += bytes.len();
        Some(bytes.len())
    }

    pub(crate) fn write(&mut self, fs: &mut RamFs, fd: usize, bytes: &[u8]) -> Option<usize> {
        let handle = self.handle_mut(fd)?;
        if handle.access != Access::ReadWrite {
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
        let handle = self.handle_mut(fd)?;
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
        let handle = self
            .slots
            .iter_mut()
            .find_map(|slot| slot.take_if(|handle| handle.id == fd))?;
        fs.release(handle.node);
        Some(0)
    }

    pub(crate) fn revoke_goal_file(&mut self, fs: &mut RamFs, goal: usize) {
        for slot in &mut self.slots {
            if let Some(handle) = slot.take_if(|handle| {
                matches!(handle.access, Access::GoalFile { goal: source, .. } if source == goal)
            }) {
                fs.release(handle.node);
            }
        }
    }

    pub(crate) fn close_all(&mut self, fs: &mut RamFs) {
        for handle in self.slots.iter_mut().filter_map(Option::take) {
            fs.release(handle.node);
        }
    }
}
