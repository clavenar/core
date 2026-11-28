use clv_abi::agent::SHA256_BYTES;
use clv_abi::tool::{FILE_TOOLS, ToolId};
use clv_abi::{PROGRAMS, Program};

static TOOL_IMAGE_HASHES: [[u8; SHA256_BYTES]; FILE_TOOLS.len()] =
    include!(concat!(env!("OUT_DIR"), "/tool-image-hashes.rs"));

pub(crate) fn file_tool(id: ToolId) -> Option<(Program, &'static [u8; SHA256_BYTES])> {
    FILE_TOOLS
        .iter()
        .zip(&TOOL_IMAGE_HASHES)
        .find(|(tool, _)| tool.id == id)
        .map(|(tool, digest)| (tool.program, digest))
}

pub(crate) const SHELL_TASK_INDEX: usize = 1;
const COMMAND_IMAGE_COUNT: usize = PROGRAMS.len() + 1;
const NETD_IMAGE_INDEX: usize = COMMAND_IMAGE_COUNT;
pub(crate) const BOOT_IMAGE_COUNT: usize = NETD_IMAGE_INDEX + 1;
pub(crate) const ECHO_REPORT_TASK_INDEX: usize = COMMAND_IMAGE_COUNT;
pub(crate) const GOAL_MONITOR_TASK_INDEX: usize = ECHO_REPORT_TASK_INDEX + 1;
pub(crate) const NETD_TASK_INDEX: usize = GOAL_MONITOR_TASK_INDEX + 1;
pub(crate) const TASK_COUNT: usize = NETD_TASK_INDEX + 1;

/// The boot shell occupies task 2, between the original hello and echo slots.
pub(crate) const fn task_index(program: Program) -> usize {
    let selector = program as usize;
    if selector < SHELL_TASK_INDEX {
        selector
    } else {
        selector + 1
    }
}

pub(crate) const fn task_image_index(task: usize) -> usize {
    match task {
        ECHO_REPORT_TASK_INDEX | GOAL_MONITOR_TASK_INDEX => task_index(Program::Echo),
        NETD_TASK_INDEX => NETD_IMAGE_INDEX,
        _ => task,
    }
}

pub(crate) const BOOT_IMAGES: [(&str, &[u8]); BOOT_IMAGE_COUNT] = {
    let tools: [&[u8]; PROGRAMS.len()] = include!(concat!(env!("OUT_DIR"), "/program-images.rs"));
    let mut images = [(
        "shell",
        include_bytes!(concat!(env!("OUT_DIR"), "/shell.elf")) as &[u8],
    ); BOOT_IMAGE_COUNT];
    let mut i = 0;
    while i < PROGRAMS.len() {
        let (program, name) = PROGRAMS[i];
        images[task_index(program)] = (name, tools[i]);
        i += 1;
    }
    images[NETD_IMAGE_INDEX] = (
        "netd",
        include_bytes!(concat!(env!("OUT_DIR"), "/netd.elf")),
    );
    images
};
