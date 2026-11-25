#![no_std]
#![no_main]

use clv_abi::{
    FILE_REPLY_ERROR, FILE_REPLY_OK, MESSAGE_MAX_BYTES, NO_CHILD_HANDLE, OPEN_READ,
    OPEN_WRITE_TRUNCATE, PROGRAM_HELLO, SYS_CHDIR, SYS_CLOSE, SYS_EXIT, SYS_GETCWD,
    SYS_MESSAGE_RECV, SYS_MKDIR, SYS_OPEN, SYS_READ, SYS_READ_CONSOLE, SYS_READDIR, SYS_RMDIR,
    SYS_SPAWN, SYS_SPAWN_AT, SYS_SPAWN_AT_WITH_MESSAGE, SYS_SPAWN_WITH_MESSAGE, SYS_TIMER_FIRED,
    SYS_UNLINK, SYS_WAIT, SYS_WRITE, SYS_WRITE_CONSOLE, SYSCALL_ERROR, WAIT_EXITED, WAIT_FAULTED,
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
    let mut hello_handle = NO_CHILD_HANDLE;

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
                    run_command(&line[..length], &mut hello_handle);
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

fn run_command(line: &[u8], hello_handle: &mut usize) {
    let (command, arguments) = split_word(line.trim_ascii());
    match command {
        b"help" if arguments.is_empty() => write(
            b"help pwd ls [path] cd mkdir rmdir cat write rm exit\r\nrun hello [dir]  ask hello <text>  recv hello  wait hello\r\naskat hello <dir> <path>\r\n",
        ),
        b"pwd" if arguments.is_empty() => pwd(),
        b"cd" => run_path_command(
            SYS_CHDIR,
            arguments,
            b"usage: cd <path>\r\n",
            b"cannot change directory\r\n",
        ),
        b"ls" if arguments.is_empty() => show(b".", SYS_READDIR),
        b"ls" => show(arguments, SYS_READDIR),
        b"mkdir" => run_path_command(
            SYS_MKDIR,
            arguments,
            b"usage: mkdir <path>\r\n",
            b"cannot create directory\r\n",
        ),
        b"rmdir" => run_path_command(
            SYS_RMDIR,
            arguments,
            b"usage: rmdir <path>\r\n",
            b"cannot remove directory\r\n",
        ),
        b"cat" if arguments.is_empty() => write(b"usage: cat <path>\r\n"),
        b"cat" => show(arguments, SYS_READ),
        b"write" => write_file(arguments),
        b"rm" => run_path_command(
            SYS_UNLINK,
            arguments,
            b"usage: rm <path>\r\n",
            b"cannot remove path\r\n",
        ),
        b"run" => run_hello(arguments, hello_handle),
        b"ask" => ask_hello(arguments, hello_handle),
        b"askat" => ask_at_hello(arguments, hello_handle),
        b"recv" if arguments == b"hello" => recv_hello(*hello_handle),
        b"recv" => write(b"usage: recv hello\r\n"),
        b"wait" if arguments == b"hello" => wait_hello(*hello_handle),
        b"wait" => write(b"usage: wait hello\r\n"),
        b"exit" if arguments.is_empty() => exit(0),
        b"" => {}
        _ => write(b"unknown command\r\n"),
    }
}

fn split_word(line: &[u8]) -> (&[u8], &[u8]) {
    let end = line
        .iter()
        .position(|&byte| byte == b' ')
        .unwrap_or(line.len());
    let (word, rest) = line.split_at(end);
    (word, rest.trim_ascii_start())
}

fn run_path_command(number: usize, path: &[u8], usage: &[u8], error: &[u8]) {
    if path.is_empty() {
        write(usage);
    } else if call_path(number, path, 0) == SYSCALL_ERROR {
        write(error);
    }
}

fn pwd() {
    // The absolute path can span several separately entered components.
    let mut buffer = [0; 512];
    let count = syscall(
        SYS_GETCWD,
        buffer.as_mut_ptr().expose_provenance(),
        buffer.len(),
        0,
    );
    let Some(path) = buffer.get(..count) else {
        write(b"cannot get current directory\r\n");
        return;
    };
    write(path);
    write(b"\r\n");
}

fn run_hello(arguments: &[u8], hello_handle: &mut usize) {
    let (program, directory) = split_word(arguments);
    if program != b"hello" {
        write(b"usage: run hello [dir]\r\n");
        return;
    }
    if directory.is_empty() {
        let handle = syscall(SYS_SPAWN, PROGRAM_HELLO, 0, 0);
        if handle == SYSCALL_ERROR {
            write(b"hello is already running\r\n");
        } else {
            *hello_handle = handle;
        }
    } else if let Some(handle) = run_scoped_hello(directory, None) {
        *hello_handle = handle;
    }
}

fn run_scoped_hello(directory: &[u8], request: Option<&[u8]>) -> Option<usize> {
    let directory_handle = call_path(SYS_OPEN, directory, OPEN_READ);
    if directory_handle == SYSCALL_ERROR {
        write(b"cannot open path\r\n");
        return None;
    }
    let handle = match request {
        Some(path) => {
            ecall(
                SYS_SPAWN_AT_WITH_MESSAGE,
                PROGRAM_HELLO,
                directory_handle,
                path.as_ptr().expose_provenance(),
                path.len(),
            )
            .0
        }
        None => syscall(SYS_SPAWN_AT, PROGRAM_HELLO, directory_handle, 0),
    };
    syscall(SYS_CLOSE, directory_handle, 0, 0);
    if handle == SYSCALL_ERROR {
        write(b"cannot run scoped hello\r\n");
        return None;
    }
    Some(handle)
}

fn wait_hello(handle: usize) {
    let (kind, mut value) = ecall(SYS_WAIT, handle, 0, 0, 0);
    match kind {
        WAIT_EXITED => write(b"hello exited 0x"),
        WAIT_FAULTED => write(b"hello fault scause=0x"),
        _ => {
            write(b"no pending launch\r\n");
            return;
        }
    }

    let mut line = *b"0000000000000000\r\n";
    for digit in line[..16].iter_mut().rev() {
        *digit = b"0123456789abcdef"[value & 0xf];
        value >>= 4;
    }
    write_stack(&line);
}

fn ask_hello(arguments: &[u8], hello_handle: &mut usize) {
    let (program, text) = split_word(arguments);
    if program != b"hello" || text.is_empty() {
        write(b"usage: ask hello <text>\r\n");
        return;
    }
    // The text is a slice of the command line on the user stack.
    let handle = syscall(
        SYS_SPAWN_WITH_MESSAGE,
        PROGRAM_HELLO,
        text.as_ptr().expose_provenance(),
        text.len(),
    );
    if handle == SYSCALL_ERROR {
        write(b"cannot ask hello\r\n");
        return;
    }
    *hello_handle = handle;
    recv_hello(handle);
}

fn recv_hello(handle: usize) {
    if handle == NO_CHILD_HANDLE {
        write(b"no message\r\n");
        return;
    }
    let mut buffer = [0; MESSAGE_MAX_BYTES];
    match receive_message(handle, &mut buffer) {
        Some([]) => write(b"no message\r\n"),
        Some(message) => {
            write_stack(message);
            write(b"\r\n");
        }
        None => write(b"recv failed\r\n"),
    }
}

fn ask_at_hello(arguments: &[u8], hello_handle: &mut usize) {
    let (program, arguments) = split_word(arguments);
    let (directory, path) = split_word(arguments);
    if program != b"hello" || directory.is_empty() || path.is_empty() {
        write(b"usage: askat hello <dir> <path>\r\n");
        return;
    }
    let Some(handle) = run_scoped_hello(directory, Some(path)) else {
        return;
    };
    *hello_handle = handle;

    let mut buffer = [0; MESSAGE_MAX_BYTES];
    match receive_message(handle, &mut buffer) {
        Some([FILE_REPLY_OK]) => write(b"empty file\r\n"),
        Some([FILE_REPLY_OK, contents @ ..]) => {
            write_stack(contents);
            if !contents.ends_with(b"\n") {
                write(b"\r\n");
            }
        }
        Some([FILE_REPLY_ERROR]) => write(b"cannot read requested file\r\n"),
        Some([]) => write(b"no message\r\n"),
        _ => write(b"recv failed\r\n"),
    }
}

fn receive_message(handle: usize, buffer: &mut [u8]) -> Option<&[u8]> {
    let count = syscall(
        SYS_MESSAGE_RECV,
        buffer.as_mut_ptr().expose_provenance(),
        buffer.len(),
        handle,
    );
    buffer.get(..count)
}

fn show(path: &[u8], operation: usize) {
    let handle = call_path(SYS_OPEN, path, OPEN_READ);
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
    let (path, text) = split_word(arguments);
    if text.is_empty() {
        write(WRITE_USAGE);
        return;
    }
    let mut buffer = [0; LINE_LIMIT + 2];
    buffer[..text.len()].copy_from_slice(text);
    let length = text.len() + 2;
    buffer[text.len()..length].copy_from_slice(b"\r\n");

    let handle = call_path(SYS_OPEN, path, OPEN_WRITE_TRUNCATE);
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

fn call_path(number: usize, path: &[u8], mode: usize) -> usize {
    let mut buffer = [0; LINE_LIMIT];
    buffer[..path.len()].copy_from_slice(path);
    syscall(
        number,
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

fn syscall(number: usize, a0: usize, a1: usize, a2: usize) -> usize {
    ecall(number, a0, a1, a2, 0).0
}

#[inline(never)]
fn ecall(number: usize, mut a0: usize, mut a1: usize, a2: usize, a3: usize) -> (usize, usize) {
    // SAFETY: Clavenar returns results in a0 and a1 and preserves other registers.
    unsafe {
        asm!(
            "ecall",
            inlateout("a0") a0,
            inlateout("a1") a1,
            in("a2") a2,
            in("a3") a3,
            in("a7") number,
            options(nostack),
        );
    }
    (a0, a1)
}

#[panic_handler]
fn panic(_info: &PanicInfo<'_>) -> ! {
    exit(1)
}
