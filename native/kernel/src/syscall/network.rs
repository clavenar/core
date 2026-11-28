use clv_abi::SYSCALL_ERROR;
use clv_abi::net::{ETHERNET_HEADER_BYTES, FrameOperation, MAX_FRAME_BYTES, NO_RECEIVE_DEADLINE};

use super::{REG_A0, REG_A1, REG_A2, REG_A3};
use crate::arch::monotonic_nanos;
use crate::arch::riscv64::trap::TrapFrame;
use crate::drivers::virtio::net::{self, BUFFER_BYTES, NetError, Nic};
use crate::memory::usercopy::UserStack;
use crate::scheduler::{Scheduler, WaitResult};

pub(super) fn frame_io(
    scheduler: &mut Scheduler,
    interface: &mut Option<Nic>,
    frame: &mut TrapFrame,
) -> WaitResult<usize> {
    if !scheduler.is_netd() {
        return WaitResult::Unavailable;
    }
    let Some(operation) = FrameOperation::from_register(frame.registers[REG_A0]) else {
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
    let deadline = frame.registers[REG_A3] as u64;
    let deadline = (operation == FrameOperation::Receive && deadline != NO_RECEIVE_DEADLINE)
        .then_some(deadline);
    let result = match operation {
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
