use super::mmio::Device;
use super::queue::{Queue, QueueError, QueueMemory};
use crate::arch::monotonic_nanos;

const ENTROPY_DEVICE: u32 = 4;
const REQUEST_QUEUE: u32 = 0;
const SEED_BYTES: usize = size_of::<u64>();
const PROBE_TIMEOUT_NS: u64 = 500_000_000;

// The kernel reserves this DMA storage even if the device fails to reset.
static mut ENTROPY: QueueMemory<SEED_BYTES> = QueueMemory::empty();

pub(crate) enum ProbeError {
    Device,
    Queue,
    Timeout,
    ResetTimeout,
}

impl From<QueueError> for ProbeError {
    fn from(_: QueueError) -> Self {
        Self::Queue
    }
}

/// # Safety
/// The boot hart must call this at most once before paging, with the QEMU
/// virt MMIO windows accessible and the reserved DMA storage identity-mapped.
pub(crate) unsafe fn probe() -> Option<Result<u64, ProbeError>> {
    // SAFETY: The caller supplies the virt transport mappings.
    let device = unsafe { Device::discover(ENTROPY_DEVICE) }?;
    let deadline = monotonic_nanos().saturating_add(PROBE_TIMEOUT_NS);
    if !device.reset(deadline) {
        return Some(Err(ProbeError::ResetTimeout));
    }
    // SAFETY: The boot hart owns this idle queue after the device reset.
    let mut queue = unsafe { Queue::new(&raw mut ENTROPY, REQUEST_QUEUE) };
    let seed = read_seed(&device, &mut queue, deadline);
    if !device.reset(deadline) {
        return Some(Err(ProbeError::ResetTimeout));
    }
    // SAFETY: The acknowledged reset returned exclusive ownership of the storage.
    unsafe { (&raw mut ENTROPY).write_volatile(QueueMemory::empty()) };
    Some(seed)
}

fn read_seed(
    device: &Device,
    queue: &mut Queue<SEED_BYTES>,
    deadline: u64,
) -> Result<u64, ProbeError> {
    if device.negotiate(0).is_none() || !queue.configure(device) || !device.start() {
        return Err(ProbeError::Device);
    }
    // Publish the first request after DRIVER_OK so starting the device and
    // notifying it cannot both request entropy for the same descriptor.
    queue.offer_receive()?;
    queue.notify(device);

    let mut seed = [0; SEED_BYTES];
    let mut bytes = [0; SEED_BYTES];
    let mut filled = 0;
    while monotonic_nanos() < deadline {
        if !device.operational() {
            return Err(ProbeError::Device);
        }
        if let Some(length) = queue.receive(&mut bytes)? {
            let end = filled + length;
            seed[filled..end].copy_from_slice(&bytes[..length]);
            filled = end;
            if filled == SEED_BYTES {
                return Ok(u64::from_le_bytes(seed));
            }
            queue.offer_receive_bytes(SEED_BYTES - filled)?;
            queue.notify(device);
        }
        core::hint::spin_loop();
    }
    Err(ProbeError::Timeout)
}
