#![no_std]
#![no_main]

mod children;
mod color;
mod command;
mod files;
mod goal;
mod help;

use children::Children;
use clv::print;
use command::Command;
use core::panic::PanicInfo;
use help::Entry;

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
        Command::Help if arguments.is_empty() => help::all(),
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
        Command::Event => Entry::Event.usage(),
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

// Buffer short lines in one console call and stream long, escaped audit fields.
struct ConsoleLine {
    buffer: [u8; 128],
    len: usize,
}

impl ConsoleLine {
    fn new() -> Self {
        Self {
            buffer: [0; 128],
            len: 0,
        }
    }

    fn push(&mut self, byte: u8) {
        self.buffer[self.len] = byte;
        self.len += 1;
        if self.len == self.buffer.len() {
            clv::write_console(&self.buffer);
            self.len = 0;
        }
    }

    fn write(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.push(byte);
        }
    }

    fn quoted(&mut self, bytes: &[u8]) {
        self.push(b'"');
        for byte in bytes.escape_ascii() {
            self.push(byte);
        }
        self.push(b'"');
    }

    fn finish(&mut self, parts: &[&[u8]]) {
        for part in parts {
            self.write(part);
        }
        self.write(b"\r\n");
        if self.len > 0 {
            clv::write_console(&self.buffer[..self.len]);
        }
    }
}

fn print_line(parts: &[&[u8]]) {
    ConsoleLine::new().finish(parts);
}

#[panic_handler]
fn panic(_info: &PanicInfo<'_>) -> ! {
    clv::exit(1)
}
