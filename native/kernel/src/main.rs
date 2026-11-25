#![no_std]
#![no_main]

mod arch;
mod console;
mod drivers;
mod fs;
mod memory;
mod scheduler;
mod syscall;

use arch::riscv64::{idle, trap};
use console::write_hex;
use core::panic::PanicInfo;
use drivers::uart;
use memory::frame::FrameAllocator;
use memory::sv39;
use memory::user_image;
use scheduler::Scheduler;

// Make Cargo rebuild the kernel when the linker script changes.
const _: &str = include_str!("../../linker/riscv64.ld");

const BOOT_PROGRAMS: [(&[u8], &[u8]); 2] = [
    (
        b"hello",
        include_bytes!(concat!(env!("OUT_DIR"), "/hello.elf")),
    ),
    (
        b"shell",
        include_bytes!(concat!(env!("OUT_DIR"), "/shell.elf")),
    ),
];

// SAFETY: The boot stub calls this symbol with a RISC-V ABI-aligned stack.
#[unsafe(no_mangle)]
extern "C" fn kernel_main() -> ! {
    let banner = concat!(
        "Clavenar kernel booted (v",
        env!("CARGO_PKG_VERSION"),
        ")\r\n"
    );
    uart::write(banner.as_bytes());

    // SAFETY: The single boot hart claims the frame pool here once.
    let mut frames = unsafe { FrameAllocator::from_boot_pool() };
    let [hello, shell] =
        BOOT_PROGRAMS.map(|(name, image)| load_user_program(&mut frames, name, image));
    let mappings = sv39::BootMappings::new(&mut frames, [hello.pages, shell.pages], uart::BASE)
        .expect("boot frame pool cannot hold supervisor mappings");
    let programs = [
        (
            mappings
                .task_address_space(&mut frames, 0)
                .expect("no frames for task 1"),
            hello.entry,
        ),
        (
            mappings
                .task_address_space(&mut frames, 1)
                .expect("no frames for task 2"),
            shell.entry,
        ),
    ];
    let scheduler = Scheduler::new(programs, trap::supervisor_status());

    uart::write(b"boot frame=");
    write_hex(scheduler.address_space().root_address());
    uart::write(b"\r\n");
    if !scheduler.address_space().activate() {
        uart::write(b"Sv39 paging setup failed\r\n");
        idle();
    }
    uart::write(b"Sv39 paging enabled\r\n");
    // SAFETY: The boot hart has installed the first address space and trap vector.
    unsafe { trap::start(scheduler) }
}

fn load_user_program<'a>(
    frames: &mut FrameAllocator<'a>,
    name: &[u8],
    image: &[u8],
) -> user_image::LoadedImage<'a> {
    let Some(loaded) = user_image::load(image, frames) else {
        uart::write(b"User ELF load failed for ");
        uart::write(name);
        uart::write(b"\r\n");
        idle();
    };
    uart::write(b"User ELF loaded entry=");
    write_hex(loaded.entry);
    uart::write(b" program=");
    uart::write(name);
    uart::write(b"\r\n");
    loaded
}

#[panic_handler]
fn panic(_info: &PanicInfo<'_>) -> ! {
    idle();
}
