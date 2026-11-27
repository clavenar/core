mod journal;

use super::mmio::Device;
use super::queue::{DEVICE_WRITES, Descriptor, NEXT, Ring, RingMemory};
use crate::arch::monotonic_nanos;

use journal::Journal;
pub(crate) use journal::{JournalLoadError, JournalProbe, JournalRecord, JournalUpdate};

const BLOCK_DEVICE: u32 = 2;
const BLOCK_READ_ONLY: u32 = 1 << 5;
const BLOCK_FLUSH: u32 = 1 << 9;
const REQUEST_QUEUE: u32 = 0;
const IO_TIMEOUT_NS: u64 = 500_000_000;
const SECTOR_BYTES: usize = 512;
const READ: u32 = 0;
const WRITE: u32 = 1;
const FLUSH: u32 = 4;
const STATUS_OK: u8 = 0;
const STATUS_IO_ERROR: u8 = 1;
const STATUS_UNSUPPORTED: u8 = 2;

type Sector = [u8; SECTOR_BYTES];
const DEMO_SECTOR: u64 = 1;
const BLANK_SECTOR: Sector = [0; SECTOR_BYTES];
const DEMO_HEADER: Sector = padded_sector(b"CLVDEMO1");
const DEMO_MARKER: Sector = padded_sector(b"CLVSAVE1");
const DURABLE_HEADER: Sector = padded_sector(b"CLVRNM01");

const fn padded_sector(prefix: &[u8; 8]) -> Sector {
    let mut sector = BLANK_SECTOR;
    sector.split_at_mut(8).0.copy_from_slice(prefix);
    sector
}

#[repr(C)]
struct RequestHeader {
    kind: u32,
    reserved: u32,
    sector: u64,
}

impl RequestHeader {
    const fn new(kind: u32, sector: u64) -> Self {
        Self {
            kind: kind.to_le(),
            reserved: 0,
            sector: sector.to_le(),
        }
    }
}

struct RequestMemory {
    header: RequestHeader,
    data: Sector,
    status: u8,
}

enum Request<'a> {
    Read(u64),
    Write(u64, &'a Sector),
    Flush,
}

struct BlockMemory {
    queue: RingMemory<4>,
    request: RequestMemory,
}

// Keep this memory reserved even if a device does not acknowledge reset.
static mut DMA: BlockMemory = BlockMemory {
    queue: RingMemory::empty(),
    request: RequestMemory {
        header: RequestHeader::new(READ, 0),
        data: BLANK_SECTOR,
        status: u8::MAX,
    },
};

pub(crate) struct Probe {
    pub(crate) sectors: u64,
    pub(crate) prefix: [u8; 8],
    pub(crate) scratch: Option<ScratchProbe>,
    pub(crate) durable_disk: Option<Block>,
}

pub(crate) enum ScratchProbe {
    Marker(Result<DemoOutcome, ProbeError>),
    Journal(Result<JournalProbe, JournalLoadError>),
}

pub(crate) enum DemoOutcome {
    Flushed,
    Recovered,
    Blocked(WriteRestriction),
    Occupied,
    TooSmall,
}

pub(crate) enum WriteRestriction {
    ReadOnly,
    FlushUnavailable,
}

pub(crate) enum ProbeError {
    Device,
    Empty,
    Read,
    Write,
    Flush,
    InvalidCompletion,
    Timeout,
}

pub(crate) struct Block {
    device: Device,
    queue: Ring<4>,
    request: *mut RequestMemory,
    sectors: u64,
    features: u32,
    deadline: u64,
}

pub(crate) struct JournalDisk {
    block: Block,
    journal: Journal,
    failed: bool,
}

impl JournalDisk {
    pub(crate) fn is_writable(&self) -> bool {
        !self.failed && self.block.write_restriction().is_none() && self.journal.can_append()
    }

