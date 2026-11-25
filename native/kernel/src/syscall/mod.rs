use clv_abi::{
    OPEN_READ, OPEN_WRITE_TRUNCATE, PROBE_VALUE, SYS_CLOSE, SYS_EXIT, SYS_MKDIR, SYS_OPEN,
    SYS_PROBE, SYS_READ, SYS_READ_CONSOLE, SYS_READDIR, SYS_TIMER_FIRED, SYS_UNLINK, SYS_WRITE,
    SYS_WRITE_CONSOLE, SYSCALL_ERROR,
};

use crate::arch::riscv64::trap::TrapFrame;
use crate::console::write_task;
use crate::drivers::uart;
use crate::fs::ramfs::{FileHandles, RamFs, WRITABLE_FILE_CAPACITY};
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

pub(crate) fn dispatch(
    scheduler: &mut Scheduler,
    fs: &mut RamFs,
    frame: &mut TrapFrame,
) -> Outcome {
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
        SYS_OPEN => open_file(scheduler.file_handles(), fs, a0, a1, a2).unwrap_or(SYSCALL_ERROR),
        SYS_READ => scheduler
            .file_handles()
            .read(fs, a0, a2, |bytes| copy_to_user_stack(bytes, a1, a2))
            .unwrap_or(SYSCALL_ERROR),
        SYS_READDIR => scheduler
            .file_handles()
            .readdir(fs, a0, |name| copy_to_user_stack(name, a1, a2))
            .unwrap_or(SYSCALL_ERROR),
        SYS_CLOSE => scheduler
            .file_handles()
            .close(fs, a0)
            .unwrap_or(SYSCALL_ERROR),
        SYS_WRITE => write_file(scheduler.file_handles(), fs, a0, a1, a2).unwrap_or(SYSCALL_ERROR),
        SYS_UNLINK => with_user_path(a0, a1, |path| fs.unlink(path)).unwrap_or(SYSCALL_ERROR),
        SYS_MKDIR => with_user_path(a0, a1, |path| fs.mkdir(path)).unwrap_or(SYSCALL_ERROR),
        _ => SYSCALL_ERROR,
    };
    // ECALL is a 32-bit instruction, including on cores with compressed code.
    frame.sepc += 4;
    Outcome::Resume
}

fn open_file(
    files: &mut FileHandles,
    fs: &mut RamFs,
    address: usize,
    length: usize,
    mode: usize,
) -> Option<usize> {
    let writable = match mode {
        OPEN_READ => false,
        OPEN_WRITE_TRUNCATE => true,
        _ => return None,
    };
    with_user_path(address, length, |path| files.open(fs, path, writable))
}

fn with_user_path(
    address: usize,
    length: usize,
    operation: impl FnOnce(&[u8]) -> Option<usize>,
) -> Option<usize> {
    let mut buffer = [0; PATH_LIMIT];
    let path = buffer.get_mut(..length)?;
    if !copy_from_user_stack(address, path) {
        return None;
    }
    operation(path)
}

fn write_file(
    files: &mut FileHandles,
    fs: &mut RamFs,
    fd: usize,
    address: usize,
    length: usize,
) -> Option<usize> {
    let mut buffer = [0; WRITABLE_FILE_CAPACITY];
    let bytes = buffer.get_mut(..length)?;
    if !copy_from_user_stack(address, bytes) {
        return None;
    }
    files.write(fs, fd, bytes)
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
