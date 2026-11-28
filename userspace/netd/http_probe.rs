use core::ops::Range;
use smoltcp::iface::{Interface, SocketHandle, SocketSet};
use smoltcp::socket::tcp;
use smoltcp::time::{Duration, Instant};
use smoltcp::wire::{IpAddress, IpEndpoint, Ipv4Address};

const PEER: IpEndpoint = IpEndpoint::new(IpAddress::Ipv4(Ipv4Address::new(192, 0, 2, 1)), 8080);
const LOCAL_PORT: u16 = 49154;
const REQUEST: &[u8] = b"GET /probe HTTP/1.1\r\nHost: 192.0.2.1:8080\r\nConnection: close\r\n\r\n";
const BODY: &[u8] = b"Hello from native HTTP\n";
const TIMEOUT: Duration = Duration::from_secs(5);
const MAX_HEAD_BYTES: usize = 1024;
const MAX_BODY_BYTES: usize = 1024;
pub(super) const RESPONSE_BYTES: usize = MAX_HEAD_BYTES + MAX_BODY_BYTES;
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
    phase: Phase,
    sent: usize,
    bytes: &'a mut [u8; RESPONSE_BYTES],
    received: usize,
    body: Option<Range<usize>>,
}

impl<'a> Probe<'a> {
    pub(super) fn new(handle: SocketHandle, bytes: &'a mut [u8; RESPONSE_BYTES]) -> Self {
        Self {
            handle,
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
        iface: &mut Interface,
        sockets: &mut SocketSet<'_>,
    ) {
        if matches!(self.phase, Phase::Done) {
            return;
        }

        let socket = sockets.get_mut::<tcp::Socket>(self.handle);
        match self.advance(now, iface, socket) {
            Ok(false) => {}
            Ok(true) => {
                self.phase = Phase::Done;
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
                    .connect(iface.context(), PEER, LOCAL_PORT)
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
                self.body = response_body(&self.bytes[..self.received])?;
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

fn response_body(bytes: &[u8]) -> Result<Option<Range<usize>>, Failure> {
    let mut headers = [httparse::EMPTY_HEADER; 32];
    let mut response = httparse::Response::new(&mut headers);
    let head = &bytes[..bytes.len().min(MAX_HEAD_BYTES)];
    let end = match response.parse(head) {
        Ok(httparse::Status::Complete(end)) => end,
        Ok(httparse::Status::Partial) if head.len() < MAX_HEAD_BYTES => return Ok(None),
        _ => return Err(Failure::Invalid),
    };
    if response.version != Some(1) || response.code != Some(200) {
        return Err(Failure::Invalid);
    }

    let mut length = None;
    for header in response.headers {
        if header.name.eq_ignore_ascii_case("Transfer-Encoding") {
            return Err(Failure::Invalid);
        }
        if header.name.eq_ignore_ascii_case("Content-Length") {
            let value = header.value.trim_ascii();
            if length.is_some() || value.is_empty() || !value.iter().all(u8::is_ascii_digit) {
                return Err(Failure::Invalid);
            }
            length = core::str::from_utf8(value)
                .ok()
                .and_then(|value| value.parse::<usize>().ok())
                .filter(|length| *length <= MAX_BODY_BYTES);
            if length.is_none() {
                return Err(Failure::Invalid);
            }
        }
    }
    let length = length.ok_or(Failure::Invalid)?;
    Ok(Some(end..end + length))
}
