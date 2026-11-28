#![no_std]
#![no_main]

use clv::net;
use core::panic::PanicInfo;
use smoltcp::iface::{Config, Interface, SocketHandle, SocketSet, SocketStorage};
use smoltcp::phy::{Device, DeviceCapabilities, Medium, RxToken, TxToken};
use smoltcp::socket::{Socket, tcp, udp};
use smoltcp::time::Instant;
use smoltcp::wire::{
    ETHERNET_HEADER_LEN, EthernetAddress, HardwareAddress, IPV4_HEADER_LEN, IpAddress, IpCidr,
    IpEndpoint, Ipv4Address, UDP_HEADER_LEN,
};

// Relink the service when the boot user linker script changes.
const _: &str = include_str!("../user.ld");

const GUEST_ADDRESS: IpAddress = IpAddress::Ipv4(Ipv4Address::new(192, 0, 2, 2));
const UDP_ECHO_ENDPOINT: IpEndpoint = IpEndpoint::new(GUEST_ADDRESS, 49152);
const TCP_ECHO_ENDPOINT: IpEndpoint = IpEndpoint::new(GUEST_ADDRESS, 49153);
const TCP_BUFFER_BYTES: usize = 1024;
const MAX_UDP_PAYLOAD_BYTES: usize =
    net::MAX_FRAME_BYTES - ETHERNET_HEADER_LEN - IPV4_HEADER_LEN - UDP_HEADER_LEN;
const NANOS_PER_MICROSECOND: u64 = 1_000;
const MIN_FRAME_BYTES: usize = 60;
const PROBE_ETHERTYPE: [u8; 2] = 0x88b5_u16.to_be_bytes();
const PROBE_PEER: [u8; 6] = [0x02, 0, 0, 0, 0, 1];
const PROBE_REQUEST: &[u8; 8] = b"CLVNET01";
const PROBE_REPLY: &[u8; 8] = b"CLVNET02";

// SAFETY: The kernel enters this unique symbol with an aligned user stack.
#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
extern "C" fn _start(_task: usize, packed_mac: usize) -> ! {
    let [a, b, c, d, e, f, _, _] = (packed_mac as u64).to_le_bytes();
    let mac = [a, b, c, d, e, f];
    let request = probe_frame(mac, PROBE_PEER, PROBE_REQUEST);
    let reply = probe_frame(PROBE_PEER, mac, PROBE_REPLY);
    let mut buffer = [0; net::MAX_FRAME_BYTES];
    let config = Config::new(HardwareAddress::Ethernet(EthernetAddress(mac)));
    let mut interface = Interface::new(config, &mut EthernetDevice(None), Instant::ZERO);
    interface.update_ip_addrs(|addrs| addrs.push(IpCidr::new(GUEST_ADDRESS, 24)).unwrap());

    let mut rx_packets = [udp::PacketMetadata::EMPTY];
    let mut rx_payload = [0; MAX_UDP_PAYLOAD_BYTES];
    let mut tx_packets = [udp::PacketMetadata::EMPTY];
    let mut tx_payload = [0; MAX_UDP_PAYLOAD_BYTES];
    let mut udp_socket = udp::Socket::new(
        udp::PacketBuffer::new(&mut rx_packets[..], &mut rx_payload[..]),
        udp::PacketBuffer::new(&mut tx_packets[..], &mut tx_payload[..]),
    );
    udp_socket.bind(UDP_ECHO_ENDPOINT).unwrap();

    let mut tcp_rx = [0; TCP_BUFFER_BYTES];
    let mut tcp_tx = [0; TCP_BUFFER_BYTES];
    let mut tcp_socket = tcp::Socket::new(
        tcp::SocketBuffer::new(&mut tcp_rx[..]),
        tcp::SocketBuffer::new(&mut tcp_tx[..]),
    );
    tcp_socket.listen(TCP_ECHO_ENDPOINT).unwrap();

    let mut storage = [SocketStorage::EMPTY; 2];
    let mut sockets = SocketSet::new(&mut storage[..]);
    let mut udp_handle = sockets.add(udp_socket);
    let tcp_handle = sockets.add(tcp_socket);

    clv::print(b"netd ready\r\n");
    let mut deadline = None;
    loop {
        let Some((frame, now)) = net::receive(&mut buffer, deadline) else {
            clv::exit(1);
        };
        let timestamp = Instant::from_micros((now / NANOS_PER_MICROSECOND) as i64);
        interface.poll_maintenance(timestamp);
        if frame == request {
            send_frame(&reply);
        } else if !frame.is_empty() {
            interface.poll_ingress_single(
                timestamp,
                &mut EthernetDevice(Some(frame)),
                &mut sockets,
            );
            // Ingress has copied the datagram into the socket queue, freeing the frame buffer.
            udp_handle = queue_udp_echo(
                &mut sockets,
                udp_handle,
                &mut buffer[..MAX_UDP_PAYLOAD_BYTES],
            );
        }
        queue_tcp_echo(sockets.get_mut::<tcp::Socket>(tcp_handle), &mut buffer);
        interface.poll_egress(timestamp, &mut EthernetDevice(None), &mut sockets);

        let socket = sockets.get_mut::<tcp::Socket>(tcp_handle);
        if socket.state() == tcp::State::Closed {
            socket.listen(TCP_ECHO_ENDPOINT).unwrap();
        }
        deadline = interface.poll_at(timestamp, &sockets).map(|at| {
            // smoltcp uses zero for work due now. The ABI reserves zero for an indefinite wait.
            (at.total_micros().max(0) as u64)
                .saturating_mul(NANOS_PER_MICROSECOND)
                .max(1)
        });
    }
}

