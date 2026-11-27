use super::mmio::Device;
use super::queue::{DEVICE_WRITES, Descriptor, NEXT, Ring, RingMemory};
use crate::arch::monotonic_nanos;

const BLOCK_DEVICE: u32 = 2;
const BLOCK_READ_ONLY: u32 = 1 << 5;
const REQUEST_QUEUE: u32 = 0;
const PROBE_TIMEOUT_NS: u64 = 500_000_000;
const SECTOR_BYTES: usize = 512;
const READ: u32 = 0;
const STATUS_OK: u8 = 0;
const STATUS_IO_ERROR: u8 = 1;
const STATUS_UNSUPPORTED: u8 = 2;

#[repr(C)]
struct RequestHeader {
    kind: u32,
    reserved: u32,
    sector: u64,
}

struct ReadRequest {
    header: RequestHeader,
    data: [u8; SECTOR_BYTES],
    status: u8,
}

impl ReadRequest {
    const fn sector_zero() -> Self {
        Self {
            header: RequestHeader {
                kind: READ.to_le(),
                reserved: 0,
                sector: 0,
            },
            data: [0; SECTOR_BYTES],
            status: u8::MAX,
        }
    }
}

struct BlockMemory {
    queue: RingMemory<4>,
    request: ReadRequest,
}

// Keep this memory reserved even if a device does not acknowledge reset.
static mut DMA: BlockMemory = BlockMemory {
    queue: RingMemory::empty(),
    request: ReadRequest::sector_zero(),
};

pub(crate) struct Probe {
    pub(crate) sectors: u64,
    pub(crate) prefix: [u8; 8],
}

pub(crate) enum ProbeError {
    Device,
    Empty,
    Read,
    InvalidCompletion,
    Timeout,
}

/// # Safety
/// Call once on the boot hart before paging on QEMU RV64 virt, with its MMIO
/// windows and the DMA storage accessible at their physical addresses.
pub(crate) unsafe fn probe() -> Option<Result<Probe, ProbeError>> {
    // SAFETY: The boot caller guarantees access to the virt transport windows.
    let device = unsafe { Device::discover(BLOCK_DEVICE) }?;
    let deadline = monotonic_nanos().saturating_add(PROBE_TIMEOUT_NS);
    if !device.reset(deadline) {
        return Some(Err(ProbeError::Timeout));
    }
    // SAFETY: Reset left the queue idle and the boot hart owns the static DMA storage.
    let result = unsafe { read_sector_zero(&device, deadline) };
    if !device.reset(deadline) {
        return Some(Err(ProbeError::Timeout));
    }
    Some(result)
}

/// # Safety
/// The caller must have reset the block device and own the resident DMA area.
unsafe fn read_sector_zero(device: &Device, deadline: u64) -> Result<Probe, ProbeError> {
    if !device.negotiate(BLOCK_READ_ONLY) {
        return Err(ProbeError::Device);
    }
    // SAFETY: Capacity is the 64-bit field at offset zero of the block configuration.
    let sectors = unsafe { device.config_u64(0, deadline) }.ok_or(ProbeError::Timeout)?;
    if sectors == 0 {
        return Err(ProbeError::Empty);
    }

    let memory = &raw mut DMA;
    // SAFETY: Reset completed, so the device has no access while we initialize these fields.
    let (mut queue, header, data, status) = unsafe {
        let queue = Ring::new(&raw mut (*memory).queue, REQUEST_QUEUE);
        let request = &raw mut (*memory).request;
        request.write(ReadRequest::sector_zero());
        (
            queue,
            &raw const (*request).header,
            &raw const (*request).data,
            &raw const (*request).status,
        )
    };
    if !queue.configure(device) || !device.start() {
        return Err(ProbeError::Device);
    }
    let descriptors = [
        Descriptor::new(header.addr(), size_of::<RequestHeader>(), NEXT, 1),
        Descriptor::new(data.addr(), SECTOR_BYTES, NEXT | DEVICE_WRITES, 2),
        Descriptor::new(status.addr(), 1, DEVICE_WRITES, 0),
    ];
    // SAFETY: The chain stays in reserved DMA memory and is read only after completion.
    unsafe { queue.submit(&descriptors) }.map_err(|_| ProbeError::Device)?;
    queue.notify(device);

    while monotonic_nanos() < deadline {
        if !device.operational() {
            return Err(ProbeError::Device);
        }
        let Some(length) = queue
            .complete()
            .map_err(|_| ProbeError::InvalidCompletion)?
        else {
            core::hint::spin_loop();
            continue;
        };
        if length == 0 {
            return Err(ProbeError::InvalidCompletion);
        }
        // SAFETY: The used entry transfers ownership of the request back to the driver.
        match unsafe { status.read_volatile() } {
            STATUS_OK if length == SECTOR_BYTES + 1 => {
                // SAFETY: The completion covers the full sector and the status byte.
                let prefix = unsafe { data.cast::<[u8; 8]>().read_volatile() };
                return Ok(Probe { sectors, prefix });
            }
            STATUS_IO_ERROR | STATUS_UNSUPPORTED => return Err(ProbeError::Read),
            _ => return Err(ProbeError::InvalidCompletion),
        }
    }
    Err(ProbeError::Timeout)
}
