#![no_std]
#![no_main]

mod children;
mod files;
mod goal;

use children::Children;
use clv::print;
use core::panic::PanicInfo;

const LINE_LIMIT: usize = 64;

// SAFETY: The kernel enters this unique symbol with an aligned user stack.
#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
extern "C" fn _start() -> ! {
    let mut line = [0; LINE_LIMIT];
    let mut length = 0;
    let mut overflow = false;
    let mut previous_was_cr = false;
    let mut children = Children::new();
    let mut current_goal = None;

    // Let the boot greeting run before drawing the prompt.
    while !clv::timer_fired() {
        core::hint::spin_loop();
    }
    print(b"clv> ");

    loop {
        let Some(byte) = clv::read_console() else {
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
                print(b"\r\n");
                if overflow {
                    print(b"line too long\r\n");
                } else {
                    run_command(&line[..length], &mut children, &mut current_goal);
                }
                length = 0;
                overflow = false;
                print(b"clv> ");
            }
            8 | 127 if length > 0 && !overflow => {
                length -= 1;
                print(b"\x08 \x08");
            }
            b' '..=b'~' if !overflow => {
                if let Some(slot) = line.get_mut(length) {
                    *slot = byte;
                    length += 1;
                    print(&[byte]);
                } else {
                    overflow = true;
                }
            }
            _ => {}
        }
    }
}

fn run_command(line: &[u8], children: &mut Children, current_goal: &mut Option<goal::CurrentGoal>) {
    let (command, arguments) = split_word(line.trim_ascii());
    match command {
        b"help" if arguments.is_empty() => print(
            b"help pwd ls [path] cd mkdir rmdir cat write rm exit\r\n\
              run hello [dir]  run echo\r\n\
              ask [--read] <hello|echo> <text>  askat [--echo] hello <dir> <payload>\r\n\
              askfile [--stream] hello <path>\r\n\
              recv <hello|echo>  wait <hello|echo>  event\r\n\
              goal create [--reply-bytes N]  goal run [--reply-bytes N] <hello|echo>\r\n\
              goal grant-file <path>  goal status  goal budget  goal cancel  goal close\r\n\
              agent run [--suspended] [--reply-bytes N] <hello|echo>\r\n\
              agent status  agent suspend  agent resume\r\n",
        ),
        b"pwd" if arguments.is_empty() => files::pwd(),
        b"cd" => files::cd(arguments),
        b"ls" => files::ls(arguments),
        b"mkdir" => files::mkdir(arguments),
        b"rmdir" => files::rmdir(arguments),
        b"cat" => files::cat(arguments),
        b"write" => files::write_file(arguments),
        b"rm" => files::rm(arguments),
        b"run" => children.run(arguments),
        b"ask" => children.ask(arguments),
        b"askat" => children.ask_at(arguments),
        b"askfile" => children.ask_file(arguments),
        b"recv" => children.recv(arguments),
        b"wait" => children.wait(arguments),
        b"event" if arguments.is_empty() => children.event(),
        b"event" => print(b"usage: event\r\n"),
        b"goal" => goal::command(arguments, current_goal, children),
        b"agent" => goal::agent_command(arguments, current_goal, children),
        b"exit" if arguments.is_empty() => clv::exit(0),
        b"" => {}
        _ => print(b"unknown command\r\n"),
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

fn hex(mut value: usize) -> [u8; 18] {
    let mut bytes = *b"0x0000000000000000";
    for digit in bytes[2..].iter_mut().rev() {
        *digit = b"0123456789abcdef"[value & 0xf];
        value >>= 4;
    }
    bytes
}

fn print_handle(label: &[u8], handle: usize) {
    print_line(&[label, &hex(handle)]);
}

// Emit a complete status line in one console call so task switches cannot split it.
fn print_line(parts: &[&[u8]]) {
    let mut line = [0; 128];
    let mut len = 0;
    for part in parts {
        let end = len + part.len();
        line[len..end].copy_from_slice(part);
        len = end;
    }
    line[len..len + 2].copy_from_slice(b"\r\n");
    clv::write_console(&line[..len + 2]);
}

#[panic_handler]
fn panic(_info: &PanicInfo<'_>) -> ! {
    clv::exit(1)
}
