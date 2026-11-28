use super::mmio::Device;
use super::queue::{Queue, QueueError, QueueMemory};
use crate::arch::monotonic_nanos;
use crate::drivers::uart;

const NETWORK_DEVICE: u32 = 1;
const MAC_FEATURE: u32 = 1 << 5;
const RECEIVE_QUEUE: u32 = 0;
const TRANSMIT_QUEUE: u32 = 1;
const TIMEOUT_NS: u64 = 500_000_000;

// The modern header and a standard Ethernet frame without its FCS.
const NETWORK_HEADER_BYTES: usize = 12;
const ETHERNET_HEADER_BYTES: usize = 14;
const MAX_FRAME_BYTES: usize = ETHERNET_HEADER_BYTES + 1500;
const BUFFER_BYTES: usize = NETWORK_HEADER_BYTES + MAX_FRAME_BYTES;
const FRAME_BYTES: usize = 60;
const PROBE_ETHERTYPE: [u8; 2] = 0x88b5_u16.to_be_bytes();
const PROBE_PEER: [u8; 6] = [0x02, 0, 0, 0, 0, 1];
const PROBE_REQUEST: &[u8; 8] = b"CLVNET01";
const PROBE_REPLY: &[u8; 8] = b"CLVNET02";

// Keep both buffers reserved even if the device does not acknowledge reset.
static mut RECEIVE: QueueMemory<BUFFER_BYTES> = QueueMemory::empty();
static mut TRANSMIT: QueueMemory<BUFFER_BYTES> = QueueMemory::empty();

enum NetError {
    Device,
    ResetTimeout,
    ConfigTimeout,
    Queue,
    FrameLength,
    TransmitTimeout,
}

impl NetError {
    fn report(self) {
        uart::write(match self {
            Self::Device => b"virtio net unavailable\r\n",
            Self::ResetTimeout => b"virtio net reset timed out\r\n",
            Self::ConfigTimeout => b"virtio net configuration timed out\r\n",
            Self::Queue => b"virtio net queue error\r\n",
            Self::FrameLength => b"virtio net invalid transmit length\r\n",
            Self::TransmitTimeout => b"virtio net transmit timed out\r\n",
        });
    }
}

impl From<QueueError> for NetError {
    fn from(_: QueueError) -> Self {
        Self::Queue
    }
}

pub(crate) struct Nic {
    device: Device,
    receive: Queue<BUFFER_BYTES>,
    transmit: Queue<BUFFER_BYTES>,
    mac: [u8; 6],
    transmit_deadline: u64,
}

impl Nic {
    /// # Safety
    /// Call once on the QEMU RV64 virt boot hart with the transport windows
    /// accessible. Keep the NIC's MMIO window and DMA storage identity-mapped.
    pub(crate) unsafe fn attach() -> Option<Self> {
        // SAFETY: The caller guarantees access to the transport windows.
        let device = unsafe { Device::discover(NETWORK_DEVICE) }?;
        if !device.reset(deadline()) {
            NetError::ResetTimeout.report();
            return None;
        }
        // SAFETY: Reset returned ownership of the queues and their resident buffers.
        let mut nic = Self {
            device,
            receive: unsafe { Queue::new(&raw mut RECEIVE, RECEIVE_QUEUE) },
            transmit: unsafe { Queue::new(&raw mut TRANSMIT, TRANSMIT_QUEUE) },
            mac: [0; 6],
            transmit_deadline: 0,
        };
        if let Err(error) = nic.start() {
            error.report();
            if !nic.device.reset(deadline()) {
                NetError::ResetTimeout.report();
            }
            return None;
        }
        report_mac(nic.mac);
        Some(nic)
    }

    pub(crate) fn base(&self) -> usize {
        self.device.base()
    }

