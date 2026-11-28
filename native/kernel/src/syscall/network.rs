use clv_abi::SYSCALL_ERROR;
use clv_abi::net::{
    COMPLETE_HTTPS_PROBE, ETHERNET_HEADER_BYTES, FrameOperation, HTTPS_PROBE_IPV4, HttpsStatus,
    MAX_FRAME_BYTES, NO_RECEIVE_DEADLINE, POLL_HTTPS_PROBE, READ_UNIX_TIME, SUBMIT_HTTPS_PROBE,
    TAKE_HTTPS_PROBE,
};

use super::{REG_A0, REG_A1, REG_A2, REG_A3, REG_A4};
use crate::arch::monotonic_nanos;
use crate::arch::riscv64::trap::TrapFrame;
use crate::drivers::virtio::net::{self, BUFFER_BYTES, NetError, Nic};
use crate::memory::usercopy::UserStack;
use crate::scheduler::{Scheduler, WaitResult};

pub(super) fn dispatch(
    scheduler: &mut Scheduler,
    interface: &mut Option<Nic>,
    frame: &mut TrapFrame,
) -> WaitResult<usize> {
    let operation = frame.registers[REG_A0];
    let argument = frame.registers[REG_A1];
    match operation {
        SUBMIT_HTTPS_PROBE => {
            let result = scheduler.submit_https(interface.is_some());
            return WaitResult::Ready(result.unwrap_or_else(|status| {
                frame.registers[REG_A1] = status as usize;
                SYSCALL_ERROR
            }));
        }
        POLL_HTTPS_PROBE => {
            return scheduler
                .poll_https(argument)
                .map_or(WaitResult::Unavailable, WaitResult::Ready);
        }
        _ => {}
    }
    if !scheduler.is_netd() {
        return WaitResult::Unavailable;
    }
    match operation {
        READ_UNIX_TIME => {
            return scheduler
                .netd_unix_time()
                .map_or(WaitResult::Unavailable, WaitResult::Ready);
        }
        TAKE_HTTPS_PROBE => {
            let Some((request, deadline)) = scheduler.take_https() else {
                return WaitResult::Ready(0);
            };
            frame.registers[REG_A1] = u32::from_le_bytes(HTTPS_PROBE_IPV4) as usize;
            frame.registers[REG_A2] = deadline as usize;
            return WaitResult::Ready(request);
        }
        COMPLETE_HTTPS_PROBE => {
            return HttpsStatus::from_raw(frame.registers[REG_A2])
                .and_then(|status| scheduler.complete_https(argument, status))
                .map_or(WaitResult::Unavailable, |()| WaitResult::Ready(0));
        }
        _ => {}
    }
    let Some(operation) = FrameOperation::from_register(operation) else {
        return WaitResult::Unavailable;
    };
    let address = frame.registers[REG_A1];
    let length = frame.registers[REG_A2];
    let valid_length = match operation {
        FrameOperation::Receive => length == MAX_FRAME_BYTES,
        FrameOperation::Transmit => (ETHERNET_HEADER_BYTES..=MAX_FRAME_BYTES).contains(&length),
    };
    let stack = scheduler.address_space().stack;
    if !valid_length || !stack.contains(address, length) {
        return WaitResult::Unavailable;
    }
    let Some(nic) = interface.as_mut() else {
        return WaitResult::Unavailable;
    };
    let requested = frame.registers[REG_A3] as u64;
    let requested = (operation == FrameOperation::Receive && requested != NO_RECEIVE_DEADLINE)
        .then_some(requested);
    let request = frame.registers[REG_A4];
    let scoped = if request == 0 {
        None
    } else {
        let Some(deadline) = scheduler.https_deadline(request) else {
            return WaitResult::Unavailable;
        };
        Some(deadline)
    };
    let deadline = requested.into_iter().chain(scoped).min();
    let expired = scoped.is_some_and(|at| monotonic_nanos() >= at);
    let result = match operation {
        FrameOperation::Receive if expired || (request == 0 && scheduler.https_queued()) => {
            Ok(Some(0))
        }
        FrameOperation::Transmit if expired => return WaitResult::Unavailable,
        FrameOperation::Receive => receive(nic, stack, address),
        FrameOperation::Transmit => transmit(nic, stack, address, length),
    };
    let result = match result {
        Ok(Some(length)) => WaitResult::Ready(length),
        Ok(None) if deadline.is_some_and(|at| monotonic_nanos() >= at) => WaitResult::Ready(0),
        Ok(None) => scheduler.wait_for_network(frame, operation, deadline),
        Err(error) => {
            net::retire(interface, error);
            WaitResult::Unavailable
        }
    };
    if let WaitResult::Ready(length) = result
        && operation == FrameOperation::Receive
        && length != SYSCALL_ERROR
    {
        // A parked syscall retries with its original buffer pointer in a1.
        frame.registers[REG_A1] = monotonic_nanos() as usize;
    }
    result
}

fn receive(nic: &mut Nic, stack: UserStack, address: usize) -> Result<Option<usize>, NetError> {
    let mut packet = [0; BUFFER_BYTES];
    Ok(nic.receive_frame(&mut packet)?.map(|frame| {
        if stack.copy_to_user(frame, address, MAX_FRAME_BYTES) {
            frame.len()
        } else {
            SYSCALL_ERROR
        }
    }))
}

fn transmit(
    nic: &mut Nic,
    stack: UserStack,
    address: usize,
    length: usize,
) -> Result<Option<usize>, NetError> {
    if !nic.transmit_ready()? {
        return Ok(None);
    }
    let mut bytes = [0; MAX_FRAME_BYTES];
    let frame = &mut bytes[..length];
    if !stack.copy_from_user(address, frame) {
        return Ok(Some(SYSCALL_ERROR));
    }
    nic.send_frame(frame)?;
    Ok(Some(length))
}
