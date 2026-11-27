use super::mmio::Device;
use super::queue::{BUFFER_BYTES, Queue, QueueError, QueueMemory};
use crate::arch::monotonic_nanos;

const CONSOLE_DEVICE: u32 = 3;
const RECEIVE_QUEUE: u32 = 0;
const TRANSMIT_QUEUE: u32 = 1;
const PROBE_TIMEOUT_NS: u64 = 500_000_000;
const PROBE_REQUEST: &[u8] = b"PING";
const PROBE_RESPONSE: &[u8] = b"PONG";

// The DMA storage remains reserved even if a device fails to reset.
static mut RECEIVE: QueueMemory = QueueMemory::empty();
static mut TRANSMIT: QueueMemory = QueueMemory::empty();

pub(crate) enum ProbeError {
    Device,
    InvalidReply,
    Timeout,
}

impl From<QueueError> for ProbeError {
    fn from(_: QueueError) -> Self {
        Self::Device
    }
}

pub(crate) struct Console(Device);

impl Console {
    /// # Safety
    /// Call on QEMU RV64 virt before enabling paging, with its MMIO windows accessible.
    pub(crate) unsafe fn discover() -> Option<Self> {
        // SAFETY: The boot caller guarantees access to the virt transport windows.
        unsafe { Device::discover(CONSOLE_DEVICE) }.map(Self)
    }

    pub(crate) fn base(&self) -> usize {
        self.0.base()
    }

    /// # Safety
    /// Only the boot hart may probe, once, with the selected device and kernel
    /// DMA storage identity-mapped for the supervisor.
    pub(crate) unsafe fn probe(self) -> Result<(), ProbeError> {
        let deadline = monotonic_nanos().saturating_add(PROBE_TIMEOUT_NS);
        if !self.0.reset(deadline) {
            return Err(ProbeError::Timeout);
        }
        // SAFETY: Reset left the queues idle. The boot probe owns both static buffers.
        let mut receive = unsafe { Queue::new(&raw mut RECEIVE, RECEIVE_QUEUE) };
        let mut transmit = unsafe { Queue::new(&raw mut TRANSMIT, TRANSMIT_QUEUE) };
        let result = self.exchange(&mut receive, &mut transmit, deadline);
        let reset = self.0.reset(deadline);
        result?;
        reset.then_some(()).ok_or(ProbeError::Timeout)
    }

    fn exchange(
        &self,
        receive: &mut Queue,
        transmit: &mut Queue,
        deadline: u64,
    ) -> Result<(), ProbeError> {
        let device = &self.0;
        if !device.negotiate() || !receive.configure(device) || !transmit.configure(device) {
            return Err(ProbeError::Device);
        }
        // Have a receive descriptor in place before the host can answer.
        receive.offer_receive()?;
        transmit.offer_send(PROBE_REQUEST)?;
        if !device.start() {
            return Err(ProbeError::Device);
        }
        receive.notify(device);
        transmit.notify(device);

        let mut sent = false;
        let mut expected = PROBE_RESPONSE;
        let mut bytes = [0; BUFFER_BYTES];
        while monotonic_nanos() < deadline {
            if !device.operational() {
                return Err(ProbeError::Device);
            }
            if !sent {
                sent = transmit.complete()?.is_some();
            }
            if !expected.is_empty()
                && let Some(length) = receive.receive(&mut bytes)?
            {
                expected = expected
                    .strip_prefix(&bytes[..length])
                    .ok_or(ProbeError::InvalidReply)?;
                if !expected.is_empty() {
                    receive.offer_receive()?;
                    receive.notify(device);
                }
            }
            if sent && expected.is_empty() {
                return Ok(());
            }
            core::hint::spin_loop();
        }
        Err(ProbeError::Timeout)
    }
}