    fn start(&mut self) -> Result<(), NetError> {
        if self.device.negotiate(MAC_FEATURE) != Some(MAC_FEATURE) {
            return Err(NetError::Device);
        }
        // SAFETY: Negotiating VIRTIO_NET_F_MAC exposes the six MAC bytes at offset zero.
        self.mac =
            unsafe { self.device.config_bytes(0, deadline()) }.ok_or(NetError::ConfigTimeout)?;
        if !self.receive.configure(&self.device) || !self.transmit.configure(&self.device) {
            return Err(NetError::Device);
        }
        self.receive.offer_receive()?;
        if !self.device.start() {
            return Err(NetError::Device);
        }
        self.receive.notify(&self.device);
        Ok(())
    }

    fn transmit_ready(&mut self) -> Result<bool, NetError> {
        if !self.device.operational() {
            return Err(NetError::Device);
        }
        self.transmit.complete()?;
        if !self.transmit.is_pending() {
            return Ok(true);
        }
        if monotonic_nanos() >= self.transmit_deadline {
            return Err(NetError::TransmitTimeout);
        }
        Ok(false)
    }

    fn receive_frame<'a>(
        &mut self,
        packet: &'a mut [u8; BUFFER_BYTES],
    ) -> Result<Option<&'a [u8]>, NetError> {
        let Some(length) = self.receive.receive(packet)? else {
            return Ok(None);
        };
        self.receive.offer_receive()?;
        self.receive.notify(&self.device);
        Ok(packet
            .get(NETWORK_HEADER_BYTES..length)
            .filter(|frame| frame.len() >= ETHERNET_HEADER_BYTES))
    }

    fn send_frame(&mut self, frame: &[u8]) -> Result<(), NetError> {
        if !(ETHERNET_HEADER_BYTES..=MAX_FRAME_BYTES).contains(&frame.len()) {
            return Err(NetError::FrameLength);
        }
        // The zero header requests ordinary Ethernet transmission without offloads.
        let mut packet = [0; BUFFER_BYTES];
        let length = NETWORK_HEADER_BYTES + frame.len();
        packet[NETWORK_HEADER_BYTES..length].copy_from_slice(frame);
        self.transmit.offer_send(&packet[..length])?;
        self.transmit.notify(&self.device);
        self.transmit_deadline = deadline();
        Ok(())
    }
}

/// Service at most one frame on the boot hart's timer tick.
pub(crate) fn poll(interface: &mut Option<Nic>) {
    if let Some(nic) = interface.as_mut()
        && let Err(error) = poll_probe(nic)
    {
        // Leave the reserved DMA memory untouched while reset takes effect.
        nic.device.request_reset();
        error.report();
        *interface = None;
    }
}

fn poll_probe(nic: &mut Nic) -> Result<(), NetError> {
    // A busy transmitter leaves the receive descriptor available for the next tick.
    if !nic.transmit_ready()? {
        return Ok(());
    }
    let mac = nic.mac;
    let mut packet = [0; BUFFER_BYTES];
    if let Some(frame) = nic.receive_frame(&mut packet)?
        && frame == probe_frame(mac, PROBE_PEER, PROBE_REQUEST)
    {
        nic.send_frame(&probe_frame(PROBE_PEER, mac, PROBE_REPLY))?;
    }
    Ok(())
}

fn probe_frame(destination: [u8; 6], source: [u8; 6], marker: &[u8; 8]) -> [u8; FRAME_BYTES] {
    let mut frame = [0; FRAME_BYTES];
    frame[..6].copy_from_slice(&destination);
    frame[6..12].copy_from_slice(&source);
    frame[12..14].copy_from_slice(&PROBE_ETHERTYPE);
    frame[14..22].copy_from_slice(marker);
    frame
}

fn report_mac(mac: [u8; 6]) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut address = [b':'; 17];
    for (index, byte) in mac.into_iter().enumerate() {
        address[index * 3] = HEX[(byte >> 4) as usize];
        address[index * 3 + 1] = HEX[(byte & 0xf) as usize];
    }
    uart::write(b"virtio net ready mac=");
    uart::write(&address);
    uart::write(b"\r\n");
}

fn deadline() -> u64 {
    monotonic_nanos().saturating_add(TIMEOUT_NS)
}
