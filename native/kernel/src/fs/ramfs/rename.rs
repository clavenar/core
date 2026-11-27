use clv_abi::audit::{
    EncodedPublicationRecord, PublicationOutcome, PublicationRecord, RenameDenial,
};
use clv_abi::goal::{MAX_GOAL_RENAMES, PublicationError, RENAME_DIFF_MAX_BYTES, RenameOptions};

use super::{
    CWD_PATH_CAPACITY, DirectoryId, NAME_LIMIT, Name, Node, RamFs, Revision, TaskFs,
    WRITABLE_FILE_CAPACITY, WritableFile,
};

const DIFF_CAPACITY: usize = CWD_PATH_CAPACITY + 1 + 2 * NAME_LIMIT;
const _: () = assert!(DIFF_CAPACITY <= RENAME_DIFF_MAX_BYTES);

pub(crate) struct RenamePolicy {
    // RamFS refuses rmdir on this directory for the lifetime of the owning goal.
    pub(crate) directory: DirectoryId,
    preserved_file: Option<PreservedFile>,
}

impl RenamePolicy {
    pub(crate) fn capture(caller: &TaskFs, fs: &mut RamFs, options: RenameOptions) -> Option<Self> {
        let directory = match options.directory {
            None => DirectoryId::Root,
            Some(fd) => caller.directory_for_handle(fd)?,
        };
        let preserved_file = match options.preserve_file {
            Some(fd) => Some(PreservedFile::capture(fs, caller.node_for_handle(fd)?)?),
            None => None,
        };
        Some(Self {
            directory,
            preserved_file,
        })
    }

    pub(crate) fn release_input(&mut self, fs: &mut RamFs) {
        if let Some(input) = self.preserved_file.take() {
            fs.release(Node::WritableFile(input.file_index));
        }
    }
}

/// The input object and its exact contents at goal creation.
struct PreservedFile {
    file_index: usize,
    contents: [u8; WRITABLE_FILE_CAPACITY],
    length: usize,
}

impl PreservedFile {
    fn capture(fs: &mut RamFs, node: Node) -> Option<Self> {
        let (file_index, file) = fs.linked_file(node)?;
        let snapshot = Self {
            file_index,
            contents: file.contents,
            length: file.length,
        };
        fs.retain(node);
        Some(snapshot)
    }

    fn matches(&self, file: &WritableFile) -> bool {
        self.contents[..self.length] == file.contents[..file.length]
    }
}

/// A retained file and the names captured when this rename was staged.
pub(crate) struct StagedRename {
    file_index: usize,
    parent: DirectoryId,
    base: Revision,
    names: [u8; DIFF_CAPACITY],
    source_len: usize,
    new_name_len: usize,
}

impl StagedRename {
    pub(crate) fn base_revision(&self) -> u64 {
        self.base.0
    }

    pub(crate) fn diff(&self) -> (&[u8], usize) {
        let length = self.source_len + self.new_name_len;
        (&self.names[self.names.len() - length..], self.source_len)
    }

    pub(crate) fn publication_record(
        &self,
        fs: &RamFs,
        outcome: PublicationOutcome,
    ) -> EncodedPublicationRecord {
        let (names, split) = self.diff();
        let (source, new_name) = names.split_at(split);
        PublicationRecord {
            outcome,
            base_revision: self.base_revision(),
            revision: fs.revision.0,
            source,
            new_name,
        }
        .encode()
        .expect("a staged RamFS rename and its decision fit the audit record")
    }

    pub(crate) fn discard(self, fs: &mut RamFs) {
        fs.release(Node::WritableFile(self.file_index));
    }

    fn checked_target(&self, fs: &RamFs) -> Result<Name, PublicationError> {
        let (names, split) = self.diff();
        let (path, new_name) = names.split_at(split);
        let old_name = path
            .rsplit(|&byte| byte == b'/')
            .next()
            .ok_or(PublicationError::Unavailable)?;
        if fs.revision != self.base
            || fs.lookup(self.parent, old_name) != Some(Node::WritableFile(self.file_index))
            || fs.lookup(self.parent, new_name).is_some()
        {
            return Err(PublicationError::Conflict);
        }
        Name::new(new_name).ok_or(PublicationError::Unavailable)
    }

    fn destination(&self) -> &[u8] {
        let (names, source_len) = self.diff();
        &names[source_len..]
    }
}

/// A nonempty set of renames against one RamFS revision.
pub(crate) struct StagedRenames {
    first: StagedRename,
    second: Option<StagedRename>,
}

struct PreparedRenames {
    names: [Option<(usize, Name)>; MAX_GOAL_RENAMES],
    revision: Revision,
}

