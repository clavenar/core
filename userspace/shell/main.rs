#![no_std]
#![no_main]

use core::arch::asm;
use core::panic::PanicInfo;

const SYS_EXIT: usize = 1;
const SYS_WRITE_CONSOLE: usize = 2;
const SYS_TIMER_FIRED: usize = 3;
const SYS_READ_CONSOLE: usize = 4;
const SYS_OPEN: usize = 5;
const SYS_READ: usize = 6;
const SYS_READDIR: usize = 7;
const SYS_CLOSE: usize = 8;
const SYSCALL_ERROR: usize = usize::MAX;

// SAFETY: The kernel enters this unique symbol with an aligned user stack.
#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
extern "C" fn _start() -> ! {
    let mut line = [0; 64];
    let mut length = 0;
    let mut previous_was_cr = false;

    // Let both boot tasks get a turn before drawing the prompt.
    while syscall(SYS_TIMER_FIRED, 0, 0, 0) == 0 {
        core::hint::spin_loop();
    }
    write(b"clv> ");

    loop {
        let Ok(byte) = u8::try_from(syscall(SYS_READ_CONSOLE, 0, 0, 0)) else {
            core::hint::spin_loop();
            continue;
        };
        if byte == b'\n' && previous_was_cr {
            previous_was_cr = false;
            continue;
        }
        previous_was_cr = byte == b'\r';

        match byte {
            b'\r' | b'\n' => {
                write(b"\r\n");
                run_command(&line[..length]);
                length = 0;
                write(b"clv> ");
            }
            8 | 127 if length > 0 => {
                length -= 1;
                write(b"\x08 \x08");
            }
            b' '..=b'~' if length < line.len() => {
                line[length] = byte;
                length += 1;
                write(&[byte]);
            }
            _ => {}
        }
    }
}

fn run_command(line: &[u8]) {
    match line.trim_ascii() {
        b"help" => write(b"help  ls  cat <path>  exit\r\n"),
        b"ls" => show(&[b'/'], SYS_READDIR),
        b"cat" => write(b"usage: cat <path>\r\n"),
        b"exit" => exit(0),
        b"" => {}
        command => {
            if let Some(path) = command.strip_prefix(b"cat ") {
                show(path.trim_ascii(), SYS_READ);
            } else {
                write(b"unknown command\r\n");
            }
        }
    }
}

fn show(path: &[u8], operation: usize) {
    let mut buffer = [0; 64];
    buffer[..path.len()].copy_from_slice(path);
    let handle = syscall(
        SYS_OPEN,
        buffer.as_mut_ptr().expose_provenance(),
        path.len(),
        0,
    );
    if handle == SYSCALL_ERROR {
        write(b"cannot open path\r\n");
        return;
    }

    loop {
        let count = syscall(
            operation,
            handle,
            buffer.as_mut_ptr().expose_provenance(),
            buffer.len(),
        );
        if count == 0 {
            break;
        }
        let Some(bytes) = buffer.get(..count) else {
            write(b"cannot read path\r\n");
            break;
        };
        write_stack(bytes);
        if operation == SYS_READDIR {
            write(b"\r\n");
        }
    }
    syscall(SYS_CLOSE, handle, 0, 0);
}

fn write(text: &[u8]) {
    // Console writes take their source bytes from the user stack.
    let mut buffer = [0; 32];
    for chunk in text.chunks(buffer.len()) {
        buffer[..chunk.len()].copy_from_slice(chunk);
        write_stack(&buffer[..chunk.len()]);
    }
}

fn write_stack(text: &[u8]) {
    syscall(
        SYS_WRITE_CONSOLE,
        text.as_ptr().expose_provenance(),
        text.len(),
        0,
    );
}

fn exit(status: usize) -> ! {
    syscall(SYS_EXIT, status, 0, 0);
    loop {
        core::hint::spin_loop();
    }
}

#[inline(never)]
fn syscall(number: usize, mut a0: usize, a1: usize, a2: usize) -> usize {
    // SAFETY: Clavenar preserves registers other than the result in a0.
    unsafe {
        asm!(
            "ecall",
            inlateout("a0") a0,
            in("a1") a1,
            in("a2") a2,
            in("a7") number,
            options(nostack),
        );
    }
    a0
}

#[panic_handler]
fn panic(_info: &PanicInfo<'_>) -> ! {
    exit(1)
}
