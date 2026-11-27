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
const BUFFER_BYTES: usize = NETWORK_HEADER_BYTES + 1514;
const FRAME_BYTES: usize = 60;
const PROBE_ETHERTYPE: [u8; 2] = 0x88b5_u16.to_be_bytes();
const PROBE_PEER: [u8; 6] = [0x02, 0, 0, 0, 0, 1];
const PROBE_REQUEST: &[u8; 8] = b"CLVNET01";
const PROBE_REPLY: &[u8; 8] = b"CLVNET02";

// Keep both buffers reserved even if the boot probe cannot reset the device.
static mut RECEIVE: QueueMemory<BUFFER_BYTES> = QueueMemory::empty();
static mut TRANSMIT: QueueMemory<BUFFER_BYTES> = QueueMemory::empty();

enum ProbeError {
    Device,
    ConfigTimeout,
    Queue,
    ReceiveTimeout,
    TransmitTimeout,
}

impl From<QueueError> for ProbeError {
    fn from(_: QueueError) -> Self {
        Self::Queue
    }
}

/// # Safety
/// Call once on the QEMU RV64 virt boot hart with the MMIO windows and the
/// resident DMA storage accessible at their physical addresses.
pub(crate) unsafe fn boot_probe() {
    // SAFETY: The caller guarantees access to the transport windows.
    let Some(device) = (unsafe { Device::discover(NETWORK_DEVICE) }) else {
        return;
    };
    if !device.reset(deadline()) {
        uart::write(b"virtio net reset timed out\r\n");
        return;
    }

    // SAFETY: Reset returned ownership of the queues and their resident buffers.
    let mut receive = unsafe { Queue::new(&raw mut RECEIVE, RECEIVE_QUEUE) };
    let mut transmit = unsafe { Queue::new(&raw mut TRANSMIT, TRANSMIT_QUEUE) };
    if let Err(error) = exchange(&device, &mut receive, &mut transmit) {
        uart::write(match error {
            ProbeError::Device => b"virtio net unavailable\r\n",
            ProbeError::ConfigTimeout => b"virtio net configuration timed out\r\n",
            ProbeError::Queue => b"virtio net queue error\r\n",
            ProbeError::ReceiveTimeout => b"virtio net receive timed out\r\n",
            ProbeError::TransmitTimeout => b"virtio net transmit timed out\r\n",
        });
    }
    if !device.reset(deadline()) {
        uart::write(b"virtio net reset timed out\r\n");
    }
}

fn exchange(
    device: &Device,
    receive: &mut Queue<BUFFER_BYTES>,
    transmit: &mut Queue<BUFFER_BYTES>,
) -> Result<(), ProbeError> {
    if device.negotiate(MAC_FEATURE) != Some(MAC_FEATURE) {
        return Err(ProbeError::Device);
    }
    // SAFETY: Negotiating VIRTIO_NET_F_MAC exposes the six MAC bytes at offset zero.
    let mac: [u8; 6] =
        unsafe { device.config_bytes(0, deadline()) }.ok_or(ProbeError::ConfigTimeout)?;
    if !receive.configure(device) || !transmit.configure(device) {
        return Err(ProbeError::Device);
    }
    receive.offer_receive()?;
    if !device.start() {
        return Err(ProbeError::Device);
    }
    receive.notify(device);
    report_mac(mac);

    receive_probe(
        device,
        receive,
        &probe_frame(mac, PROBE_PEER, PROBE_REQUEST),
    )?;
    uart::write(b"virtio net received probe\r\n");

    // The zero header requests ordinary Ethernet transmission without offloads.
    let mut packet = [0; NETWORK_HEADER_BYTES + FRAME_BYTES];
    packet[NETWORK_HEADER_BYTES..].copy_from_slice(&probe_frame(PROBE_PEER, mac, PROBE_REPLY));
    transmit.offer_send(&packet)?;
    transmit.notify(device);
    let expires = deadline();
    while monotonic_nanos() < expires {
        if !device.operational() {
            return Err(ProbeError::Device);
        }
        if transmit.complete()?.is_some() {
            uart::write(b"virtio net sent probe reply\r\n");
            return Ok(());
        }
        core::hint::spin_loop();
    }
    Err(ProbeError::TransmitTimeout)
}

fn receive_probe(
    device: &Device,
    receive: &mut Queue<BUFFER_BYTES>,
    request: &[u8; FRAME_BYTES],
) -> Result<(), ProbeError> {
    let mut packet = [0; BUFFER_BYTES];
    let expires = deadline();
    while monotonic_nanos() < expires {
        if !device.operational() {
            return Err(ProbeError::Device);
        }
        if let Some(length) = receive.receive(&mut packet)? {
            // A valid completion owns just one buffer. Check the entire Ethernet frame.
            if packet.get(NETWORK_HEADER_BYTES..length) == Some(request.as_slice()) {
                return Ok(());
            }
            receive.offer_receive()?;
            receive.notify(device);
        }
        core::hint::spin_loop();
    }
    Err(ProbeError::ReceiveTimeout)
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
