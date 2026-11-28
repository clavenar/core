//! Ethernet frame I/O for the dedicated network service.
//!
//! At boot, the service receives its MAC in the low 48 bits of a1 in
//! little-endian order and a 64-bit transport seed in a2. The kernel
//! supplies zero as the prototype seed when it cannot read the entropy device.
//! Register a3 holds the signed offset in nanoseconds from guest monotonic
//! time to Unix time, or NO_UNIX_TIME_OFFSET.
//! When bit 48 of a1 is set, a4 through a7 hold the cryptographic seed
//! as four little-endian words in order.

pub const ETHERNET_HEADER_BYTES: usize = 14;
pub const MAX_FRAME_BYTES: usize = ETHERNET_HEADER_BYTES + 1500;
pub const NO_RECEIVE_DEADLINE: u64 = 0;
pub const NO_UNIX_TIME_OFFSET: i64 = i64::MIN;
pub const CRYPTO_SEED_PRESENT: u64 = 1 << 48;
pub const CRYPTO_SEED_BYTES: usize = 32;
/// Operation for reading current Unix seconds through SYS_NET_FRAME.
pub const READ_UNIX_TIME: usize = 2;

pub const SUBMIT_HTTPS_PROBE: usize = 3;
pub const POLL_HTTPS_PROBE: usize = 4;
pub const TAKE_HTTPS_PROBE: usize = 5;
pub const COMPLETE_HTTPS_PROBE: usize = 6;
pub const HTTPS_PROBE_IPV4: [u8; 4] = [192, 0, 2, 3];
pub const HTTPS_PROBE_TIMEOUT_NS: u64 = 5_000_000_000;
pub const HTTPS_PROBE_BODY: &[u8] = b"Hello from native HTTPS\n";

#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum HttpsStatus {
    Complete = 1,
    Denied = 2,
    Unavailable = 3,
    TimedOut = 4,
    DnsFailed = 5,
    AddressDenied = 6,
    TlsFailed = 7,
    InvalidResponse = 8,
}

impl HttpsStatus {
    pub const fn from_raw(value: usize) -> Option<Self> {
        match value {
            1 => Some(Self::Complete),
            2 => Some(Self::Denied),
            3 => Some(Self::Unavailable),
            4 => Some(Self::TimedOut),
            5 => Some(Self::DnsFailed),
            6 => Some(Self::AddressDenied),
            7 => Some(Self::TlsFailed),
            8 => Some(Self::InvalidResponse),
            _ => None,
        }
    }
}

pub fn crypto_seed(packed_mac: u64, words: [u64; 4]) -> Option<[u8; CRYPTO_SEED_BYTES]> {
    if packed_mac & CRYPTO_SEED_PRESENT == 0 {
        return None;
    }
    let mut seed = [0; CRYPTO_SEED_BYTES];
    for (chunk, word) in seed.as_chunks_mut::<8>().0.iter_mut().zip(words) {
        *chunk = word.to_le_bytes();
    }
    Some(seed)
}

/// Applies the boot clock offset to a guest monotonic timestamp.
pub fn unix_nanos(monotonic_nanos: u64, offset: i64) -> Option<u64> {
    if offset == NO_UNIX_TIME_OFFSET {
        return None;
    }
    monotonic_nanos.checked_add_signed(offset)
}

#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(usize)]
pub enum FrameOperation {
    /// Pass an absolute deadline in guest monotonic nanoseconds in a3, or
    /// NO_RECEIVE_DEADLINE to wait for a frame. A successful receive returns the
    /// length in a0 (zero for timeout or retry) and current monotonic nanoseconds in a1.
    Receive = 0,
    Transmit = 1,
}

impl FrameOperation {
    pub const fn from_register(value: usize) -> Option<Self> {
        match value {
            0 => Some(Self::Receive),
            1 => Some(Self::Transmit),
            _ => None,
        }
    }
}
