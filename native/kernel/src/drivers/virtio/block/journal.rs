use super::{Block, ProbeError, SECTOR_BYTES, Sector, WriteRestriction, padded_sector};

pub(super) const HEADER: Sector = padded_sector(b"CLVJNL01");
const DEMO_PAYLOADS: [&[u8]; 2] = [b"CLVSLOT1", b"CLVSLOT2"];

const RECORD_HEADER_BYTES: usize = 16;
const MAX_SLOT_BYTES: usize = 2 * SECTOR_BYTES;
type Slot = [u8; MAX_SLOT_BYTES];

#[derive(Clone, Copy)]
#[repr(u32)]
pub(super) enum Format {
    Single = 1,
    Pair = 2,
}

impl Format {
    // The single-file and pair formats use slots of one and two sectors.
    pub(super) const fn file_count(self) -> usize {
        match self {
            Self::Single => 1,
            Self::Pair => 2,
        }
    }

    fn slot_bytes(self) -> usize {
        self.file_count() * SECTOR_BYTES
    }

    fn slot_start(self, slot: usize) -> u64 {
        1 + (slot * self.file_count()) as u64
    }

    fn checksum_offset(self) -> usize {
        self.slot_bytes() - size_of::<u32>()
    }

    fn payload_end(self) -> usize {
        match self {
            Self::Single => self.checksum_offset(),
            Self::Pair => self.checksum_offset() - size_of::<u64>(),
        }
    }

    pub(super) fn payload_capacity(self) -> usize {
        self.payload_end() - RECORD_HEADER_BYTES
    }

    fn read_slot(self, block: &mut Block, slot: usize) -> Result<Slot, JournalLoadError> {
        let mut bytes = [0; MAX_SLOT_BYTES];
        let (sectors, _) = bytes[..self.slot_bytes()].as_chunks_mut::<SECTOR_BYTES>();
        for (offset, sector) in sectors.iter_mut().enumerate() {
            *sector = block
                .read_sector(self.slot_start(slot) + offset as u64)
                .map_err(JournalLoadError::Io)?;
        }
        Ok(bytes)
    }
}

#[derive(Clone, Copy)]
struct Record<'a> {
    generation: u64,
    payload: &'a [u8],
}

impl<'a> Record<'a> {
    fn decode(slot: &'a Slot, format: Format) -> Option<Self> {
        let (record, checksum) = slot[..format.slot_bytes()].split_at(format.checksum_offset());
        let version = u32::from_le_bytes(record[..4].try_into().ok()?);
        let length = u32::from_le_bytes(record[4..8].try_into().ok()?) as usize;
        let generation = u64::from_le_bytes(record[8..16].try_into().ok()?);
        let (body, footer) = record.split_at(format.payload_end());
        let (payload, padding) = body[RECORD_HEADER_BYTES..].split_at_checked(length)?;
        if version != format as u32
            || generation == 0
            || (matches!(format, Format::Pair) && footer != generation.to_le_bytes())
            || padding.iter().any(|&byte| byte != 0)
            || crc32(record) != u32::from_le_bytes(checksum.try_into().ok()?)
        {
            return None;
        }
        Some(Self {
            generation,
            payload,
        })
    }

    fn encode(self, format: Format) -> Option<Slot> {
        if self.generation == 0 || self.payload.len() > format.payload_capacity() {
            return None;
        }
        let mut slot = [0; MAX_SLOT_BYTES];
        let (record, checksum) = slot[..format.slot_bytes()].split_at_mut(format.checksum_offset());
        record[..4].copy_from_slice(&(format as u32).to_le_bytes());
        record[4..8].copy_from_slice(&(self.payload.len() as u32).to_le_bytes());
        record[8..16].copy_from_slice(&self.generation.to_le_bytes());
        record[RECORD_HEADER_BYTES..][..self.payload.len()].copy_from_slice(self.payload);
        if matches!(format, Format::Pair) {
            // Reject two sectors from different generations before checking the slot CRC.
            record[format.payload_end()..].copy_from_slice(&self.generation.to_le_bytes());
        }
        checksum.copy_from_slice(&crc32(record).to_le_bytes());
        Some(slot)
    }

