use clv::mcp::{self, FRAME_BYTES, Frame};
use clv::net::{self, HTTPS_PROBE_BODY, HttpsRequest, HttpsStatus};
use core::fmt::Write as _;
use core::ops::Range;
use embedded_io::{ErrorKind, ErrorType, Read, Write};
use embedded_tls::blocking::{
    Aes128GcmSha256, Certificate, CertificateEntryRef, CertificateRef, CertificateVerifyRef,
    CryptoProvider, Sha256, TlsClock, TlsConfig, TlsConnection, TlsContext, TlsError, TlsVerifier,
};
use embedded_tls::pki::CertVerifier;
use rand_chacha::{ChaCha20Rng, rand_core::RngCore};
use rustls_pki_types::{CertificateDer, ServerName};
use smoltcp::iface::SocketHandle;
use smoltcp::socket::{dns, tcp};
use smoltcp::time::Instant;
use smoltcp::wire::{
    DnsQueryType, EthernetFrame, EthernetProtocol, IpAddress, IpEndpoint, IpProtocol, Ipv4Address,
    Ipv4Packet, UdpPacket,
};
use webpki::EndEntityCert;

use super::http_response::{ExpectedResponse, RESPONSE_BYTES, body_range};
use super::{GUEST_ADDRESS, Network, UDP_ECHO_ENDPOINT};

pub(super) const RESOLVER: IpAddress = IpAddress::Ipv4(Ipv4Address::new(192, 0, 2, 1));
const HTTPS_PORT: u16 = 8443;
const SERVER_NAME: &str = "probe.clavenar.test";
const PROBE_REQUEST: &[u8] =
    b"GET /probe HTTP/1.1\r\nHost: probe.clavenar.test:8443\r\nConnection: close\r\n\r\n";
const TRUST_ANCHOR: &[u8] = include_bytes!(env!("CLV_TLS_CA_DER"));
const CERTIFICATE_BYTES: usize = 1024;
const READ_RECORD_BYTES: usize = 4096;
const WRITE_RECORD_BYTES: usize = 2048;

enum Failure {
    DnsLookup,
    DnsTimeout,
    DnsAddress,
    Tls(TlsError),
    InvalidResponse,
    TruncatedResponse,
    UnexpectedBody,
}

impl From<TlsError> for Failure {
    fn from(error: TlsError) -> Self {
        Self::Tls(error)
    }
}

pub(super) fn run(
    network: &mut Network<'_>,
    handle: SocketHandle,
    dns_handle: SocketHandle,
    probe: net::HttpsProbe,
    rng: Option<&mut ChaCha20Rng>,
) {
    network.https_request = probe.request;
    let (status, message): (HttpsStatus, &[u8]) = match rng {
        None => (
            HttpsStatus::Unavailable,
            b"netd TLS probe entropy unavailable\r\n",
        ),
        Some(_) if net::unix_time().is_none() => (
            HttpsStatus::Unavailable,
            b"netd TLS probe clock unavailable\r\n",
        ),
        Some(rng) => report(
            exchange(network, handle, dns_handle, &probe, rng),
            probe.kind,
        ),
    };
    network.https_request = 0;
    if net::complete_https_probe(probe.request, status) {
        clv::print(message);
    } else {
        clv::print(b"netd HTTPS probe cancelled\r\n");
    }
}