    pub(crate) fn save(&mut self, payload: &[u8]) -> bool {
        if !self.is_writable() || payload.len() > journal::MAX_PAYLOAD_BYTES {
            return false;
        }
        self.block.deadline = io_deadline();
        if matches!(
            self.journal.append(&mut self.block, payload),
            JournalUpdate::Flushed(_)
        ) {
            return true;
        }
        // A timed-out request may still own the DMA buffers. Stop using the
        // journal for this boot and keep the buffers reserved even if reset fails.
        self.failed = true;
        self.block.device.reset(self.block.deadline);
        false
    }
}

fn io_deadline() -> u64 {
    monotonic_nanos().saturating_add(IO_TIMEOUT_NS)
}

/// # Safety
/// Call once on the boot hart before paging on QEMU RV64 virt, with its MMIO
/// windows and the DMA storage accessible at their physical addresses. Keep
/// the retained device's window and DMA storage mapped while it is in use.
pub(crate) unsafe fn probe() -> Option<Result<Probe, ProbeError>> {
    // SAFETY: The boot caller guarantees access to the virt transport windows.
    let device = unsafe { Device::discover(BLOCK_DEVICE) }?;
    let deadline = io_deadline();
    if !device.reset(deadline) {
        return Some(Err(ProbeError::Timeout));
    }
    let memory = &raw mut DMA;
    // SAFETY: Reset completed, so the device has no access while we initialize these fields.
    let mut block = unsafe {
        Block {
            device,
            queue: Ring::new(&raw mut (*memory).queue, REQUEST_QUEUE),
            request: &raw mut (*memory).request,
            sectors: 0,
            features: 0,
            deadline,
        }
    };
    let result = block.start().and_then(|()| block.inspect());
    Some(match result {
        Ok((mut probe, true)) => {
            probe.durable_disk = Some(block);
            Ok(probe)
        }
        result if block.device.reset(deadline) => result.map(|(probe, _)| probe),
        _ => Err(ProbeError::Timeout),
    })
}

impl Block {
    fn start(&mut self) -> Result<(), ProbeError> {
        self.features = self
            .device
            .negotiate(BLOCK_READ_ONLY | BLOCK_FLUSH)
            .ok_or(ProbeError::Device)?;
        // SAFETY: Capacity is the 64-bit field at offset zero of the block configuration.
        self.sectors =
            unsafe { self.device.config_u64(0, self.deadline) }.ok_or(ProbeError::Timeout)?;
        if self.sectors == 0 {
            return Err(ProbeError::Empty);
        }
        if !self.queue.configure(&self.device) || !self.device.start() {
            return Err(ProbeError::Device);
        }
        Ok(())
    }

    fn inspect(&mut self) -> Result<(Probe, bool), ProbeError> {
        let first = self.read_sector(0)?;
        let scratch = match first {
            DEMO_HEADER => Some(ScratchProbe::Marker(self.persist_marker())),
            journal::HEADER => Some(ScratchProbe::Journal(journal::run(self))),
            _ => None,
        };
        Ok((
            Probe {
                sectors: self.sectors,
                prefix: *first.first_chunk().expect("a sector holds eight bytes"),
                scratch,
                durable_disk: None,
            },
            first == DURABLE_HEADER,
        ))
    }

    pub(crate) fn base(&self) -> usize {
        self.device.base()
    }

    pub(crate) fn open_journal<T>(
        mut self,
        decode: impl Fn(&[u8]) -> Option<T>,
    ) -> Result<(JournalDisk, Option<T>), JournalLoadError> {
        self.deadline = io_deadline();
        let (journal, recovered) = match Journal::load(&mut self, decode) {
            Ok(loaded) => loaded,
            Err(error) => {
                self.device.reset(self.deadline);
                return Err(error);
            }
        };
        Ok((
            JournalDisk {
                block: self,
                journal,
                failed: false,
            },
            recovered,
        ))
    }

    fn read_sector(&mut self, sector: u64) -> Result<Sector, ProbeError> {
        self.transfer(Request::Read(sector))?;
        // SAFETY: The completion covers the full sector and returns ownership of the buffer.
        Ok(unsafe { (&raw const (*self.request).data).read_volatile() })
    }

