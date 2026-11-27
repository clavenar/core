use super::{
    BLANK_SECTOR, Block, ProbeError, SECTOR_BYTES, Sector, WriteRestriction, padded_sector,
};

pub(super) const HEADER: Sector = padded_sector(b"CLVJNL01");
const SLOT_SECTORS: [u64; 2] = [1, 2];
const DEMO_PAYLOADS: [&[u8]; 2] = [b"CLVSLOT1", b"CLVSLOT2"];

const VERSION: u32 = 1;
const RECORD_HEADER_BYTES: usize = 16;
const CHECKSUM_OFFSET: usize = SECTOR_BYTES - size_of::<u32>();
const MAX_PAYLOAD_BYTES: usize = CHECKSUM_OFFSET - RECORD_HEADER_BYTES;

#[derive(Clone, Copy)]
struct Record<'a> {
    generation: u64,
    payload: &'a [u8],
}

impl<'a> Record<'a> {
    fn decode(sector: &'a Sector) -> Option<Self> {
        let (record, checksum) = sector.split_at(CHECKSUM_OFFSET);
        let version = u32::from_le_bytes(record[..4].try_into().ok()?);
        let length = u32::from_le_bytes(record[4..8].try_into().ok()?) as usize;
        let generation = u64::from_le_bytes(record[8..16].try_into().ok()?);
        let (payload, padding) = record[RECORD_HEADER_BYTES..].split_at_checked(length)?;
        if version != VERSION
            || generation == 0
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

    fn encode(self) -> Option<Sector> {
        if self.generation == 0 || self.payload.len() > MAX_PAYLOAD_BYTES {
            return None;
        }
        let mut sector = BLANK_SECTOR;
        let (record, checksum) = sector.split_at_mut(CHECKSUM_OFFSET);
        record[..4].copy_from_slice(&VERSION.to_le_bytes());
        record[4..8].copy_from_slice(&(self.payload.len() as u32).to_le_bytes());
        record[8..16].copy_from_slice(&self.generation.to_le_bytes());
        record[RECORD_HEADER_BYTES..][..self.payload.len()].copy_from_slice(self.payload);
        checksum.copy_from_slice(&crc32(record).to_le_bytes());
        Some(sector)
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
    TooSmall,
    InvalidSlots,
    GenerationExhausted,
    Failed(ProbeError),
}

pub(super) fn run(block: &mut Block<'_>) -> Result<JournalProbe, ProbeError> {
    if block.sectors <= SLOT_SECTORS[1] {
        return Ok(JournalProbe {
            recovered: None,
            update: JournalUpdate::TooSmall,
        });
    }
    let slots = [
        block.read_sector(SLOT_SECTORS[0])?,
        block.read_sector(SLOT_SECTORS[1])?,
    ];
    let records = slots.each_ref().map(Record::decode);
    if matches!(records, [Some(first), Some(second)] if first.generation == second.generation) {
        return Ok(JournalProbe {
            recovered: None,
            update: JournalUpdate::InvalidSlots,
        });
    }
    let latest = records
        .into_iter()
        .enumerate()
        .filter_map(|(slot, record)| record.map(|record| (slot, record)))
        .max_by_key(|(_, record)| record.generation);
    if latest.is_none() && slots.iter().any(|slot| slot != &BLANK_SECTOR) {
        return Ok(JournalProbe {
            recovered: None,
            update: JournalUpdate::InvalidSlots,
        });
    }

    let recovered = latest.map(|(slot, record)| record.report(SLOT_SECTORS[slot]));
    if let Some(restriction) = block.write_restriction() {
        return Ok(JournalProbe {
            recovered,
            update: JournalUpdate::Blocked(restriction),
        });
    }

    let generation = latest.map_or(0, |(_, record)| record.generation);
    let Some(generation) = generation.checked_add(1) else {
        return Ok(JournalProbe {
            recovered,
            update: JournalUpdate::GenerationExhausted,
        });
    };
    // Keep the recovered record intact while writing its replacement in the other slot.
    let slot = latest.map_or(0, |(slot, _)| 1 - slot);
    let record = Record {
        generation,
        payload: DEMO_PAYLOADS[slot],
    };
    let bytes = record
        .encode()
        .expect("the journal demo record fits one sector");
    let sector = SLOT_SECTORS[slot];
    let update = match block.write_and_flush(sector, &bytes) {
        Ok(()) => JournalUpdate::Flushed(record.report(sector)),
        Err(error) => JournalUpdate::Failed(error),
    };
    Ok(JournalProbe { recovered, update })
}