impl StagedRenames {
    pub(crate) fn new(first: StagedRename) -> Self {
        Self {
            first,
            second: None,
        }
    }

    pub(crate) fn len(&self) -> usize {
        1 + usize::from(self.second.is_some())
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = &StagedRename> {
        core::iter::once(&self.first).chain(self.second.as_ref())
    }

    pub(crate) fn base_revision(&self) -> u64 {
        self.first.base_revision()
    }

    pub(crate) fn append(
        &mut self,
        fs: &mut RamFs,
        rename: StagedRename,
    ) -> Result<(), RenameDenial> {
        if let Err(reason) = self.check_append(&rename) {
            rename.discard(fs);
            return Err(reason);
        }
        self.second = Some(rename);
        Ok(())
    }

    fn check_append(&self, rename: &StagedRename) -> Result<(), RenameDenial> {
        if self.second.is_some() {
            return Err(RenameDenial::AlreadyStaged);
        }
        if self.first.base != rename.base {
            return Err(RenameDenial::BaseChanged);
        }
        if self.first.file_index == rename.file_index {
            return Err(RenameDenial::SourceAlreadyStaged);
        }
        if self.first.parent == rename.parent && self.first.destination() == rename.destination() {
            return Err(RenameDenial::DestinationAlreadyStaged);
        }
        Ok(())
    }

    pub(crate) fn discard(self, fs: &mut RamFs) {
        self.first.discard(fs);
        if let Some(second) = self.second {
            second.discard(fs);
        }
    }

    fn prepare(
        &self,
        fs: &RamFs,
        policy: &RenamePolicy,
    ) -> Result<PreparedRenames, PublicationError> {
        let mut names = [const { None }; MAX_GOAL_RENAMES];
        for (slot, rename) in names.iter_mut().zip(self.iter()) {
            *slot = Some((rename.file_index, rename.checked_target(fs)?));
        }
        if self
            .iter()
            .any(|rename| !policy.directory.contains(fs, rename.parent))
        {
            return Err(PublicationError::PostconditionFailed);
        }
        if let Some(input) = &policy.preserved_file {
            let includes_input = self
                .iter()
                .any(|rename| rename.file_index == input.file_index);
            let file = fs.writable[input.file_index].as_ref();
            if !includes_input || !file.is_some_and(|file| input.matches(file)) {
                return Err(PublicationError::PostconditionFailed);
            }
        }
        let revision = fs.revision.next().ok_or(PublicationError::Unavailable)?;
        Ok(PreparedRenames { names, revision })
    }

    pub(crate) fn check(&self, fs: &RamFs, policy: &RenamePolicy) -> Result<(), PublicationError> {
        self.prepare(fs, policy).map(|_| ())
    }

    pub(crate) fn publish(
        &self,
        fs: &mut RamFs,
        policy: &RenamePolicy,
    ) -> Result<(), PublicationError> {
        let prepared = self.prepare(fs, policy)?;
        // The kernel runs this syscall without scheduling another task. Every
        // source is linked and every destination has been validated above.
        for (index, name) in prepared.names.into_iter().flatten() {
            fs.writable[index]
                .as_mut()
                .expect("validated rename source")
                .name = name;
        }
        fs.revision = prepared.revision;
        Ok(())
    }
}

impl RamFs {
    pub(super) fn stage_rename(
        &mut self,
        source: Node,
        new_name: &[u8],
        policy: &RenamePolicy,
    ) -> Result<StagedRename, RenameDenial> {
        let (index, file) = self
            .linked_file(source)
            .ok_or(RenameDenial::InvalidSource)?;
        if !policy.directory.contains(self, file.parent) {
            return Err(RenameDenial::OutsideRenameScope);
        }
        if !Name::valid(new_name) {
            return Err(RenameDenial::InvalidBasename);
        }
        if self.lookup(file.parent, new_name).is_some() {
            return Err(RenameDenial::DestinationExists);
        }
        let mut names = [0; DIFF_CAPACITY];
        let parent = file.parent;
        let (path, destination) = names.split_at_mut(DIFF_CAPACITY - new_name.len());
        destination.copy_from_slice(new_name);
        // absolute_path writes backwards, placing the source directly before the new name.
        let source_len = self
            .absolute_path(parent, Some(file.name.as_bytes()), path)
            .ok_or(RenameDenial::InvalidSource)?
            .len();
        self.retain(source);
        Ok(StagedRename {
            file_index: index,
            parent,
            base: self.revision,
            names,
            source_len,
            new_name_len: new_name.len(),
        })
    }
}
