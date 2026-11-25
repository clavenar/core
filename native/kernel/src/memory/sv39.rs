use core::arch::asm;

use super::frame::{FRAME_SIZE, Frame, FrameAllocator};
use super::user_image::USER_TEXT_BASE;

pub(crate) const USER_STACK_BASE: usize = USER_TEXT_BASE + FRAME_SIZE;
pub(crate) const USER_STACK_TOP: usize = USER_STACK_BASE + FRAME_SIZE;

const PAGE_SHIFT: usize = 12;
const VPN_BITS: usize = 9;
const PTE_PPN_SHIFT: usize = 10;
const SATP_SV39: usize = 8 << 60;

const VALID: u64 = 1 << 0;
const READ: u64 = 1 << 1;
const WRITE: u64 = 1 << 2;
const EXECUTE: u64 = 1 << 3;
const USER: u64 = 1 << 4;
const ACCESSED: u64 = 1 << 6;
const DIRTY: u64 = 1 << 7;

unsafe extern "C" {
    static __text_start: u8;
    static __rodata_start: u8;
    static __writable_start: u8;
    static __kernel_end: u8;
}

pub(crate) struct BootMappings {
    text: usize,
    uart: usize,
    user_code: usize,
    kernel_middle: usize,
    uart_leaves: usize,
}

impl BootMappings {
    pub(crate) fn new(
        frames: &mut FrameAllocator<'_>,
        user_code: &Frame,
        uart: usize,
    ) -> Option<Self> {
        let kernel_middle = frames.allocate_zeroed()?;
        let kernel_leaves = frames.allocate_zeroed()?;
        let uart_leaves = frames.allocate_zeroed()?;

        // The linker symbols mark page boundaries. Only their addresses are used.
        let text = (&raw const __text_start).addr();
        let rodata = (&raw const __rodata_start).addr();
        let writable = (&raw const __writable_start).addr();
        let end = (&raw const __kernel_end).addr();
        let user_code = user_code.physical_address();

        map_range(kernel_leaves, text, rodata, READ | EXECUTE);
        map_range(kernel_leaves, rodata, writable, READ);
        // The loaded code gets only the user RX mapping, with no writable alias.
        map_range(kernel_leaves, writable, user_code, READ | WRITE | DIRTY);
        map_range(
            kernel_leaves,
            user_code + FRAME_SIZE,
            end,
            READ | WRITE | DIRTY,
        );
        map_range(uart_leaves, uart, uart + FRAME_SIZE, READ | WRITE | DIRTY);
        kernel_middle.words_mut()[vpn_index(text, 1)] =
            pte(kernel_leaves.physical_address(), VALID);

        Some(Self {
            text,
            uart,
            user_code,
            kernel_middle: kernel_middle.physical_address(),
            uart_leaves: uart_leaves.physical_address(),
        })
    }

    pub(crate) fn task_address_space(
        &self,
        frames: &mut FrameAllocator<'_>,
    ) -> Option<AddressSpace> {
        let root = frames.allocate_zeroed()?;
        let low_middle = frames.allocate_zeroed()?;
        let user_leaves = frames.allocate_zeroed()?;
        let user_stack = frames.allocate_zeroed()?;

        // The two user pages share a leaf table. The UART uses a separate
        // leaf table under the same low root entry on QEMU virt.
        let entries = user_leaves.words_mut();
        entries[vpn_index(USER_TEXT_BASE, 0)] =
            pte(self.user_code, VALID | ACCESSED | READ | EXECUTE | USER);
        entries[vpn_index(USER_STACK_BASE, 0)] = pte(
            user_stack.physical_address(),
            VALID | ACCESSED | READ | WRITE | DIRTY | USER,
        );

        low_middle.words_mut()[vpn_index(self.uart, 1)] = pte(self.uart_leaves, VALID);
        low_middle.words_mut()[vpn_index(USER_TEXT_BASE, 1)] =
            pte(user_leaves.physical_address(), VALID);
        root.words_mut()[vpn_index(self.text, 2)] = pte(self.kernel_middle, VALID);
        root.words_mut()[vpn_index(self.uart, 2)] = pte(low_middle.physical_address(), VALID);
        Some(AddressSpace {
            root: root.physical_address(),
        })
    }
}

#[derive(Clone, Copy)]
pub(crate) struct AddressSpace {
    root: usize,
}

impl AddressSpace {
    pub(crate) fn root_address(self) -> usize {
        self.root
    }

    pub(crate) fn activate(self) -> bool {
        let satp = SATP_SV39 | (self.root >> PAGE_SHIFT);
        let active: usize;
        // SAFETY: Both address spaces keep the same supervisor mappings and
        // their tables stay reserved. ASID zero is reused after a full fence.
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
