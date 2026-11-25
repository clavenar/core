#![no_std]
#![no_main]

use core::arch::asm;
use core::panic::PanicInfo;

const SYS_EXIT: usize = 1;
const SYS_WRITE_CONSOLE: usize = 2;
const SYS_TIMER_FIRED: usize = 3;
const SYS_READ_CONSOLE: usize = 4;

// SAFETY: The kernel enters this unique symbol with an aligned user stack.
#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
extern "C" fn _start() -> ! {
    let mut line = [0; 64];
    let mut length = 0;
    let mut previous_was_cr = false;

    // Let both boot tasks get a turn before drawing the prompt.
    while syscall(SYS_TIMER_FIRED, 0, 0) == 0 {
        core::hint::spin_loop();
    }
    write(b"clv> ");

    loop {
        let Ok(byte) = u8::try_from(syscall(SYS_READ_CONSOLE, 0, 0)) else {
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
                match &line[..length] {
                    b"help" => write(b"help  exit\r\n"),
                    b"exit" => exit(0),
                    b"" => {}
                    _ => write(b"unknown command\r\n"),
                }
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

fn write<const N: usize>(text: &[u8; N]) {
    // Console writes take their source bytes from the user stack.
    let mut buffer = *text;
    syscall(
        SYS_WRITE_CONSOLE,
        buffer.as_mut_ptr().expose_provenance(),
        N,
    );
}

fn exit(status: usize) -> ! {
    syscall(SYS_EXIT, status, 0);
    loop {
        core::hint::spin_loop();
    }
}

fn syscall(number: usize, mut argument: usize, length: usize) -> usize {
    // SAFETY: Clavenar preserves registers other than the result in a0.
    unsafe {
        asm!(
            "ecall",
            inlateout("a0") argument,
            in("a1") length,
            in("a7") number,
            options(nostack),
        );
    }
    argument
}

#[panic_handler]
fn panic(_info: &PanicInfo<'_>) -> ! {
    exit(1)
}
