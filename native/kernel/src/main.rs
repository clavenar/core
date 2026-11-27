#![no_std]
#![no_main]

mod arch;
mod console;
mod drivers;
mod fs;
mod host_bridge;
mod ipc;
mod memory;
mod programs;
mod scheduler;
mod syscall;

use arch::riscv64::{idle, trap};
use console::write_hex;
use core::panic::PanicInfo;
use drivers::uart;
use drivers::virtio::block;
use drivers::virtio::console::{Console, ProbeError};
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

    probe_block();
    // SAFETY: QEMU virt supplies the MMIO windows, and paging is still disabled.
    let bridge = unsafe { Console::discover() };
    // SAFETY: The single boot hart claims the frame pool here once.
    let mut frames = unsafe { FrameAllocator::from_boot_pool() };
    let images = programs::BOOT_IMAGES
        .map(|(name, image)| load_user_program(&mut frames, name.as_bytes(), image));
    let pages = images.each_ref().map(|image| image.pages);
    let mappings = sv39::BootMappings::new(
        &mut frames,
        pages,
        uart::BASE,
        bridge.as_ref().map(Console::base),
    )
    .expect("could not build supervisor mappings");
    let tasks = core::array::from_fn(|task| {
        let image = programs::task_image_index(task);
        (
            mappings
                .task_address_space(&mut frames, image)
                .expect("boot frame pool cannot hold user mappings"),
            images[image].entry,
        )
    });
    let probe_bridge = || {
        let bridge = bridge?;
        // SAFETY: This runs on the boot hart with the selected MMIO page mapped.
        match unsafe { bridge.probe() } {
            Ok(port) => {
                uart::write(b"host bridge ready\r\n");
                Some(port)
            }
            Err(error) => {
                uart::write(match error {
                    ProbeError::Device => b"host bridge unavailable\r\n",
                    ProbeError::InvalidReply => b"host bridge invalid reply\r\n",
                    ProbeError::Timeout => b"host bridge timed out\r\n",
                });
                None
            }
        }
    };
    // SAFETY: The boot hart installed the trap vector and built every task's address space.
    unsafe { trap::start(tasks, probe_bridge) }
}

fn probe_block() {
    // SAFETY: This is called once on the boot hart, before enabling paging.
    let Some(result) = (unsafe { block::probe() }) else {
        return;
    };
    match result {
        Ok(probe) => {
            uart::write(b"virtio block sectors=");
            write_hex(probe.sectors as usize);
            uart::write(b" sector0=");
            write_hex(u64::from_be_bytes(probe.prefix) as usize);
            uart::write(b"\r\n");
        }
        Err(error) => uart::write(match error {
            block::ProbeError::Device => b"virtio block unavailable\r\n",
            block::ProbeError::Empty => b"virtio block empty\r\n",
            block::ProbeError::Read => b"virtio block read failed\r\n",
            block::ProbeError::InvalidCompletion => b"virtio block invalid completion\r\n",
            block::ProbeError::Timeout => b"virtio block timed out\r\n",
        }),
    }
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
