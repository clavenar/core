use core::arch::asm;

use super::frame::{FRAME_SIZE, Frame, FrameAllocator};

const PAGE_SHIFT: usize = 12;
const VPN_BITS: usize = 9;
const PTE_PPN_SHIFT: usize = 10;
const SATP_SV39: usize = 8 << 60;

const VALID: u64 = 1 << 0;
const READ: u64 = 1 << 1;
const WRITE: u64 = 1 << 2;
const EXECUTE: u64 = 1 << 3;
const ACCESSED: u64 = 1 << 6;
const DIRTY: u64 = 1 << 7;

unsafe extern "C" {
    static __text_start: u8;
    static __rodata_start: u8;
    static __writable_start: u8;
    static __kernel_end: u8;
}

/// Installs the boot supervisor address space.
///
/// # Safety
///
/// Address translation must be off when called on the boot hart. The caller
/// must supply a fresh root frame and the QEMU virt UART base. After activation
/// the root must remain intact while this address space is active.
pub(crate) unsafe fn enable_identity_map(
    root: &mut Frame,
    frames: &mut FrameAllocator<'_>,
    uart_base: usize,
) -> bool {
    let (Some(kernel_middle), Some(kernel_leaves), Some(uart_middle), Some(uart_leaves)) = (
        frames.allocate_zeroed(),
        frames.allocate_zeroed(),
        frames.allocate_zeroed(),
        frames.allocate_zeroed(),
    ) else {
        return false;
    };

    // The linker symbols mark page boundaries. Only their addresses are used.
    let text = (&raw const __text_start).addr();
    let rodata = (&raw const __rodata_start).addr();
    let writable = (&raw const __writable_start).addr();
    let end = (&raw const __kernel_end).addr();

    // The image fits in one aligned 2 MiB region. UART is in a separate
    // root entry on QEMU virt, so each region needs one pair of child tables.
    map_range(kernel_leaves, text, rodata, READ | EXECUTE);
    map_range(kernel_leaves, rodata, writable, READ);
    map_range(kernel_leaves, writable, end, READ | WRITE | DIRTY);
    map_range(
        uart_leaves,
        uart_base,
        uart_base + FRAME_SIZE,
        READ | WRITE | DIRTY,
    );

    kernel_middle.words_mut()[vpn_index(text, 1)] = pte(kernel_leaves.physical_address(), VALID);
    uart_middle.words_mut()[vpn_index(uart_base, 1)] = pte(uart_leaves.physical_address(), VALID);
    root.words_mut()[vpn_index(text, 2)] = pte(kernel_middle.physical_address(), VALID);
    root.words_mut()[vpn_index(uart_base, 2)] = pte(uart_middle.physical_address(), VALID);

    let satp = SATP_SV39 | (root.physical_address() >> PAGE_SHIFT);
    let active: usize;
    // SAFETY: The map covers the running code, stack, trap vector, and all
    // kernel data at their current addresses. The fences order table writes
    // and synchronize address translation on this hart.
    unsafe {
        asm!(
            "sfence.vma zero, zero",
            "csrw satp, {satp}",
            "sfence.vma zero, zero",
            "csrr {active}, satp",
            satp = in(reg) satp,
            active = lateout(reg) active,
            options(nostack),
        );
    }
    active == satp
}

fn map_range(table: &mut Frame, start: usize, end: usize, access: u64) {
    let entries = table.words_mut();
    for address in (start..end).step_by(FRAME_SIZE) {
        entries[vpn_index(address, 0)] = pte(address, VALID | ACCESSED | access);
    }
}

fn vpn_index(address: usize, level: usize) -> usize {
    (address >> (PAGE_SHIFT + level * VPN_BITS)) & ((1 << VPN_BITS) - 1)
}

fn pte(address: usize, flags: u64) -> u64 {
    ((address as u64 >> PAGE_SHIFT) << PTE_PPN_SHIFT) | flags
}
