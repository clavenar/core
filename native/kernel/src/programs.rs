use clv_abi::agent::SHA256_BYTES;
use clv_abi::tool::{FILE_TOOLS, ToolId};
use clv_abi::{PROGRAMS, Program, USER_IMAGE_MAX_BYTES};

use crate::image_limits::NETD_IMAGE_MAX_BYTES;

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
// TLS certificate verification uses the most stack when both probes are built.
const NETD_STACK_PAGES: usize = if cfg!(feature = "tls-probe") {
    24
} else if cfg!(feature = "http-probe") {
    6
} else {
    4
};
pub(crate) const STACK_PAGE_COUNT: usize = TASK_COUNT - 1 + NETD_STACK_PAGES;

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

pub(crate) const fn task_stack_pages(task: usize) -> usize {
    match task {
        NETD_TASK_INDEX => NETD_STACK_PAGES,
        _ => 1,
    }
}

#[derive(Clone, Copy)]
pub(crate) struct BootImage {
    pub(crate) name: &'static str,
    pub(crate) elf: &'static [u8],
    pub(crate) max_bytes: usize,
}

pub(crate) const BOOT_IMAGES: [BootImage; BOOT_IMAGE_COUNT] = {
    let tools: [&[u8]; PROGRAMS.len()] = include!(concat!(env!("OUT_DIR"), "/program-images.rs"));
    let mut images = [BootImage {
        name: "shell",
        elf: include_bytes!(concat!(env!("OUT_DIR"), "/shell.elf")),
        max_bytes: USER_IMAGE_MAX_BYTES,
    }; BOOT_IMAGE_COUNT];
    let mut i = 0;
    while i < PROGRAMS.len() {
        let (program, name) = PROGRAMS[i];
        images[task_index(program)] = BootImage {
            name,
            elf: tools[i],
            max_bytes: USER_IMAGE_MAX_BYTES,
        };
        i += 1;
    }
    images[NETD_IMAGE_INDEX] = BootImage {
        name: "netd",
        elf: include_bytes!(concat!(env!("OUT_DIR"), "/netd.elf")),
        max_bytes: NETD_IMAGE_MAX_BYTES,
    };
    images
};