fn report(result: Result<(), Failure>, kind: HttpsRequest) -> (HttpsStatus, &'static [u8]) {
    match result {
        Ok(()) if kind == HttpsRequest::McpInit => {
            (HttpsStatus::Complete, b"netd MCP initialized\r\n")
        }
        Ok(()) => (
            HttpsStatus::Complete,
            b"netd HTTPS probe received Hello from native HTTPS\r\n",
        ),
        Err(Failure::DnsLookup) => (HttpsStatus::DnsFailed, b"netd DNS lookup failed\r\n"),
        Err(Failure::DnsTimeout) => (HttpsStatus::TimedOut, b"netd DNS lookup timed out\r\n"),
        Err(Failure::DnsAddress) => (HttpsStatus::AddressDenied, b"netd DNS address rejected\r\n"),
        Err(Failure::Tls(
            TlsError::InvalidCertificate
            | TlsError::InvalidCertificateEntry
            | TlsError::InvalidSignature,
        )) => (
            HttpsStatus::TlsFailed,
            b"netd TLS probe certificate rejected\r\n",
        ),
        Err(Failure::Tls(TlsError::Io(ErrorKind::TimedOut))) => {
            (HttpsStatus::TimedOut, b"netd HTTPS probe timed out\r\n")
        }
        Err(Failure::Tls(TlsError::Io(_) | TlsError::IoError | TlsError::ConnectionClosed)) => (
            HttpsStatus::TlsFailed,
            b"netd TLS probe transport failed\r\n",
        ),
        Err(Failure::Tls(_)) => (HttpsStatus::TlsFailed, b"netd TLS probe failed\r\n"),
        Err(Failure::InvalidResponse) => (
            HttpsStatus::InvalidResponse,
            b"netd HTTPS probe invalid response\r\n",
        ),
        Err(Failure::TruncatedResponse) => (
            HttpsStatus::InvalidResponse,
            b"netd HTTPS probe truncated response\r\n",
        ),
        Err(Failure::UnexpectedBody) => (
            HttpsStatus::InvalidResponse,
            b"netd HTTPS probe unexpected body\r\n",
        ),
    }
}

fn exchange(
    network: &mut Network<'_>,
    handle: SocketHandle,
    dns_handle: SocketHandle,
    probe: &net::HttpsProbe,
    rng: &mut ChaCha20Rng,
) -> Result<(), Failure> {
    network
        .receive(Some(Instant::ZERO))
        .ok_or(TlsError::IoError)?;
    let deadline = Instant::from_micros((probe.deadline / 1_000) as i64);
    let allowed = IpAddress::Ipv4(Ipv4Address::from(probe.ipv4));
    network.resolving_dns = true;
    let peer = resolve_peer(network, dns_handle, allowed, deadline);
    network.resolving_dns = false;
    let peer = peer?;
    clv::print(b"netd DNS resolved probe.clavenar.test\r\n");

    let mut https = HttpsConnection {
        network,
        handle,
        peer,
        deadline,
        rng,
        last_port: 0,
    };
    let mut response = [0; RESPONSE_BYTES];
    match probe.kind {
        HttpsRequest::Probe => https
            .request(PROBE_REQUEST, b"", ExpectedResponse::Ok, &mut response)
            .map(|_| ()),
        HttpsRequest::McpInit => mcp_initialize(&mut https, &mut response),
    }
}

fn mcp_initialize(
    https: &mut HttpsConnection<'_, '_>,
    response: &mut [u8; RESPONSE_BYTES],
) -> Result<(), Failure> {
    let mut buffer = [0; FRAME_BYTES];
    let mut initialize = Frame::new(&mut buffer);
    write!(
        initialize,
        concat!(
            r#"{{"jsonrpc":"2.0","id":1,"method":"initialize","params":{{"protocolVersion":"{}","#,
            r#""capabilities":{{}},"clientInfo":{{"name":"clavenar-netd","version":"1"}}}}}}"#
        ),
        mcp::VERSION
    )
    .map_err(|_| Failure::InvalidResponse)?;
    let result = https.post(initialize.into_bytes(), ExpectedResponse::McpJson, response)?;
    if !mcp::result(result, "1").is_some_and(mcp::accepts_initialize) {
        return Err(Failure::InvalidResponse);
    }
    https.post(
        br#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        ExpectedResponse::McpAccepted,
        response,
    )?;
    Ok(())
}

struct HttpsConnection<'a, 's> {
    network: &'a mut Network<'s>,
    handle: SocketHandle,
    peer: IpEndpoint,
    deadline: Instant,
    rng: &'a mut ChaCha20Rng,
    last_port: u16,
}

