#![no_std]
#![no_main]

use clv_abi::{
    OPEN_READ, OPEN_WRITE_TRUNCATE, SYS_CLOSE, SYS_EXIT, SYS_OPEN, SYS_READ, SYS_READ_CONSOLE,
    SYS_READDIR, SYS_TIMER_FIRED, SYS_WRITE, SYS_WRITE_CONSOLE, SYSCALL_ERROR,
};
use core::arch::asm;
use core::panic::PanicInfo;

const LINE_LIMIT: usize = 64;
const WRITE_USAGE: &[u8] = b"usage: write <path> <text>\r\n";

// SAFETY: The kernel enters this unique symbol with an aligned user stack.
#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
extern "C" fn _start() -> ! {
    let mut line = [0; LINE_LIMIT];
    let mut length = 0;
    let mut overflow = false;
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
                if overflow {
                    write(b"line too long\r\n");
                } else {
                    run_command(&line[..length]);
                }
                length = 0;
                overflow = false;
                write(b"clv> ");
            }
            8 | 127 if length > 0 && !overflow => {
                length -= 1;
                write(b"\x08 \x08");
            }
            b' '..=b'~' if !overflow => {
                if let Some(slot) = line.get_mut(length) {
                    *slot = byte;
                    length += 1;
                    write(&[byte]);
                } else {
                    overflow = true;
                }
            }
            _ => {}
        }
    }
}

fn run_command(line: &[u8]) {
    match line.trim_ascii() {
        b"help" => write(b"help  ls  cat <path>  write <path> <text>  exit\r\n"),
        b"ls" => show(&[b'/'], SYS_READDIR),
        b"cat" => write(b"usage: cat <path>\r\n"),
        b"write" => write(WRITE_USAGE),
        b"exit" => exit(0),
        b"" => {}
        command => {
            if let Some(path) = command.strip_prefix(b"cat ") {
                show(path.trim_ascii(), SYS_READ);
            } else if let Some(arguments) = command.strip_prefix(b"write ") {
                write_file(arguments.trim_ascii_start());
            } else {
                write(b"unknown command\r\n");
            }
        }
    }
}

fn show(path: &[u8], operation: usize) {
    let handle = open(path, OPEN_READ);
    if handle == SYSCALL_ERROR {
        write(b"cannot open path\r\n");
        return;
    }

    let mut buffer = [0; 64];
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

fn write_file(arguments: &[u8]) {
    let Some(separator) = arguments.iter().position(|&byte| byte == b' ') else {
        write(WRITE_USAGE);
        return;
    };
    let (path, text) = arguments.split_at(separator);
    let text = text.trim_ascii_start();
    let mut buffer = [0; LINE_LIMIT + 2];
    buffer[..text.len()].copy_from_slice(text);
    let length = text.len() + 2;
    buffer[text.len()..length].copy_from_slice(b"\r\n");

    let handle = open(path, OPEN_WRITE_TRUNCATE);
    if handle == SYSCALL_ERROR {
        write(b"cannot open path\r\n");
        return;
    }
    let count = syscall(
        SYS_WRITE,
        handle,
        buffer.as_mut_ptr().expose_provenance(),
        length,
    );
    syscall(SYS_CLOSE, handle, 0, 0);
    if count != length {
        write(b"cannot write path\r\n");
    }
}

fn open(path: &[u8], mode: usize) -> usize {
    let mut buffer = [0; LINE_LIMIT];
    buffer[..path.len()].copy_from_slice(path);
    syscall(
        SYS_OPEN,
        buffer.as_mut_ptr().expose_provenance(),
        path.len(),
        mode,
    )
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