fn queue_udp_echo(
    sockets: &mut SocketSet<'_>,
    echo: SocketHandle,
    buffer: &mut [u8],
) -> SocketHandle {
    let socket = sockets.get_mut::<udp::Socket>(echo);
    let Ok((length, sender)) = socket.recv_slice(buffer) else {
        return echo;
    };
    if sender.local_address != Some(GUEST_ADDRESS)
        || !sender.endpoint.addr.is_unicast()
        || sender.endpoint.port == 0
    {
        return echo;
    }
    let echo = if socket.can_send() {
        echo
    } else {
        // Discard a reply still waiting for ARP and reset the socket's neighbor wait.
        let Socket::Udp(mut socket) = sockets.remove(echo) else {
            unreachable!();
        };
        socket.close();
        socket.bind(UDP_ECHO_ENDPOINT).unwrap();
        sockets.add(socket)
    };
    let socket = sockets.get_mut::<udp::Socket>(echo);
    let _ = socket.send_slice(&buffer[..length], sender.endpoint);
    echo
}

fn queue_tcp_echo(socket: &mut tcp::Socket<'_>, buffer: &mut [u8]) {
    if socket.can_recv() && socket.can_send() {
        let available = socket.send_capacity() - socket.send_queue();
        let capacity = available.min(buffer.len());
        let length = socket.recv_slice(&mut buffer[..capacity]).unwrap();
        let queued = socket.send_slice(&buffer[..length]).unwrap();
        debug_assert_eq!(queued, length);
    }
    if socket.state() == tcp::State::CloseWait && !socket.can_recv() {
        socket.close();
    }
}

struct EthernetDevice<'a>(Option<&'a [u8]>);

impl Device for EthernetDevice<'_> {
    type RxToken<'a>
        = ReceiveToken<'a>
    where
        Self: 'a;
    type TxToken<'a>
        = TransmitToken
    where
        Self: 'a;

    fn receive(&mut self, _timestamp: Instant) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
        self.0
            .take()
            .map(|frame| (ReceiveToken(frame), TransmitToken))
    }

    fn transmit(&mut self, _timestamp: Instant) -> Option<Self::TxToken<'_>> {
        Some(TransmitToken)
    }

    fn capabilities(&self) -> DeviceCapabilities {
        let mut capabilities = DeviceCapabilities::default();
        capabilities.medium = Medium::Ethernet;
        capabilities.max_transmission_unit = net::MAX_FRAME_BYTES;
        capabilities.max_burst_size = Some(1);
        capabilities
    }
}

struct ReceiveToken<'a>(&'a [u8]);

impl RxToken for ReceiveToken<'_> {
    fn consume<R, F: FnOnce(&[u8]) -> R>(self, read: F) -> R {
        read(self.0)
    }
}

struct TransmitToken;

impl TxToken for TransmitToken {
    fn consume<R, F: FnOnce(&mut [u8]) -> R>(self, length: usize, write: F) -> R {
        let mut buffer = [0; net::MAX_FRAME_BYTES];
        let result = write(&mut buffer[..length]);
        send_frame(&buffer[..length.max(MIN_FRAME_BYTES)]);
        result
    }
}

fn send_frame(frame: &[u8]) {
    loop {
        match net::transmit(frame) {
            Some(0) => core::hint::spin_loop(),
            Some(_) => return,
            None => clv::exit(1),
        }
    }
}

fn probe_frame(destination: [u8; 6], source: [u8; 6], marker: &[u8; 8]) -> [u8; MIN_FRAME_BYTES] {
    let mut frame = [0; MIN_FRAME_BYTES];
    frame[..6].copy_from_slice(&destination);
    frame[6..12].copy_from_slice(&source);
    frame[12..14].copy_from_slice(&PROBE_ETHERTYPE);
    frame[14..22].copy_from_slice(marker);
    frame
}

#[panic_handler]
fn panic(_info: &PanicInfo<'_>) -> ! {
    clv::exit(1)
}
