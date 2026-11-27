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
    pub(crate) unsafe fn probe(self) -> Result<Port, ProbeError> {
        let deadline = monotonic_nanos().saturating_add(PROBE_TIMEOUT_NS);
        if !self.0.reset(deadline) {
            return Err(ProbeError::Timeout);
        }
        // SAFETY: Reset left the queues idle. The boot probe owns both static buffers.
        let mut port = Port {
            device: self.0,
            receive: unsafe { Queue::new(&raw mut RECEIVE, RECEIVE_QUEUE) },
            transmit: unsafe { Queue::new(&raw mut TRANSMIT, TRANSMIT_QUEUE) },
        };
        if let Err(error) = port.start().and_then(|()| port.check_peer(deadline)) {
            port.device.reset(deadline);
            return Err(error);
        }
        Ok(port)
    }
}

pub(crate) struct Port {
    device: Device,
    receive: Queue,
    transmit: Queue,
}

impl Port {
    fn start(&mut self) -> Result<(), ProbeError> {
        let device = &self.device;
        if device.negotiate(0).is_none()
            || !self.receive.configure(device)
            || !self.transmit.configure(device)
        {
            return Err(ProbeError::Device);
        }
        // Have a receive descriptor in place before the host can answer.
        self.receive.offer_receive()?;
        if !device.start() {
            return Err(ProbeError::Device);
        }
        self.receive.notify(device);
        Ok(())
    }

    fn check_peer(&mut self, deadline: u64) -> Result<(), ProbeError> {
        if !self.try_send(PROBE_REQUEST)? {
            return Err(ProbeError::Device);
        }
        let mut expected = PROBE_RESPONSE;
        let mut bytes = [0; BUFFER_BYTES];
        while monotonic_nanos() < deadline {
            if !expected.is_empty()
                && let Some(length) = self.try_read(&mut bytes)?
            {
                expected = expected
                    .strip_prefix(&bytes[..length])
                    .ok_or(ProbeError::InvalidReply)?;
            }
            if expected.is_empty() && self.transmit_ready()? {
                return Ok(());
            }
            core::hint::spin_loop();
        }
        Err(ProbeError::Timeout)
    }

    fn transmit_ready(&mut self) -> Result<bool, ProbeError> {
        if !self.device.operational() {
            return Err(ProbeError::Device);
        }
        self.transmit.complete()?;
        Ok(!self.transmit.is_pending())
    }

    pub(crate) fn try_send(&mut self, bytes: &[u8]) -> Result<bool, ProbeError> {
        if !self.transmit_ready()? {
            return Ok(false);
        }
        self.transmit.offer_send(bytes)?;
        self.transmit.notify(&self.device);
        Ok(true)
    }

    pub(crate) fn try_read(
        &mut self,
        bytes: &mut [u8; BUFFER_BYTES],
    ) -> Result<Option<usize>, ProbeError> {
        if !self.device.operational() {
            return Err(ProbeError::Device);
        }
        let length = self.receive.receive(bytes)?;
        if length.is_some() {
            self.receive.offer_receive()?;
            self.receive.notify(&self.device);
        }
        Ok(length)
    }
}