impl HttpsConnection<'_, '_> {
    fn post<'a>(
        &mut self,
        body: &[u8],
        expected: ExpectedResponse,
        response: &'a mut [u8; RESPONSE_BYTES],
    ) -> Result<&'a [u8], Failure> {
        let mut buffer = [0; FRAME_BYTES];
        let mut header = Frame::new(&mut buffer);
        let mut write_header = || {
            header.write_str(concat!(
                "POST /mcp HTTP/1.1\r\nHost: probe.clavenar.test:8443\r\n",
                "Content-Type: application/json\r\nAccept: application/json, text/event-stream\r\n",
                "Connection: close\r\n"
            ))?;
            if expected == ExpectedResponse::McpAccepted {
                write!(header, "MCP-Protocol-Version: {}\r\n", mcp::VERSION)?;
            }
            write!(header, "Content-Length: {}\r\n\r\n", body.len())
        };
        write_header().map_err(|_| Failure::InvalidResponse)?;
        self.request(header.into_bytes(), body, expected, response)
    }

    fn request<'a>(
        &mut self,
        header: &[u8],
        body: &[u8],
        expected: ExpectedResponse,
        response: &'a mut [u8; RESPONSE_BYTES],
    ) -> Result<&'a [u8], Failure> {
        let range = self.round_trip(header, body, expected, response);
        let socket = self.network.sockets.get_mut::<tcp::Socket>(self.handle);
        // Discard the old tuple and queued traffic before reusing the buffers.
        socket.abort();
        socket.listen(HTTPS_PORT).unwrap();
        socket.close();
        Ok(&response[range?])
    }

    fn round_trip(
        &mut self,
        header: &[u8],
        body: &[u8],
        expected: ExpectedResponse,
        response: &mut [u8; RESPONSE_BYTES],
    ) -> Result<Range<usize>, Failure> {
        let port = loop {
            let port = 0xe000 | (self.rng.next_u32() as u16 & 0x1fff);
            if port != self.last_port {
                break port;
            }
        };
        self.last_port = port;
        self.network
            .sockets
            .get_mut::<tcp::Socket>(self.handle)
            .connect(self.network.interface.context(), self.peer, port)
            .map_err(|_| TlsError::ConnectionClosed)?;

        let mut stream = TcpStream {
            network: self.network,
            handle: self.handle,
            deadline: self.deadline,
        };
        while stream.socket().state() == tcp::State::SynSent {
            stream.wait().map_err(TlsError::Io)?;
        }
        if stream.socket().state() != tcp::State::Established {
            return Err(TlsError::ConnectionClosed.into());
        }

        let mut read_buffer = [0; READ_RECORD_BYTES];
        let mut write_buffer = [0; WRITE_RECORD_BYTES];
        let mut tls = TlsConnection::new(stream, &mut read_buffer, &mut write_buffer);
        let config = TlsConfig::new().with_server_name(SERVER_NAME);
        let mut provider = Provider {
            rng: self.rng,
            peer: PeerVerifier {
                inner: CertVerifier::new(Certificate::X509(TRUST_ANCHOR)),
                state: Verification::Certificate,
            },
        };
        tls.open(TlsContext::new(&config, &mut provider))?;
        if provider.peer.state != Verification::Complete {
            return Err(TlsError::InvalidCertificate.into());
        }
        clv::print(b"netd TLS probe authenticated probe.clavenar.test\r\n");
        tls.write_all(header)?;
        tls.write_all(body)?;
        tls.flush()?;
        let range = read_response(&mut tls, expected, response)?;
        if expected == ExpectedResponse::Ok && &response[range.clone()] != HTTPS_PROBE_BODY {
            return Err(Failure::UnexpectedBody);
        }

        let mut stream = tls.close().map_err(|(_, error)| error)?;
        stream.socket().close();
        while !matches!(
            stream.socket().state(),
            tcp::State::Closed | tcp::State::TimeWait
        ) {
            stream.wait().map_err(TlsError::Io)?;
        }
        Ok(range)
    }
}

