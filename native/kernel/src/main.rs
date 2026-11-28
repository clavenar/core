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
use drivers::virtio::console::{Console, Port, ProbeError};
use drivers::virtio::{block, net};
use fs::ramfs::{RamFs, SavedFiles};
use memory::frame::FrameAllocator;
use memory::sv39;
use memory::user_image;
use scheduler::Scheduler;

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

    let journal = probe_block();
    // SAFETY: QEMU virt supplies the MMIO windows, and paging is still disabled.
    let nic = unsafe { net::Nic::attach() };
    // SAFETY: The transport windows are still accessible before paging.
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
        [
            bridge.as_ref().map(Console::base),
            journal.as_ref().map(block::JournalDevice::base),
            nic.as_ref().map(net::Nic::base),
        ]
        .into_iter()
        .flatten(),
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
    let after_paging = |scheduler: &mut Scheduler, fs: &mut RamFs| {
        if let Some(disk) = journal {
            if disk.is_checkpoint() {
                scheduler.attach_checkpoint_disk(disk);
            } else {
                scheduler.attach_durable_disk(recover_durable_disk(disk, fs));
            }
        }
        scheduler.attach_host_bridge(bridge.and_then(probe_bridge));
    };
    // SAFETY: The boot hart installed the trap vector and built every task's address space.
    unsafe { trap::start(tasks, nic, after_paging) }
}

fn probe_bridge(bridge: Console) -> Option<Port> {
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
}

fn recover_durable_disk(disk: block::JournalDevice, fs: &mut RamFs) -> Option<block::JournalDisk> {
    use block::JournalLoadError;
    let count = disk.file_count();
    let (journal, saved) = match disk.open_journal(|payload| SavedFiles::decode(payload, count)) {
        Ok(recovered) => recovered,
        Err(error) => {
            uart::write(match error {
                JournalLoadError::TooSmall if count == 2 => {
                    b"durable rename journal needs five sectors\r\n"
                }
                JournalLoadError::TooSmall => b"durable rename journal needs three sectors\r\n",
                JournalLoadError::InvalidSlots => b"durable rename journal invalid slots\r\n",
                JournalLoadError::Io(error) => block_error_message(error),
            });
            return None;
        }
    };
    if let Some(files) = saved {
        fs.restore_durable_files(files);
        let record = journal
            .latest_record()
            .expect("recovered files have a journal record");
        uart::write(b"durable rename recovered generation=");
        write_hex(record.generation as usize);
        uart::write(b" checksum=");
        write_hex(record.checksum as usize);
        uart::write(b"\r\n");
    }
    uart::write(if journal.is_writable() {
        b"durable rename journal ready\r\n"
    } else {
        b"durable rename journal writes unavailable\r\n"
    });
    Some(journal)
}

fn probe_block() -> Option<block::JournalDevice> {
    // SAFETY: This is called once on the boot hart, before enabling paging.
    let result = unsafe { block::probe() }?;
    match result {
        Ok(probe) => {
            uart::write(b"virtio block sectors=");
            write_hex(probe.sectors as usize);
            uart::write(b" sector0=");
            write_hex(u64::from_be_bytes(probe.prefix) as usize);
            uart::write(b"\r\n");
            use block::{DemoOutcome, JournalLoadError, ScratchProbe, WriteRestriction};
            match probe.scratch {
                Some(ScratchProbe::Marker(demo)) => uart::write(match demo {
                    Ok(DemoOutcome::Flushed) => b"virtio block demo flushed\r\n",
                    Ok(DemoOutcome::Recovered) => b"virtio block demo recovered\r\n",
                    Ok(DemoOutcome::Blocked(WriteRestriction::ReadOnly)) => {
                        b"virtio block demo read-only\r\n"
                    }
                    Ok(DemoOutcome::Blocked(WriteRestriction::FlushUnavailable)) => {
                        b"virtio block demo flush unavailable\r\n"
                    }
                    Ok(DemoOutcome::Occupied) => b"virtio block demo sector 1 occupied\r\n",
                    Ok(DemoOutcome::TooSmall) => b"virtio block demo needs two sectors\r\n",
                    Err(error) => block_error_message(error),
                }),
                Some(ScratchProbe::Journal(Ok(probe))) => report_journal(probe),
                Some(ScratchProbe::Journal(Err(error))) => uart::write(match error {
                    JournalLoadError::TooSmall => b"virtio block journal needs three sectors\r\n",
                    JournalLoadError::InvalidSlots => b"virtio block journal invalid slots\r\n",
                    JournalLoadError::Io(error) => block_error_message(error),
                }),
                None => {}
            }
            probe.durable_disk
        }
        Err(error) => {
            uart::write(block_error_message(error));
            None
        }
    }
}

fn report_journal(probe: block::JournalProbe) {
    use block::{JournalUpdate, WriteRestriction};

    if let Some(record) = &probe.recovered {
        write_journal_record(b"recovered", record);
    }
    match probe.update {
        JournalUpdate::Flushed(record) => write_journal_record(b"flushed", &record),
        JournalUpdate::Blocked(WriteRestriction::ReadOnly) if probe.recovered.is_none() => {
            uart::write(b"virtio block journal empty\r\n");
        }
        JournalUpdate::Blocked(WriteRestriction::ReadOnly) => {}
        JournalUpdate::Blocked(WriteRestriction::FlushUnavailable) => {
            uart::write(b"virtio block journal flush unavailable\r\n");
        }
        JournalUpdate::GenerationExhausted => {
            uart::write(b"virtio block journal generation exhausted\r\n");
        }
        JournalUpdate::Failed(error) | JournalUpdate::Unconfirmed(error, _) => {
            uart::write(block_error_message(error));
        }
    }
}

fn write_journal_record(action: &[u8], record: &block::JournalRecord) {
    uart::write(b"virtio block journal ");
    uart::write(action);
    uart::write(b" generation=");
    write_hex(record.generation as usize);
    uart::write(b" sector=");
    write_hex(record.sector as usize);
    uart::write(b" payload_prefix=");
    write_hex(u64::from_be_bytes(record.prefix) as usize);
    uart::write(b"\r\n");
}

fn block_error_message(error: block::ProbeError) -> &'static [u8] {
    match error {
        block::ProbeError::Device => b"virtio block unavailable\r\n",
        block::ProbeError::Empty => b"virtio block empty\r\n",
        block::ProbeError::Read => b"virtio block read failed\r\n",
        block::ProbeError::Write => b"virtio block write failed\r\n",
        block::ProbeError::Flush => b"virtio block flush failed\r\n",
        block::ProbeError::InvalidCompletion => b"virtio block invalid completion\r\n",
        block::ProbeError::Timeout => b"virtio block timed out\r\n",
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
