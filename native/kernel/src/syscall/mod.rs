use clv_abi::{
    PROBE_VALUE, SYS_CLOSE, SYS_EXIT, SYS_OPEN, SYS_PROBE, SYS_READ, SYS_READ_CONSOLE, SYS_READDIR,
    SYS_TIMER_FIRED, SYS_WRITE_CONSOLE, SYSCALL_ERROR,
};

use crate::arch::riscv64::trap::TrapFrame;
use crate::console::write_task;
use crate::drivers::uart;
use crate::fs::ramfs::FileHandles;
use crate::memory::usercopy::{copy_from_user_stack, copy_to_user_stack};
use crate::scheduler::Scheduler;

const CONSOLE_WRITE_LIMIT: usize = 128;
const PATH_LIMIT: usize = 64;
const REG_A0: usize = 10;
const REG_A1: usize = 11;
const REG_A2: usize = 12;
const REG_A7: usize = 17;

pub(crate) enum Outcome {
    Resume,
    Exit(usize),
}

pub(crate) fn dispatch(scheduler: &mut Scheduler, frame: &mut TrapFrame) -> Outcome {
    let a0 = frame.registers[REG_A0];
    let a1 = frame.registers[REG_A1];
    let a2 = frame.registers[REG_A2];
    frame.registers[REG_A0] = match frame.registers[REG_A7] {
        SYS_PROBE => {
            write_task(scheduler.current_id());
            uart::write(b" user probe\r\n");
            PROBE_VALUE
        }
        SYS_EXIT => return Outcome::Exit(a0),
        SYS_WRITE_CONSOLE => write_console(a0, a1),
        SYS_TIMER_FIRED => usize::from(scheduler.timer_fired()),
        SYS_READ_CONSOLE => uart::read_byte().map_or(SYSCALL_ERROR, usize::from),
        SYS_OPEN => open_file(scheduler.file_handles(), a0, a1).unwrap_or(SYSCALL_ERROR),
        SYS_READ => scheduler
            .file_handles()
            .read(a0, a2, |bytes| copy_to_user_stack(bytes, a1, a2))
            .unwrap_or(SYSCALL_ERROR),
        SYS_READDIR => scheduler
            .file_handles()
            .readdir(a0, |name| copy_to_user_stack(name, a1, a2))
            .unwrap_or(SYSCALL_ERROR),
        SYS_CLOSE => scheduler.file_handles().close(a0).unwrap_or(SYSCALL_ERROR),
        _ => SYSCALL_ERROR,
    };
    // ECALL is a 32-bit instruction, including on cores with compressed code.
    frame.sepc += 4;
    Outcome::Resume
}

fn open_file(files: &mut FileHandles, address: usize, length: usize) -> Option<usize> {
    let mut buffer = [0; PATH_LIMIT];
    let path = buffer.get_mut(..length)?;
    if !copy_from_user_stack(address, path) {
        return None;
    }
    files.open(path)
}

fn write_console(address: usize, length: usize) -> usize {
    let mut buffer = [0; CONSOLE_WRITE_LIMIT];
    let Some(bytes) = buffer.get_mut(..length) else {
        return SYSCALL_ERROR;
    };
    if !copy_from_user_stack(address, bytes) {
        return SYSCALL_ERROR;
    }

    uart::write(bytes);
    length
}
