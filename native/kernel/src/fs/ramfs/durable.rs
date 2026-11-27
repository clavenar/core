use super::{
    BOOT_FILES, DirectoryId, MAX_GOAL_RENAMES, NAME_LIMIT, Name, Node, RamFs,
    WRITABLE_FILE_CAPACITY, WritableFile,
};

const LENGTH_BYTES: usize = size_of::<u16>();
const PAIR_ENTRY_HEADER_BYTES: usize = 2 * LENGTH_BYTES;
const PAYLOAD_CAPACITY: usize =
    MAX_GOAL_RENAMES * (PAIR_ENTRY_HEADER_BYTES + NAME_LIMIT + WRITABLE_FILE_CAPACITY);

pub(crate) struct SavedFiles {
    first: WritableFile,
    second: Option<WritableFile>,
}

impl SavedFiles {
    pub(crate) fn decode(payload: &[u8], count: usize) -> Option<Self> {
        match count {
            1 => {
                let (name_len, body) = decode_length(payload)?;
                let (name, contents) = body.split_at_checked(name_len)?;
                Some(Self {
                    first: decode_file(name, contents)?,
                    second: None,
                })
            }
            2 => {
                let (first, rest) = decode_pair_entry(payload)?;
                let (second, rest) = decode_pair_entry(rest)?;
                if !rest.is_empty() || first.name.as_bytes() == second.name.as_bytes() {
                    return None;
                }
                Some(Self {
                    first,
                    second: Some(second),
                })
            }
            _ => None,
        }
    }
}

fn decode_length(bytes: &[u8]) -> Option<(usize, &[u8])> {
    let (length, rest) = bytes.split_first_chunk::<LENGTH_BYTES>()?;
    Some((usize::from(u16::from_le_bytes(*length)), rest))
}

fn decode_pair_entry(bytes: &[u8]) -> Option<(WritableFile, &[u8])> {
    let (name_len, rest) = decode_length(bytes)?;
    let (content_len, body) = decode_length(rest)?;
    let (name, body) = body.split_at_checked(name_len)?;
    let (contents, rest) = body.split_at_checked(content_len)?;
    Some((decode_file(name, contents)?, rest))
}

fn decode_file(name: &[u8], contents: &[u8]) -> Option<WritableFile> {
    if contents.len() > WRITABLE_FILE_CAPACITY || BOOT_FILES.iter().any(|file| file.name == name) {
        return None;
    }
    let mut file = WritableFile::new(DirectoryId::Root, name)?;
    file.contents[..contents.len()].copy_from_slice(contents);
    file.length = contents.len();
    Some(file)
}

pub(crate) struct EncodedFiles {
    bytes: [u8; PAYLOAD_CAPACITY],
    length: usize,
}

impl EncodedFiles {
    pub(super) fn new<'a>(
        files: impl Iterator<Item = (&'a Name, &'a WritableFile)>,
        count: usize,
    ) -> Self {
        let header_bytes = if count == 2 {
            PAIR_ENTRY_HEADER_BYTES
        } else {
            LENGTH_BYTES
        };
        let mut bytes = [0; PAYLOAD_CAPACITY];
        let mut length = 0;
        for (name, file) in files {
            let name = name.as_bytes();
            let entry_len = header_bytes + name.len() + file.length;
            let (header, data) = bytes[length..length + entry_len].split_at_mut(header_bytes);
            header[..LENGTH_BYTES].copy_from_slice(&(name.len() as u16).to_le_bytes());
            if count == 2 {
                header[LENGTH_BYTES..].copy_from_slice(&(file.length as u16).to_le_bytes());
            }
            let (basename, contents) = data.split_at_mut(name.len());
            basename.copy_from_slice(name);
            contents.copy_from_slice(&file.contents[..file.length]);
            length += entry_len;
        }
        Self { bytes, length }
    }

    pub(crate) fn payload(&self) -> &[u8] {
        &self.bytes[..self.length]
    }
}

impl RamFs {
    /// Called on the fresh filesystem before the first user task runs.
    pub(crate) fn restore_durable_files(&mut self, saved: SavedFiles) {
        for (index, file) in core::iter::once(saved.first)
            .chain(saved.second)
            .enumerate()
        {
            self.writable[index] = Some(file);
            self.pin_durable_file(index);
        }
    }

    pub(super) fn can_save_file(&self, index: usize, parent: DirectoryId) -> bool {
        parent == DirectoryId::Root
            && (self.durable_files[0].is_none() || self.durable_files.contains(&Some(index)))
    }

    pub(super) fn pin_durable_file(&mut self, index: usize) {
        if !self.durable_files.contains(&Some(index)) {
            let slot = self
                .durable_files
                .iter_mut()
                .find(|saved| saved.is_none())
                .expect("the prepared batch fits the journal");
            *slot = Some(index);
            // Keep the index reserved if an ordinary unlink removes the saved file.
            self.retain(Node::WritableFile(index));
        }
    }
}
