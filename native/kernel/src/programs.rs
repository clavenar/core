use clv_abi::{PROGRAMS, Program};

pub(crate) const SHELL_TASK_INDEX: usize = 1;
pub(crate) const TASK_COUNT: usize = PROGRAMS.len() + 1;

/// The boot shell occupies task 2, between the original hello and echo slots.
pub(crate) const fn task_index(program: Program) -> usize {
    let selector = program as usize;
    if selector < SHELL_TASK_INDEX {
        selector
    } else {
        selector + 1
    }
}

pub(crate) const BOOT_IMAGES: [(&str, &[u8]); TASK_COUNT] = {
    let tools: [&[u8]; PROGRAMS.len()] = include!(concat!(env!("OUT_DIR"), "/program-images.rs"));
    let mut images = [(
        "shell",
        include_bytes!(concat!(env!("OUT_DIR"), "/shell.elf")) as &[u8],
    ); TASK_COUNT];
    let mut i = 0;
    while i < PROGRAMS.len() {
        let (program, name) = PROGRAMS[i];
        images[task_index(program)] = (name, tools[i]);
        i += 1;
    }
    images
};
