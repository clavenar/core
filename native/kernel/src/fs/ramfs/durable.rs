use super::{
    BOOT_FILES, DirectoryId, NAME_LIMIT, Name, Node, RamFs, WRITABLE_FILE_CAPACITY, WritableFile,
};

const NAME_LENGTH_BYTES: usize = size_of::<u16>();
const PAYLOAD_CAPACITY: usize = NAME_LENGTH_BYTES + NAME_LIMIT + WRITABLE_FILE_CAPACITY;

pub(crate) struct SavedFile(WritableFile);

impl SavedFile {
    pub(crate) fn decode(payload: &[u8]) -> Option<Self> {
        let (length, body) = payload.split_first_chunk::<NAME_LENGTH_BYTES>()?;
        let name_len = usize::from(u16::from_le_bytes(*length));
        let (name, contents) = body.split_at_checked(name_len)?;
        if contents.len() > WRITABLE_FILE_CAPACITY
            || BOOT_FILES.iter().any(|file| file.name == name)
        {
            return None;
        }
        let mut file = WritableFile::new(DirectoryId::Root, name)?;
        file.contents[..contents.len()].copy_from_slice(contents);
        file.length = contents.len();
        Some(Self(file))
    }
}

pub(crate) struct EncodedFile {
    bytes: [u8; PAYLOAD_CAPACITY],
    length: usize,
}

impl EncodedFile {
    pub(super) fn new(name: &Name, file: &WritableFile) -> Self {
        let name = name.as_bytes();
        let name_end = NAME_LENGTH_BYTES + name.len();
        let length = name_end + file.length;
        let mut bytes = [0; PAYLOAD_CAPACITY];
        bytes[..NAME_LENGTH_BYTES].copy_from_slice(&(name.len() as u16).to_le_bytes());
        bytes[NAME_LENGTH_BYTES..name_end].copy_from_slice(name);
        bytes[name_end..length].copy_from_slice(&file.contents[..file.length]);
        Self { bytes, length }
    }

    pub(crate) fn payload(&self) -> &[u8] {
        &self.bytes[..self.length]
    }
}

impl RamFs {
    /// Called on the fresh filesystem before the first user task runs.
    pub(crate) fn restore_durable_file(&mut self, saved: SavedFile) {
        let index = self
            .writable
            .iter()
            .position(Option::is_none)
            .expect("fresh RamFS has room for the saved file");
        self.writable[index] = Some(saved.0);
        self.pin_durable_file(index);
    }

    pub(super) fn can_save_file(&self, index: usize, parent: DirectoryId) -> bool {
        parent == DirectoryId::Root && self.durable_file.is_none_or(|saved| saved == index)
    }

    pub(super) fn pin_durable_file(&mut self, index: usize) {
        if self.durable_file.is_none() {
            // Keep the index reserved if an ordinary unlink removes the saved file.
            self.retain(Node::WritableFile(index));
            self.durable_file = Some(index);
        }
    }
}
