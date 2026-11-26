#![no_std]
#![no_main]

mod children;
mod color;
mod command;
mod files;
mod goal;

use children::Children;
use clv::print;
use command::Command;
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
    print(color::PROMPT);

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
                print(color::PROMPT);
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
    match Command::parse(command) {
        Command::Help if arguments.is_empty() => {
            print(
                b"help pwd ls [path] cd mkdir rmdir cat write rm exit\r\n\
              run <program>  run hello <dir>\r\n\
              ask [--read] <program> <text>  askat [--echo] hello <dir> <payload>\r\n\
              askfile [--stream] hello <path>\r\n\
              grep -F <pattern> <path>  recv <program>  wait <program>  event\r\n",
            );
            goal::help();
            goal::agent_help();
        }
        Command::Pwd if arguments.is_empty() => files::pwd(),
        Command::Cd => files::cd(arguments),
        Command::Ls => files::ls(arguments),
        Command::Mkdir => files::mkdir(arguments),
        Command::Rmdir => files::rmdir(arguments),
        Command::Cat => files::cat(arguments),
        Command::Write => files::write_file(arguments),
        Command::Rm => files::rm(arguments),
        Command::Run => children.run(arguments),
        Command::Ask => children.ask(arguments),
        Command::AskAt => children.ask_at(arguments),
        Command::AskFile => children.ask_file(arguments),
        Command::Grep => children.grep(arguments),
        Command::Recv => children.recv(arguments),
        Command::Wait => children.wait(arguments),
        Command::Event if arguments.is_empty() => children.event(),
        Command::Event => print(b"usage: event\r\n"),
        Command::Goal => goal::command(arguments, current_goal, children),
        Command::Agent => goal::agent_command(arguments, current_goal, children),
        Command::Exit if arguments.is_empty() => clv::exit(0),
        Command::Empty => {}
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

/// A single- or double-quoted literal may contain spaces or be empty.
fn split_literal(input: &[u8]) -> Option<(&[u8], &[u8])> {
    let (&first, tail) = input.split_first()?;
    if first != b'\'' && first != b'"' {
        return Some(split_word(input));
    }
    let end = tail.iter().position(|&byte| byte == first)?;
    let (literal, rest) = tail.split_at(end);
    let rest = &rest[1..];
    (rest.is_empty() || rest.starts_with(b" ")).then(|| (literal, rest.trim_ascii_start()))
}

fn number(digits: &[u8]) -> Option<usize> {
    if digits.is_empty() {
        return None;
    }
    let mut value = 0usize;
    for &digit in digits {
        if !digit.is_ascii_digit() {
            return None;
        }
        value = value
            .checked_mul(10)?
            .checked_add(usize::from(digit - b'0'))?;
    }
    Some(value)
}

const HEX_DIGITS: &[u8; 16] = b"0123456789abcdef";
fn hex(mut value: usize) -> [u8; 18] {
    let mut bytes = *b"0x0000000000000000";
    for digit in bytes[2..].iter_mut().rev() {
        *digit = HEX_DIGITS[value & 0xf];
        value >>= 4;
    }
    bytes
}

fn print_handle(label: &[u8], handle: usize) {
    print_line(&[label, &hex(handle)]);
}

fn print_usage(syntax: &[u8]) {
    print(b"usage: ");
    print(syntax);
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