    fn report(self, sector: u64) -> JournalRecord {
        let mut prefix = [0; 8];
        let length = self.payload.len().min(prefix.len());
        prefix[..length].copy_from_slice(&self.payload[..length]);
        JournalRecord {
            generation: self.generation,
            sector,
            prefix,
        }
    }
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = u32::MAX;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            let low_bit = crc & 1;
            crc >>= 1;
            if low_bit != 0 {
                crc ^= 0xedb8_8320;
            }
        }
    }
    !crc
}

#[derive(Clone, Copy)]
pub(crate) struct JournalRecord {
    pub(crate) generation: u64,
    pub(crate) sector: u64,
    pub(crate) prefix: [u8; 8],
}

pub(crate) struct JournalProbe {
    pub(crate) recovered: Option<JournalRecord>,
    pub(crate) update: JournalUpdate,
}

pub(crate) enum JournalUpdate {
    Flushed(JournalRecord),
    Blocked(WriteRestriction),
    GenerationExhausted,
    Failed(ProbeError),
}

pub(crate) enum JournalLoadError {
    TooSmall,
    InvalidSlots,
    Io(ProbeError),
}

pub(super) struct Journal {
    pub(super) format: Format,
    latest: Option<JournalRecord>,
}

impl Journal {
    pub(super) fn load<T>(
        block: &mut Block,
        format: Format,
        decode: impl Fn(&[u8]) -> Option<T>,
    ) -> Result<(Self, Option<T>), JournalLoadError> {
        if block.sectors < 1 + 2 * format.file_count() as u64 {
            return Err(JournalLoadError::TooSmall);
        }
        let slots = [format.read_slot(block, 0)?, format.read_slot(block, 1)?];
        let records = slots.each_ref().map(|slot| {
            let record = Record::decode(slot, format)?;
            Some((record, decode(record.payload)?))
        });
        if matches!(&records, [Some((first, _)), Some((second, _))]
            if first.generation == second.generation)
        {
            return Err(JournalLoadError::InvalidSlots);
        }
        let latest = records
            .into_iter()
            .enumerate()
            .filter_map(|(slot, record)| record.map(|(record, value)| (slot, record, value)))
            .max_by_key(|(_, record, _)| record.generation);
        if let Some((slot, record, value)) = latest {
            Ok((
                Self {
                    format,
                    latest: Some(record.report(format.slot_start(slot))),
                },
                Some(value),
            ))
        } else if slots == [[0; MAX_SLOT_BYTES]; 2] {
            Ok((
                Self {
                    format,
                    latest: None,
                },
                None,
            ))
        } else {
            Err(JournalLoadError::InvalidSlots)
        }
    }

    pub(super) fn can_append(&self) -> bool {
        self.latest
            .is_none_or(|record| record.generation < u64::MAX)
    }

    fn next_slot(&self) -> usize {
        // Keep the latest record intact while writing its replacement.
        usize::from(
            self.latest
                .is_some_and(|record| record.sector == self.format.slot_start(0)),
        )
    }

    pub(super) fn append(&mut self, block: &mut Block, payload: &[u8]) -> JournalUpdate {
        if let Some(restriction) = block.write_restriction() {
            return JournalUpdate::Blocked(restriction);
        }
        let generation = self.latest.map_or(0, |record| record.generation);
        let Some(generation) = generation.checked_add(1) else {
            return JournalUpdate::GenerationExhausted;
        };
        let record = Record {
            generation,
            payload,
        };
        let bytes = record
            .encode(self.format)
            .expect("the journal caller bounds the payload");
        let sector = self.format.slot_start(self.next_slot());
        let (sectors, _) = bytes[..self.format.slot_bytes()].as_chunks::<SECTOR_BYTES>();
        if let Err(error) = block.write_and_flush(sector, sectors) {
            return JournalUpdate::Failed(error);
        }
        let record = record.report(sector);
        self.latest = Some(record);
        JournalUpdate::Flushed(record)
    }
}

pub(super) fn run(block: &mut Block) -> Result<JournalProbe, JournalLoadError> {
    let (mut journal, _) = Journal::load(block, Format::Single, |_| Some(()))?;
    let recovered = journal.latest;
    let payload = DEMO_PAYLOADS[journal.next_slot()];
    let update = journal.append(block, payload);
    Ok(JournalProbe { recovered, update })
}