    fn persist_marker(&mut self) -> Result<DemoOutcome, ProbeError> {
        if self.sectors <= DEMO_SECTOR {
            return Ok(DemoOutcome::TooSmall);
        }
        let sector = self.read_sector(DEMO_SECTOR)?;
        if sector == DEMO_MARKER {
            return Ok(DemoOutcome::Recovered);
        }
        if sector != BLANK_SECTOR {
            return Ok(DemoOutcome::Occupied);
        }
        if let Some(restriction) = self.write_restriction() {
            return Ok(DemoOutcome::Blocked(restriction));
        }

        self.write_and_flush(DEMO_SECTOR, &DEMO_MARKER)?;
        Ok(DemoOutcome::Flushed)
    }

    fn write_restriction(&self) -> Option<WriteRestriction> {
        if self.features & BLOCK_READ_ONLY != 0 {
            Some(WriteRestriction::ReadOnly)
        } else if self.features & BLOCK_FLUSH == 0 {
            Some(WriteRestriction::FlushUnavailable)
        } else {
            None
        }
    }

    fn write_and_flush(&mut self, sector: u64, data: &Sector) -> Result<(), ProbeError> {
        self.transfer(Request::Write(sector, data))?;
        self.transfer(Request::Flush)
    }

    fn transfer(&mut self, request: Request<'_>) -> Result<(), ProbeError> {
        let (kind, sector, io_error) = match request {
            Request::Read(sector) => (READ, sector, ProbeError::Read),
            Request::Write(sector, _) => (WRITE, sector, ProbeError::Write),
            Request::Flush => (FLUSH, 0, ProbeError::Flush),
        };
        if sector >= self.sectors {
            return Err(io_error);
        }
        if monotonic_nanos() >= self.deadline {
            return Err(ProbeError::Timeout);
        }
        if self.queue.is_pending() || !self.device.operational() {
            return Err(ProbeError::Device);
        }

        // SAFETY: No request is pending, so the driver owns the resident DMA buffer.
        let (header, data, status) = unsafe {
            let header = &raw mut (*self.request).header;
            let data = &raw mut (*self.request).data;
            let status = &raw mut (*self.request).status;
            header.write(RequestHeader::new(kind, sector));
            status.write(u8::MAX);
            if let Request::Write(_, source) = request {
                data.write(*source);
            }
            (header, data, status)
        };
        let head = Descriptor::new(header.addr(), size_of::<RequestHeader>(), NEXT, 1);
        let tail = Descriptor::new(status.addr(), 1, DEVICE_WRITES, 0);
        let data_flags = if kind == READ { DEVICE_WRITES } else { 0 };
        // SAFETY: The chain points to resident DMA memory held through completion or reset.
        let submitted = unsafe {
            if kind == FLUSH {
                self.queue.submit(&[head, tail])
            } else {
                self.queue.submit(&[
                    head,
                    Descriptor::new(data.addr(), SECTOR_BYTES, NEXT | data_flags, 2),
                    tail,
                ])
            }
        };
        submitted.map_err(|_| ProbeError::Device)?;
        self.queue.notify(&self.device);

        let expected = if kind == READ { SECTOR_BYTES + 1 } else { 1 };
        while monotonic_nanos() < self.deadline {
            if !self.device.operational() {
                return Err(ProbeError::Device);
            }
            let Some(length) = self
                .queue
                .complete()
                .map_err(|_| ProbeError::InvalidCompletion)?
            else {
                core::hint::spin_loop();
                continue;
            };
            if length == 0 {
                return Err(ProbeError::InvalidCompletion);
            }
            // SAFETY: The used entry returns ownership to the driver. A fresh status is required.
            return match unsafe { status.read_volatile() } {
                STATUS_OK if length == expected => Ok(()),
                STATUS_IO_ERROR | STATUS_UNSUPPORTED => Err(io_error),
                _ => Err(ProbeError::InvalidCompletion),
            };
        }
        Err(ProbeError::Timeout)
    }
}
