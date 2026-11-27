use clv_abi::audit::{
    EncodedPublicationRecord, PublicationOutcome, PublicationRecord, RenameDenial,
};
use clv_abi::goal::{PublicationError, RENAME_DIFF_MAX_BYTES, RenameOptions};

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

    fn matches(&self, file_index: usize, file: &WritableFile) -> bool {
        self.file_index == file_index
            && self.contents[..self.length] == file.contents[..file.length]
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

    fn checked_target(
        &self,
        fs: &RamFs,
        policy: &RenamePolicy,
    ) -> Result<(Name, Revision), PublicationError> {
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
        let file = fs.writable[self.file_index]
            .as_ref()
            .ok_or(PublicationError::Unavailable)?;
        if !policy.directory.contains(fs, file.parent)
            || policy
                .preserved_file
                .as_ref()
                .is_some_and(|input| !input.matches(self.file_index, file))
        {
            return Err(PublicationError::PostconditionFailed);
        }
        let name = Name::new(new_name).ok_or(PublicationError::Unavailable)?;
        let revision = fs.revision.next().ok_or(PublicationError::Unavailable)?;
        Ok((name, revision))
    }

    pub(crate) fn check(&self, fs: &RamFs, policy: &RenamePolicy) -> Result<(), PublicationError> {
        self.checked_target(fs, policy).map(|_| ())
    }

    pub(crate) fn publish(
        &self,
        fs: &mut RamFs,
        policy: &RenamePolicy,
    ) -> Result<(), PublicationError> {
        let (name, revision) = self.checked_target(fs, policy)?;
        let file = fs.writable[self.file_index]
            .as_mut()
            .ok_or(PublicationError::Unavailable)?;
        file.name = name;
        fs.revision = revision;
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
