#![no_std]
#![no_main]

mod children;
mod files;

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
                    run_command(&line[..length], &mut children);
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

fn run_command(line: &[u8], children: &mut Children) {
    let (command, arguments) = split_word(line.trim_ascii());
    match command {
        b"help" if arguments.is_empty() => print(
            b"help pwd ls [path] cd mkdir rmdir cat write rm exit\r\n\
              run hello [dir]  run echo\r\n\
              ask [--read] <hello|echo> <text>  askat [--echo] hello <dir> <payload>\r\n\
              askfile [--stream] hello <path>\r\n\
              recv <hello|echo>  wait <hello|echo>\r\n",
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

#[panic_handler]
fn panic(_info: &PanicInfo<'_>) -> ! {
    clv::exit(1)
}
