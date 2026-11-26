#![no_std]
#![no_main]

mod arch;
mod console;
mod drivers;
mod fs;
mod ipc;
mod memory;
mod programs;
mod scheduler;
mod syscall;

use arch::riscv64::{idle, trap};
use console::write_hex;
use core::panic::PanicInfo;
use drivers::uart;
use memory::frame::FrameAllocator;
use memory::sv39;
use memory::user_image;

// Make Cargo rebuild the kernel when the linker script changes.
const _: &str = include_str!("../../linker/riscv64.ld");

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
    let images = programs::BOOT_IMAGES
        .map(|(name, image)| load_user_program(&mut frames, name.as_bytes(), image));
    let pages = images.each_ref().map(|image| image.pages);
    let mappings = sv39::BootMappings::new(&mut frames, pages, uart::BASE)
        .expect("boot frame pool cannot hold supervisor mappings");
    let programs = core::array::from_fn(|index| {
        (
            mappings
                .task_address_space(&mut frames, index)
                .expect("boot frame pool cannot hold user mappings"),
            images[index].entry,
        )
    });
    // SAFETY: The boot hart installed the trap vector and built every task's address space.
    unsafe { trap::start(programs) }
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
