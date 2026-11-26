#![no_std]
#![no_main]

mod children;
mod files;
mod goal;

use children::Children;
use clv::print;
use core::panic::PanicInfo;

const LINE_LIMIT: usize = 128;

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
              run <program>  run hello <dir>\r\n\
              ask [--read] <program> <text>  askat [--echo] hello <dir> <payload>\r\n\
              askfile [--stream] hello <path>\r\n\
              grep -F <pattern> <path>  recv <program>  wait <program>  event\r\n\
              goal create [--children 1|2] [--reply-bytes N] [--allow-helper <program>] [--read-bytes N]\r\n\
              goal run [--reply-bytes N] <program>\r\n\
              goal grant-file [--delegable | --delegate-to <program>] <path>\r\n\
              goal status  goal budget  goal audit [--replies | --reads]  goal cancel  goal close\r\n\
              agent run [--suspended] [--reply-bytes N] <program>\r\n\
              agent run hello --delegate --with-file --reply-bytes N grep -F <pattern>\r\n\
              agent delegate [--with-file] --reply-bytes N <program> [-F <pattern>]\r\n\
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
        b"grep" => children.grep(arguments),
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

// Short status lines fit in one console call. Extended audit lines can exceed 128 bytes.
fn print_line(parts: &[&[u8]]) {
    let mut line = [0; 128];
    let mut len = 0;
    for &byte in parts.iter().flat_map(|part| part.iter()).chain(b"\r\n") {
        line[len] = byte;
        len += 1;
        if len == line.len() {
            clv::write_console(&line);
            len = 0;
        }
    }
    if len > 0 {
        clv::write_console(&line[..len]);
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo<'_>) -> ! {
    clv::exit(1)
}
