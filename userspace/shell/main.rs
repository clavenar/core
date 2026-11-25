#![no_std]
#![no_main]

use clv_abi::{
    FILE_REPLY_ERROR, FILE_REPLY_OK, MESSAGE_MAX_BYTES, NO_CHILD_HANDLE, OPEN_READ,
    OPEN_WRITE_TRUNCATE, PROGRAM_ECHO, PROGRAM_HELLO, SYS_CHDIR, SYS_CLOSE, SYS_EXIT, SYS_GETCWD,
    SYS_MESSAGE_RECV, SYS_MKDIR, SYS_OPEN, SYS_READ, SYS_READ_CONSOLE, SYS_READDIR, SYS_RMDIR,
    SYS_SPAWN, SYS_SPAWN_AT, SYS_SPAWN_AT_WITH_MESSAGE, SYS_SPAWN_WITH_MESSAGE, SYS_TIMER_FIRED,
    SYS_UNLINK, SYS_WAIT, SYS_WRITE, SYS_WRITE_CONSOLE, SYSCALL_ERROR, WAIT_EXITED, WAIT_FAULTED,
};
use core::arch::asm;
use core::panic::PanicInfo;

const LINE_LIMIT: usize = 64;
const WRITE_USAGE: &[u8] = b"usage: write <path> <text>\r\n";
const ASKAT_USAGE: &[u8] = b"usage: askat hello <dir> <path>\r\n";

struct Child {
    name: &'static [u8],
    selector: usize,
    handle: usize,
}

// SAFETY: The kernel enters this unique symbol with an aligned user stack.
#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
extern "C" fn _start() -> ! {
    let mut line = [0; LINE_LIMIT];
    let mut length = 0;
    let mut overflow = false;
    let mut previous_was_cr = false;
    let mut children = [
        Child {
            name: b"hello",
            selector: PROGRAM_HELLO,
            handle: NO_CHILD_HANDLE,
        },
        Child {
            name: b"echo",
            selector: PROGRAM_ECHO,
            handle: NO_CHILD_HANDLE,
        },
    ];

    // Let the boot greeting run before drawing the prompt.
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
                    run_command(&line[..length], &mut children);
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

fn run_command(line: &[u8], children: &mut [Child]) {
    let (command, arguments) = split_word(line.trim_ascii());
    match command {
        b"help" if arguments.is_empty() => write(
            b"help pwd ls [path] cd mkdir rmdir cat write rm exit\r\n\
              run hello [dir]  run echo\r\n\
              ask <hello|echo> <text>  askat hello <dir> <path>\r\n\
              recv <hello|echo>  wait <hello|echo>\r\n",
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
        b"run" | b"ask" | b"askat" | b"recv" | b"wait" => {
            child_command(command, arguments, children);
        }
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

fn child_command(command: &[u8], arguments: &[u8], children: &mut [Child]) {
    let (program, arguments) = split_word(arguments);
    let child = children.iter_mut().find(|child| child.name == program);
    match (command, child) {
        (b"run", Some(child)) if arguments.is_empty() || child.selector == PROGRAM_HELLO => {
            run_child(child, arguments);
        }
        (b"run", _) => write(b"usage: run hello [dir] or run echo\r\n"),
        (b"ask", Some(child)) if !arguments.is_empty() => ask_child(child, arguments),
        (b"ask", _) => write(b"usage: ask <hello|echo> <text>\r\n"),
        (b"askat", Some(child)) if child.selector == PROGRAM_HELLO => {
            ask_at_hello(child, arguments)
        }
        (b"askat", _) => write(ASKAT_USAGE),
        (b"recv", Some(child)) if arguments.is_empty() => recv_child(child.handle),
        (b"recv", _) => write(b"usage: recv <hello|echo>\r\n"),
        (b"wait", Some(child)) if arguments.is_empty() => wait_child(child),
        (b"wait", _) => write(b"usage: wait <hello|echo>\r\n"),
        _ => {}
    }
}

fn run_child(child: &mut Child, directory: &[u8]) {
    if directory.is_empty() {
        let handle = syscall(SYS_SPAWN, child.selector, 0, 0);
        if handle == SYSCALL_ERROR {
            write(child.name);
            write(b" is already running\r\n");
        } else {
            child.handle = handle;
        }
    } else if let Some(handle) = run_scoped_hello(directory, None) {
        child.handle = handle;
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

fn wait_child(child: &Child) {
    let (kind, mut value) = ecall(SYS_WAIT, child.handle, 0, 0, 0);
    let status: &[u8] = match kind {
        WAIT_EXITED => b" exited 0x",
        WAIT_FAULTED => b" fault scause=0x",
        _ => {
            write(b"no pending launch\r\n");
            return;
        }
    };
    write(child.name);
    write(status);

    let mut line = *b"0000000000000000\r\n";
    for digit in line[..16].iter_mut().rev() {
        *digit = b"0123456789abcdef"[value & 0xf];
        value >>= 4;
    }
    write_stack(&line);
}

fn ask_child(child: &mut Child, text: &[u8]) {
    // The text is a slice of the command line on the user stack.
    let handle = syscall(
        SYS_SPAWN_WITH_MESSAGE,
        child.selector,
        text.as_ptr().expose_provenance(),
        text.len(),
    );
    if handle == SYSCALL_ERROR {
        write(b"cannot ask ");
        write(child.name);
        write(b"\r\n");
        return;
    }
    child.handle = handle;
    recv_child(handle);
}

fn recv_child(handle: usize) {
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

fn ask_at_hello(child: &mut Child, arguments: &[u8]) {
    let (directory, path) = split_word(arguments);
    if directory.is_empty() || path.is_empty() {
        write(ASKAT_USAGE);
        return;
    }
    let Some(handle) = run_scoped_hello(directory, Some(path)) else {
        return;
    };
    child.handle = handle;

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
