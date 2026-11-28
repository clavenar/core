use core::ops::Range;
use rand_chacha::ChaCha20Rng;
use rand_chacha::rand_core::RngCore;
use smoltcp::iface::{Interface, SocketHandle, SocketSet};
use smoltcp::socket::tcp;
use smoltcp::time::{Duration, Instant};
use smoltcp::wire::{IpAddress, IpEndpoint, Ipv4Address};

use super::http_response::{ExpectedResponse, RESPONSE_BYTES, body_range};

const PEER: IpEndpoint = IpEndpoint::new(IpAddress::Ipv4(Ipv4Address::new(192, 0, 2, 1)), 8080);
const FALLBACK_PORT: u16 = 49154;
const REQUEST: &[u8] = b"GET /probe HTTP/1.1\r\nHost: 192.0.2.1:8080\r\nConnection: close\r\n\r\n";
const BODY: &[u8] = b"Hello from native HTTP\n";
const TIMEOUT: Duration = Duration::from_secs(5);
pub(super) const SOCKET_BYTES: usize = 512;

#[derive(Clone, Copy)]
enum Phase {
    Pending,
    Exchanging(Instant),
    Closing(Instant),
    Done,
}

enum Failure {
    Connection,
    Timeout,
    Truncated,
    Invalid,
    Body,
}

impl Failure {
    const fn message(self) -> &'static [u8] {
        match self {
            Self::Connection => b"netd HTTP probe connection failed\r\n",
            Self::Timeout => b"netd HTTP probe timed out\r\n",
            Self::Truncated => b"netd HTTP probe truncated response\r\n",
            Self::Invalid => b"netd HTTP probe invalid response\r\n",
            Self::Body => b"netd HTTP probe unexpected body\r\n",
        }
    }
}

pub(super) struct Probe<'a> {
    handle: SocketHandle,
    local_port: u16,
    phase: Phase,
    sent: usize,
    bytes: &'a mut [u8; RESPONSE_BYTES],
    received: usize,
    body: Option<Range<usize>>,
}

impl<'a> Probe<'a> {
    pub(super) fn new(
        handle: SocketHandle,
        bytes: &'a mut [u8; RESPONSE_BYTES],
        rng: Option<&mut ChaCha20Rng>,
    ) -> Self {
        let local_port = if let Some(rng) = rng {
            // Use the upper half of the dynamic port range for the probe.
            let port = 0xe000 | (rng.next_u32() as u16 & 0x1fff);
            clv::print(b"netd crypto entropy ready\r\n");
            port
        } else {
            clv::print(b"netd crypto entropy unavailable\r\n");
            FALLBACK_PORT
        };
        Self {
            handle,
            local_port,
            phase: Phase::Pending,
            sent: 0,
            bytes,
            received: 0,
            body: None,
        }
    }

    pub(super) fn deadline(&self) -> Option<Instant> {
        match self.phase {
            Phase::Exchanging(deadline) | Phase::Closing(deadline) => Some(deadline),
            Phase::Pending | Phase::Done => None,
        }
    }

    pub(super) fn poll(
        &mut self,
        now: Instant,
        unix_nanos: Option<u64>,
        iface: &mut Interface,
        sockets: &mut SocketSet<'_>,
    ) {
        if matches!(self.phase, Phase::Done) {
            return;
        }
        if matches!(self.phase, Phase::Pending) {
            report_unix_time(b"netd HTTP probe unix_ns_start=", unix_nanos);
        }

        let socket = sockets.get_mut::<tcp::Socket>(self.handle);
        match self.advance(now, iface, socket) {
            Ok(false) => {}
            Ok(true) => {
                self.phase = Phase::Done;
                report_unix_time(b"netd HTTP probe unix_ns_done=", unix_nanos);
                clv::print(b"netd HTTP probe received Hello from native HTTP\r\n");
            }
            Err(failure) => {
                // Drop the socket to stop retries even when the peer never answers ARP.
                sockets.remove(self.handle);
                self.phase = Phase::Done;
                clv::print(failure.message());
            }
        }
    }

    fn advance(
        &mut self,
        now: Instant,
        iface: &mut Interface,
        socket: &mut tcp::Socket<'_>,
    ) -> Result<bool, Failure> {
        let deadline = match self.phase {
            Phase::Pending => {
                socket
                    .connect(iface.context(), PEER, self.local_port)
                    .map_err(|_| Failure::Connection)?;
                self.phase = Phase::Exchanging(now + TIMEOUT);
                return Ok(false);
            }
            Phase::Exchanging(deadline) | Phase::Closing(deadline) => deadline,
            Phase::Done => return Ok(false),
        };
        if now >= deadline {
            return Err(Failure::Timeout);
        }
        if matches!(self.phase, Phase::Closing(_)) {
            if socket.can_recv() {
                return Err(Failure::Invalid);
            }
            return Ok(matches!(
                socket.state(),
                tcp::State::Closed | tcp::State::TimeWait
            ));
        }
        if socket.state() == tcp::State::Closed {
            return Err(if self.sent == 0 {
                Failure::Connection
            } else {
                Failure::Truncated
            });
        }

        if self.sent < REQUEST.len() && socket.can_send() {
            self.sent += socket
                .send_slice(&REQUEST[self.sent..])
                .map_err(|_| Failure::Connection)?;
        }
        if socket.can_recv() {
            self.received += socket
                .recv_slice(&mut self.bytes[self.received..])
                .map_err(|_| Failure::Connection)?;
            if self.body.is_none() {
                self.body = body_range(&self.bytes[..self.received], ExpectedResponse::Ok)
                    .map_err(|_| Failure::Invalid)?;
            }
            if let Some(body) = &self.body
                && self.received >= body.end
            {
                if self.received != body.end || socket.can_recv() {
                    return Err(Failure::Invalid);
                }
                if &self.bytes[body.clone()] != BODY {
                    return Err(Failure::Body);
                }
                socket.close();
                self.phase = Phase::Closing(deadline);
                return Ok(false);
            }
        }
        if socket.state() == tcp::State::CloseWait && !socket.can_recv() {
            return Err(Failure::Truncated);
        }
        Ok(false)
    }
}

fn report_unix_time(label: &[u8], unix_nanos: Option<u64>) {
    let mut hex = *b"0x0000000000000000\r\n";
    let timestamp: &[u8] = if let Some(value) = unix_nanos {
        for (index, digit) in hex[2..18].iter_mut().rev().enumerate() {
            *digit = b"0123456789abcdef"[((value >> (index * 4)) & 0xf) as usize];
        }
        &hex
    } else {
        b"unavailable\r\n"
    };
    let mut line = [0; 64];
    let end = label.len() + timestamp.len();
    line[..label.len()].copy_from_slice(label);
    line[label.len()..end].copy_from_slice(timestamp);
    let _ = clv::write_console(&line[..end]);
}
