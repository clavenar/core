use clv::net;
use embedded_io::{ErrorKind, ErrorType, Read, Write};
use embedded_tls::blocking::{
    Aes128GcmSha256, Certificate, CertificateEntryRef, CertificateRef, CertificateVerifyRef,
    CryptoProvider, Sha256, TlsClock, TlsConfig, TlsConnection, TlsContext, TlsError, TlsVerifier,
};
use embedded_tls::pki::CertVerifier;
use rand_chacha::{ChaCha20Rng, rand_core::RngCore};
use rustls_pki_types::{CertificateDer, ServerName};
use smoltcp::iface::SocketHandle;
use smoltcp::socket::tcp;
use smoltcp::time::{Duration, Instant};
use smoltcp::wire::{IpAddress, IpEndpoint, Ipv4Address};
use webpki::EndEntityCert;

use super::Network;

const PEER: IpEndpoint = IpEndpoint::new(IpAddress::Ipv4(Ipv4Address::new(192, 0, 2, 1)), 8443);
const SERVER_NAME: &str = "probe.clavenar.test";
const TRUST_ANCHOR: &[u8] = include_bytes!(env!("CLV_TLS_CA_DER"));
const TIMEOUT: Duration = Duration::from_secs(5);
const CERTIFICATE_BYTES: usize = 1024;
const READ_RECORD_BYTES: usize = 4096;
const WRITE_RECORD_BYTES: usize = 2048;

pub(super) fn run(network: &mut Network<'_>, handle: SocketHandle, rng: Option<ChaCha20Rng>) {
    let Some(rng) = rng else {
        clv::print(b"netd TLS probe entropy unavailable\r\n");
        return;
    };
    if net::unix_time().is_none() {
        clv::print(b"netd TLS probe clock unavailable\r\n");
        return;
    }
    let result = handshake(network, handle, rng);
    if result.is_err() {
        network.sockets.get_mut::<tcp::Socket>(handle).abort();
        network.flush();
    }
    let message: &[u8] = match result {
        Ok(()) => b"netd TLS probe authenticated probe.clavenar.test\r\n",
        Err(
            TlsError::InvalidCertificate
            | TlsError::InvalidCertificateEntry
            | TlsError::InvalidSignature,
        ) => b"netd TLS probe certificate rejected\r\n",
        Err(TlsError::Io(ErrorKind::TimedOut)) => b"netd TLS probe timed out\r\n",
        Err(TlsError::Io(_) | TlsError::IoError | TlsError::ConnectionClosed) => {
            b"netd TLS probe transport failed\r\n"
        }
        Err(_) => b"netd TLS probe handshake failed\r\n",
    };
    clv::print(message);
}

fn handshake(
    network: &mut Network<'_>,
    handle: SocketHandle,
    mut rng: ChaCha20Rng,
) -> Result<(), TlsError> {
    network
        .receive(Some(Instant::ZERO))
        .ok_or(TlsError::IoError)?;
    let deadline = network.timestamp() + TIMEOUT;
    let port = 0xe000 | (rng.next_u32() as u16 & 0x1fff);
    network
        .sockets
        .get_mut::<tcp::Socket>(handle)
        .connect(network.interface.context(), PEER, port)
        .map_err(|_| TlsError::ConnectionClosed)?;

    let mut stream = TcpStream {
        network,
        handle,
        deadline,
    };
    while stream.socket().state() == tcp::State::SynSent {
        stream.wait().map_err(TlsError::Io)?;
    }
    if stream.socket().state() != tcp::State::Established {
        return Err(TlsError::ConnectionClosed);
    }

    let mut read_buffer = [0; READ_RECORD_BYTES];
    let mut write_buffer = [0; WRITE_RECORD_BYTES];
    let mut tls = TlsConnection::new(stream, &mut read_buffer, &mut write_buffer);
    let config = TlsConfig::new().with_server_name(SERVER_NAME);
    let mut provider = Provider {
        rng,
        peer: PeerVerifier {
            inner: CertVerifier::new(Certificate::X509(TRUST_ANCHOR)),
            state: Verification::Certificate,
        },
    };
    tls.open(TlsContext::new(&config, &mut provider))?;
    if provider.peer.state != Verification::Complete {
        return Err(TlsError::InvalidCertificate);
    }

    let mut stream = tls.close().map_err(|(_, error)| error)?;
    stream.socket().close();
    stream.network.flush();
    Ok(())
}

struct Clock;

impl TlsClock for Clock {
    fn now() -> Option<u64> {
        // The library treats None as a request to skip the date check. An
        // unavailable clock must instead make the certificate expire.
        Some(net::unix_time().unwrap_or(u64::MAX))
    }
}

struct Provider {
    rng: ChaCha20Rng,
    peer: PeerVerifier,
}

impl CryptoProvider for Provider {
    type CipherSuite = Aes128GcmSha256;
    type Signature = [u8; 0];

    fn rng(&mut self) -> impl embedded_tls::CryptoRngCore {
        &mut self.rng
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
