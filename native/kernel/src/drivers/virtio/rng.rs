use clv_abi::net::CRYPTO_SEED_BYTES;

use super::mmio::Device;
use super::queue::{Queue, QueueError, QueueMemory};
use crate::arch::monotonic_nanos;

const ENTROPY_DEVICE: u32 = 4;
const REQUEST_QUEUE: u32 = 0;
const TRANSPORT_SEED_BYTES: usize = size_of::<u64>();
const PROBE_TIMEOUT_NS: u64 = 500_000_000;

// The kernel reserves this DMA storage even if the device fails to reset.
static mut ENTROPY: QueueMemory<CRYPTO_SEED_BYTES> = QueueMemory::empty();

#[derive(Default)]
pub(crate) struct BootSeeds {
    pub(crate) transport: u64,
    pub(crate) crypto: Option<[u8; CRYPTO_SEED_BYTES]>,
}

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
pub(crate) unsafe fn probe(want_crypto: bool) -> Option<Result<BootSeeds, ProbeError>> {
    // SAFETY: The caller supplies the virt transport mappings.
    let device = unsafe { Device::discover(ENTROPY_DEVICE) }?;
    let deadline = monotonic_nanos().saturating_add(PROBE_TIMEOUT_NS);
    if !device.reset(deadline) {
        return Some(Err(ProbeError::ResetTimeout));
    }
    // SAFETY: The boot hart owns this idle queue after the device reset.
    let mut queue = unsafe { Queue::new(&raw mut ENTROPY, REQUEST_QUEUE) };
    let seeds = read_seeds(&device, &mut queue, want_crypto, deadline);
    if !device.reset(deadline) {
        return Some(Err(ProbeError::ResetTimeout));
    }
    // SAFETY: The acknowledged reset returned exclusive ownership of the storage.
    unsafe { (&raw mut ENTROPY).write_volatile(QueueMemory::empty()) };
    Some(seeds)
}

fn read_seeds(
    device: &Device,
    queue: &mut Queue<CRYPTO_SEED_BYTES>,
    want_crypto: bool,
    deadline: u64,
) -> Result<BootSeeds, ProbeError> {
    if device.negotiate(0).is_none() || !queue.configure(device) || !device.start() {
        return Err(ProbeError::Device);
    }
    // Publish the first request after DRIVER_OK so starting the device and
    // notifying it cannot both request entropy for the same descriptor.
    let transport = u64::from_le_bytes(read_sample::<TRANSPORT_SEED_BYTES>(
        device, queue, deadline,
    )?);
    let crypto = if want_crypto {
        read_sample(device, queue, deadline).ok()
    } else {
        None
    };
    Ok(BootSeeds { transport, crypto })
}

fn read_sample<const N: usize>(
    device: &Device,
    queue: &mut Queue<CRYPTO_SEED_BYTES>,
    deadline: u64,
) -> Result<[u8; N], ProbeError> {
    if monotonic_nanos() >= deadline {
        return Err(ProbeError::Timeout);
    }
    queue.offer_receive_bytes(N)?;
    queue.notify(device);

    let mut seed = [0; N];
    let mut bytes = [0; CRYPTO_SEED_BYTES];
    let mut filled = 0;
    while monotonic_nanos() < deadline {
        if !device.operational() {
            return Err(ProbeError::Device);
        }
        if let Some(length) = queue.receive(&mut bytes)? {
            let end = filled + length;
            seed[filled..end].copy_from_slice(&bytes[..length]);
            filled = end;
            if filled == N {
                return Ok(seed);
            }
            queue.offer_receive_bytes(N - filled)?;
            queue.notify(device);
        }
        core::hint::spin_loop();
    }
    Err(ProbeError::Timeout)
}
