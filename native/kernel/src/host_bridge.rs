use clv_abi::bridge::{BridgeFailure, HEADER_BYTES, HostRequest, MAX_WIRE_BYTES, request_id};

use crate::arch::monotonic_nanos;
use crate::drivers::virtio::console::Port;

#[derive(Clone, Copy)]
struct Exchange {
    launch: usize,
    request_id: usize,
    deadline: u64,
}

pub(crate) struct HostBridge {
    port: Port,
    exchange: Option<Exchange>,
    last_request_id: usize,
    // A partial frame and one receive buffer can coexist across goal changes.
    incoming: [u8; 2 * MAX_WIRE_BYTES],
    received: usize,
    failed: bool,
}

impl HostBridge {
    pub(crate) fn new(port: Port) -> Self {
        Self {
            port,
            exchange: None,
            last_request_id: 0,
            incoming: [0; 2 * MAX_WIRE_BYTES],
            received: 0,
            failed: false,
        }
    }

    pub(crate) fn submit(
        &mut self,
        launch: usize,
        id: usize,
        request: HostRequest,
        deadline: u64,
    ) -> Result<(), BridgeFailure> {
        if self.failed || self.exchange.is_some() || id <= self.last_request_id {
            return Err(BridgeFailure::Unavailable);
        }
        if monotonic_nanos() >= deadline {
            return Err(BridgeFailure::TimedOut);
        }
        let mut buffer = [0; MAX_WIRE_BYTES];
        let frame = request.encode(id as u64, &mut buffer);
        if !self
            .port
            .try_send(frame)
            .map_err(|_| BridgeFailure::Unavailable)?
        {
            return Err(BridgeFailure::Unavailable);
        }
        self.last_request_id = id;
        self.exchange = Some(Exchange {
            launch,
            request_id: id,
            deadline,
        });
        Ok(())
    }

    /// Advances at most one receive buffer and delivers a complete matching frame.
    pub(crate) fn poll(
        &mut self,
        launch: usize,
        copy: impl FnOnce(u64, &[u8]) -> bool,
    ) -> Result<usize, BridgeFailure> {
        let exchange = self
            .exchange
            .filter(|exchange| exchange.launch == launch)
            .ok_or(BridgeFailure::Unavailable)?;
        if monotonic_nanos() >= exchange.deadline {
            self.release(launch);
            return Err(BridgeFailure::TimedOut);
        }
        let length = match self.next_frame() {
            Ok(Some(length)) => length,
            Ok(None) => return Ok(0),
            Err(error) => {
                // A bad length loses framing on this byte stream. Keep the DMA
                // storage reserved and retire the session.
                self.failed = true;
                self.release(launch);
                return Err(error);
            }
        };
        if monotonic_nanos() >= exchange.deadline {
            self.release(launch);
            return Err(BridgeFailure::TimedOut);
        }
        let frame = &self.incoming[..length];
        let id = exchange.request_id as u64;
        if request_id(frame) != Some(id) {
            self.consume(length);
            return Ok(0);
        }
        if !copy(id, frame) {
            return Err(BridgeFailure::Unavailable);
        }
        self.consume(length);
        self.release(launch);
        Ok(length)
    }

    pub(crate) fn release(&mut self, launch: usize) {
        if self
            .exchange
            .is_some_and(|exchange| exchange.launch == launch)
        {
            self.exchange = None;
        }
    }

    pub(crate) fn is_pending(&self) -> bool {
        self.exchange.is_some()
    }

    fn next_frame(&mut self) -> Result<Option<usize>, BridgeFailure> {
        if let Some(length) = self.complete_length()? {
            return Ok(Some(length));
        }
        let mut chunk = [0; MAX_WIRE_BYTES];
        let Some(count) = self
            .port
            .try_read(&mut chunk)
            .map_err(|_| BridgeFailure::Unavailable)?
        else {
            return Ok(None);
        };
        // Reading only when the front frame is incomplete leaves room for the chunk.
        let end = self.received + count;
        self.incoming[self.received..end].copy_from_slice(&chunk[..count]);
        self.received = end;
        self.complete_length()
    }

    fn complete_length(&self) -> Result<Option<usize>, BridgeFailure> {
        if self.received == 0 {
            return Ok(None);
        }
        let length = usize::from(self.incoming[0]);
        if !(HEADER_BYTES..=MAX_WIRE_BYTES).contains(&length) {
            return Err(BridgeFailure::InvalidReply);
        }
        Ok((length <= self.received).then_some(length))
    }

    fn consume(&mut self, length: usize) {
        self.incoming.copy_within(length..self.received, 0);
        self.received -= length;
    }
}