fn resolve_peer(
    network: &mut Network<'_>,
    handle: SocketHandle,
    allowed: IpAddress,
    deadline: Instant,
) -> Result<IpEndpoint, Failure> {
    let query = network
        .sockets
        .get_mut::<dns::Socket>(handle)
        .start_query(network.interface.context(), SERVER_NAME, DnsQueryType::A)
        .map_err(|_| Failure::DnsLookup)?;
    let failure = loop {
        network.flush();
        match network
            .sockets
            .get_mut::<dns::Socket>(handle)
            .get_query_result(query)
        {
            Ok(addresses) => {
                return addresses
                    .contains(&allowed)
                    .then_some(IpEndpoint::new(allowed, HTTPS_PORT))
                    .ok_or(Failure::DnsAddress);
            }
            Err(dns::GetQueryResultError::Pending) => {}
            Err(dns::GetQueryResultError::Failed) => return Err(Failure::DnsLookup),
        }
        if network.receive(Some(deadline)).is_none() {
            break Failure::DnsLookup;
        }
        if network.timestamp() >= deadline {
            break Failure::DnsTimeout;
        }
    };
    network
        .sockets
        .get_mut::<dns::Socket>(handle)
        .cancel_query(query);
    Err(failure)
}

pub(super) fn reply_uses_echo_port(frame: &[u8]) -> bool {
    let Ok(ethernet) = EthernetFrame::new_checked(frame) else {
        return false;
    };
    if ethernet.ethertype() != EthernetProtocol::Ipv4 {
        return false;
    }
    let Ok(ip) = Ipv4Packet::new_checked(ethernet.payload()) else {
        return false;
    };
    ip.next_header() == IpProtocol::Udp
        && IpAddress::Ipv4(ip.src_addr()) == RESOLVER
        && IpAddress::Ipv4(ip.dst_addr()) == GUEST_ADDRESS
        && UdpPacket::new_checked(ip.payload())
            .is_ok_and(|udp| udp.src_port() == 53 && udp.dst_port() == UDP_ECHO_ENDPOINT.port)
}

fn read_response(
    tls: &mut TlsConnection<'_, TcpStream<'_, '_>, Aes128GcmSha256>,
    expected: ExpectedResponse,
    bytes: &mut [u8; RESPONSE_BYTES],
) -> Result<Range<usize>, Failure> {
    let mut received = 0;
    let mut body = None;
    loop {
        let mut plaintext = match tls.read_buffered() {
            Ok(plaintext) => plaintext,
            // The TLS reader maps close_notify to ConnectionClosed and TCP EOF to IoError.
            Err(TlsError::ConnectionClosed | TlsError::IoError) => {
                return Err(Failure::TruncatedResponse);
            }
            Err(error) => return Err(error.into()),
        };
        let chunk = plaintext.pop_all();
        let end = received + chunk.len();
        bytes
            .get_mut(received..end)
            .ok_or(Failure::InvalidResponse)?
            .copy_from_slice(chunk);
        received = end;
        if body.is_none() {
            body =
                body_range(&bytes[..received], expected).map_err(|_| Failure::InvalidResponse)?;
        }
        if let Some(range) = &body
            && received >= range.end
        {
            if received != range.end {
                return Err(Failure::InvalidResponse);
            }
            return Ok(range.clone());
        }
    }
}

struct Clock;

impl TlsClock for Clock {
    fn now() -> Option<u64> {
        // The library treats None as a request to skip the date check. An
        // unavailable clock must instead make the certificate expire.
        Some(net::unix_time().unwrap_or(u64::MAX))
    }
}

struct Provider<'a> {
    rng: &'a mut ChaCha20Rng,
    peer: PeerVerifier,
}

impl CryptoProvider for Provider<'_> {
    type CipherSuite = Aes128GcmSha256;
    type Signature = [u8; 0];

    fn rng(&mut self) -> impl embedded_tls::CryptoRngCore {
        &mut *self.rng
    }

    fn verifier(&mut self) -> Result<&mut impl TlsVerifier<Self::CipherSuite>, TlsError> {
        Ok(&mut self.peer)
    }
}

