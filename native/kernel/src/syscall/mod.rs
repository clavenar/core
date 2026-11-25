use crate::memory::usercopy::copy_from_user_stack;
use crate::scheduler::{Scheduler, TrapFrame};
use crate::{uart_read_byte, uart_write, write_task};

const SYS_PROBE: usize = 0;
const SYS_EXIT: usize = 1;
const SYS_WRITE_CONSOLE: usize = 2;
const SYS_TIMER_FIRED: usize = 3;
const SYS_READ_CONSOLE: usize = 4;
const SYS_OPEN: usize = 5;
const SYS_READ: usize = 6;
const SYS_READDIR: usize = 7;
const SYS_CLOSE: usize = 8;

const SYSCALL_ERROR: usize = usize::MAX;
const CONSOLE_WRITE_LIMIT: usize = 128;
const PROBE_VALUE: usize = 42;
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
            uart_write(b" user probe\r\n");
            PROBE_VALUE
        }
        SYS_EXIT => return Outcome::Exit(a0),
        SYS_WRITE_CONSOLE => write_console(a0, a1),
        SYS_TIMER_FIRED => usize::from(scheduler.timer_fired()),
        SYS_READ_CONSOLE => uart_read_byte().map_or(SYSCALL_ERROR, usize::from),
        SYS_OPEN => scheduler
            .file_handles()
            .open(a0, a1)
            .unwrap_or(SYSCALL_ERROR),
        SYS_READ => scheduler
            .file_handles()
            .read(a0, a1, a2)
            .unwrap_or(SYSCALL_ERROR),
        SYS_READDIR => scheduler
            .file_handles()
            .readdir(a0, a1, a2)
            .unwrap_or(SYSCALL_ERROR),
        SYS_CLOSE => scheduler.file_handles().close(a0).unwrap_or(SYSCALL_ERROR),
        _ => SYSCALL_ERROR,
    };
    // ECALL is a 32-bit instruction, including on cores with compressed code.
    frame.sepc += 4;
    Outcome::Resume
}

fn write_console(address: usize, length: usize) -> usize {
    let mut buffer = [0; CONSOLE_WRITE_LIMIT];
    let Some(bytes) = buffer.get_mut(..length) else {
        return SYSCALL_ERROR;
    };
    if !copy_from_user_stack(address, bytes) {
        return SYSCALL_ERROR;
    }

    uart_write(bytes);
    length
}
