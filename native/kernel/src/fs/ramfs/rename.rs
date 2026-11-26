use clv_abi::goal::{CommitResult, RENAME_DIFF_MAX_BYTES};

use super::{CWD_PATH_CAPACITY, DirectoryId, NAME_LIMIT, Name, Node, RamFs, Revision};

const DIFF_CAPACITY: usize = CWD_PATH_CAPACITY + 1 + 2 * NAME_LIMIT;
const _: () = assert!(DIFF_CAPACITY <= RENAME_DIFF_MAX_BYTES);

/// A retained file and the names captured when its owner staged this rename.
pub(crate) struct StagedRename {
    file_index: usize,
    parent: DirectoryId,
    base: Revision,
    names: [u8; DIFF_CAPACITY],
    source_len: usize,
    new_name_len: usize,
}

impl StagedRename {
    pub(crate) fn diff(&self) -> (&[u8], usize) {
        let length = self.source_len + self.new_name_len;
        (&self.names[self.names.len() - length..], self.source_len)
    }

    pub(crate) fn discard(self, fs: &mut RamFs) {
        fs.release(Node::WritableFile(self.file_index));
    }

    pub(crate) fn publish(&self, fs: &mut RamFs) -> Option<CommitResult> {
        let (names, split) = self.diff();
        let (path, new_name) = names.split_at(split);
        let old_name = path.rsplit(|&byte| byte == b'/').next()?;
        if fs.revision != self.base
            || fs.lookup(self.parent, old_name) != Some(Node::WritableFile(self.file_index))
            || fs.lookup(self.parent, new_name).is_some()
        {
            return Some(CommitResult::Conflict);
        }
        let name = Name::new(new_name)?;
        let file = fs.writable[self.file_index].as_mut()?;
        fs.revision.advance()?;
        file.name = name;
        Some(CommitResult::Committed)
    }
}

impl RamFs {
    pub(super) fn stage_rename(&mut self, source: Node, new_name: &[u8]) -> Option<StagedRename> {
        let Node::WritableFile(index) = source else {
            return None;
        };
        let file = self.writable[index].as_ref()?;
        if !file.linked || !Name::valid(new_name) || self.lookup(file.parent, new_name).is_some() {
            return None;
        }
        let mut names = [0; DIFF_CAPACITY];
        let parent = file.parent;
        let (path, destination) = names.split_at_mut(DIFF_CAPACITY - new_name.len());
        destination.copy_from_slice(new_name);
        // absolute_path writes backwards, placing the source directly before the new name.
        let source_len = self
            .absolute_path(parent, Some(file.name.as_bytes()), path)?
            .len();
        self.retain(source);
        Some(StagedRename {
            file_index: index,
            parent,
            base: self.revision,
            names,
            source_len,
            new_name_len: new_name.len(),
        })
    }
}