#[derive(PartialEq, Eq)]
enum Verification {
    Certificate,
    Signature,
    Complete,
}

struct PeerVerifier {
    inner: CertVerifier<'static, Aes128GcmSha256, Clock, CERTIFICATE_BYTES>,
    state: Verification,
}

impl TlsVerifier<Aes128GcmSha256> for PeerVerifier {
    fn set_hostname_verification(&mut self, hostname: &str) -> Result<(), TlsError> {
        self.inner.set_hostname_verification(hostname)
    }

    fn verify_certificate(
        &mut self,
        transcript: &Sha256,
        certificate: CertificateRef<'_>,
    ) -> Result<(), TlsError> {
        // The fixture uses one leaf signed directly by the configured CA.
        if self.state != Verification::Certificate {
            return Err(TlsError::InvalidCertificate);
        }
        let [CertificateEntryRef::X509(leaf)] = certificate.entries.as_slice() else {
            return Err(TlsError::InvalidCertificate);
        };
        // Require a matching DNS SAN because rustpki also accepts a matching common name.
        let leaf = CertificateDer::from(*leaf);
        let name = ServerName::try_from(SERVER_NAME).map_err(|_| TlsError::InvalidCertificate)?;
        EndEntityCert::try_from(&leaf)
            .and_then(|leaf| leaf.verify_is_valid_for_subject_name(&name))
            .map_err(|_| TlsError::InvalidCertificate)?;
        self.inner.verify_certificate(transcript, certificate)?;
        self.state = Verification::Signature;
        Ok(())
    }

    fn verify_signature(&mut self, signature: CertificateVerifyRef<'_>) -> Result<(), TlsError> {
        if self.state != Verification::Signature {
            return Err(TlsError::InvalidCertificate);
        }
        self.inner.verify_signature(signature)?;
        self.state = Verification::Complete;
        Ok(())
    }
}

struct TcpStream<'a, 's> {
    network: &'a mut Network<'s>,
    handle: SocketHandle,
    deadline: Instant,
}

impl<'s> TcpStream<'_, 's> {
    fn socket(&mut self) -> &mut tcp::Socket<'s> {
        self.network.sockets.get_mut::<tcp::Socket>(self.handle)
    }

    fn wait(&mut self) -> Result<(), ErrorKind> {
        self.network.flush();
        self.network
            .receive(Some(self.deadline))
            .ok_or(ErrorKind::Other)?;
        if self.network.timestamp() >= self.deadline {
            return Err(ErrorKind::TimedOut);
        }
        self.network.flush();
        Ok(())
    }
}

impl ErrorType for TcpStream<'_, '_> {
    type Error = ErrorKind;
}

impl Read for TcpStream<'_, '_> {
    fn read(&mut self, bytes: &mut [u8]) -> Result<usize, Self::Error> {
        if bytes.is_empty() {
            return Ok(0);
        }
        loop {
            let socket = self.socket();
            if socket.can_recv() {
                return socket
                    .recv_slice(bytes)
                    .map_err(|_| ErrorKind::ConnectionAborted);
            }
            if !socket.may_recv() {
                return Ok(0);
            }
            self.wait()?;
        }
    }
}

impl Write for TcpStream<'_, '_> {
    fn write(&mut self, bytes: &[u8]) -> Result<usize, Self::Error> {
        if bytes.is_empty() {
            return Ok(0);
        }
        loop {
            let socket = self.socket();
            if socket.can_send() {
                let count = socket
                    .send_slice(bytes)
                    .map_err(|_| ErrorKind::ConnectionAborted)?;
                self.network.flush();
                return Ok(count);
            }
            if !socket.may_send() {
                return Err(ErrorKind::ConnectionAborted);
            }
            self.wait()?;
        }
    }

    fn flush(&mut self) -> Result<(), Self::Error> {
        while self.socket().send_queue() != 0 {
            if self.socket().state() == tcp::State::Closed {
                return Err(ErrorKind::ConnectionAborted);
            }
            self.wait()?;
        }
        Ok(())
    }
}
