use core::arch::asm;
use core::ptr::NonNull;

use super::frame::{FRAME_SIZE, Frame, FrameAllocator};
use super::user_image::{MAX_USER_IMAGE_PAGES, USER_TEXT_BASE};
use super::usercopy::UserStack;

// Preserve the original stack address for one- and two-page programs.
const MIN_USER_STACK_BASE: usize = USER_TEXT_BASE + 2 * FRAME_SIZE;

const PAGE_SHIFT: usize = 12;
const VPN_BITS: usize = 9;
const SUPERPAGE_SHIFT: usize = PAGE_SHIFT + VPN_BITS;
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

pub(crate) struct BootMappings<const PROGRAMS: usize> {
    text: usize,
    uart: usize,
    user_code: [[Option<usize>; MAX_USER_IMAGE_PAGES]; PROGRAMS],
    kernel_middle: usize,
    mmio_leaves: usize,
}

impl<const PROGRAMS: usize> BootMappings<PROGRAMS> {
    pub(crate) fn new(
        frames: &mut FrameAllocator<'_>,
        user_code: [[Option<&Frame>; MAX_USER_IMAGE_PAGES]; PROGRAMS],
        uart: usize,
        bridge: Option<usize>,
    ) -> Option<Self> {
        let kernel_middle = frames.allocate_zeroed()?;
        let kernel_leaves = frames.allocate_zeroed()?;
        let mmio_leaves = frames.allocate_zeroed()?;

        // The linker symbols mark page boundaries. Only their addresses are used.
        let text = (&raw const __text_start).addr();
        let rodata = (&raw const __rodata_start).addr();
        let writable = (&raw const __writable_start).addr();
        let end = (&raw const __kernel_end).addr();
        let user_code =
            user_code.map(|pages| pages.map(|frame| frame.map(Frame::physical_address)));

        map_range(kernel_leaves, text, rodata, READ | EXECUTE);
        map_range(kernel_leaves, rodata, writable, READ);
        map_range(kernel_leaves, writable, end, READ | WRITE | DIRTY);
        // Remove user image frames from the writable identity map before activation.
        for &code in user_code.iter().flatten().flatten() {
            kernel_leaves.words_mut()[vpn_index(code, 0)] = 0;
        }
        map_range(mmio_leaves, uart, uart + FRAME_SIZE, READ | WRITE | DIRTY);
        if let Some(bridge) = bridge {
            // The selected transport must fit in the UART's supervisor leaf table.
            if bridge >> SUPERPAGE_SHIFT != uart >> SUPERPAGE_SHIFT
                || !bridge.is_multiple_of(FRAME_SIZE)
            {
                return None;
            }
            map_range(
                mmio_leaves,
                bridge,
                bridge + FRAME_SIZE,
                READ | WRITE | DIRTY,
            );
        }
        kernel_middle.words_mut()[vpn_index(text, 1)] =
            pte(kernel_leaves.physical_address(), VALID);

        Some(Self {
            text,
            uart,
            user_code,
            kernel_middle: kernel_middle.physical_address(),
            mmio_leaves: mmio_leaves.physical_address(),
        })
    }

    pub(crate) fn task_address_space(
        &self,
        frames: &mut FrameAllocator<'_>,
        program: usize,
    ) -> Option<AddressSpace> {
        let user_code = *self.user_code.get(program)?;
        let root = frames.allocate_zeroed()?;
        let low_middle = frames.allocate_zeroed()?;
        let user_leaves = frames.allocate_zeroed()?;
        let stack_frame = frames.allocate_zeroed()?;

        // The user image and stack share a leaf table. MMIO uses a separate
        // supervisor leaf table under the same low root entry on QEMU virt.
        let entries = user_leaves.words_mut();
        let mut stack_base = MIN_USER_STACK_BASE;
        for (index, page) in user_code.into_iter().enumerate() {
            if let Some(address) = page {
                let virtual_address = USER_TEXT_BASE + index * FRAME_SIZE;
                entries[vpn_index(virtual_address, 0)] =
                    pte(address, VALID | ACCESSED | READ | EXECUTE | USER);
                stack_base = stack_base.max(virtual_address + FRAME_SIZE);
            }
        }
        entries[vpn_index(stack_base, 0)] = pte(
            stack_frame.physical_address(),
            VALID | ACCESSED | READ | WRITE | DIRTY | USER,
        );

        low_middle.words_mut()[vpn_index(self.uart, 1)] = pte(self.mmio_leaves, VALID);
        low_middle.words_mut()[vpn_index(USER_TEXT_BASE, 1)] =
            pte(user_leaves.physical_address(), VALID);
        root.words_mut()[vpn_index(self.text, 2)] = pte(self.kernel_middle, VALID);
        root.words_mut()[vpn_index(self.uart, 2)] = pte(low_middle.physical_address(), VALID);
        Some(AddressSpace {
            root: root.physical_address(),
            stack: UserStack { base: stack_base },
            stack_frame: NonNull::from(stack_frame),
        })
    }
}

#[derive(Clone, Copy)]
pub(crate) struct AddressSpace {
    root: usize,
    pub(crate) stack: UserStack,
    stack_frame: NonNull<Frame>,
}

impl AddressSpace {
    pub(crate) fn root_address(self) -> usize {
        self.root
    }

    /// # Safety
    /// The task must be stopped and its stack frame must be writable through
    /// the supervisor identity map.
    pub(crate) unsafe fn clear_user_stack(self) {
        // SAFETY: The frame is reserved for this task and the caller ensures
        // exclusive access while clearing it.
        unsafe { self.stack_frame.as_ptr().write_bytes(0, 1) };
    }

    pub(crate) fn activate(self) -> bool {
        let satp = SATP_SV39 | (self.root >> PAGE_SHIFT);
        let active: usize;
        // SAFETY: All address spaces keep the same supervisor mappings and
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
