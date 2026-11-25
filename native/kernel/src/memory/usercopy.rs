use core::arch::asm;

use super::frame::FRAME_SIZE;

pub(crate) const SSTATUS_SUM: usize = 1 << 18;

#[derive(Clone, Copy)]
pub(crate) struct UserStack {
    pub(super) base: usize,
}

impl UserStack {
    pub(crate) fn top(self) -> usize {
        self.base + FRAME_SIZE
    }

    pub(crate) fn contains(self, address: usize, length: usize) -> bool {
        length == 0
            || (address >= self.base
                && address
                    .checked_add(length)
                    .is_some_and(|end| end <= self.top()))
    }

    pub(crate) fn copy_from_user(self, source: usize, destination: &mut [u8]) -> bool {
        if !self.contains(source, destination.len()) {
            return false;
        }
        // SAFETY: Syscall dispatch uses the active task's stack. The source is
        // in that stack and the destination slice is writable supervisor memory.
        unsafe {
            copy_bytes(
                source,
                destination.as_mut_ptr().expose_provenance(),
                destination.len(),
            );
        }
        true
    }

    pub(crate) fn copy_to_user(self, source: &[u8], destination: usize, capacity: usize) -> bool {
        if source.len() > capacity || !self.contains(destination, capacity) {
            return false;
        }
        // SAFETY: The source slice is readable supervisor memory and the copied
        // bytes fit within the active task's stack.
        unsafe {
            copy_bytes(
                source.as_ptr().expose_provenance(),
                destination,
                source.len(),
            );
        }
        true
    }
}

// The caller supplies readable and writable ranges on the active address space.
unsafe fn copy_bytes(source: usize, destination: usize, length: usize) {
    if length == 0 {
        return;
    }
    // SAFETY: The syscall handler keeps supervisor interrupts disabled.
    // SUM is enabled only during the copy.
    unsafe {
        asm!(
            "csrs sstatus, {sum}",
            "2:",
            "lbu {byte}, 0({source})",
            "sb {byte}, 0({destination})",
            "addi {source}, {source}, 1",
            "addi {destination}, {destination}, 1",
            "addi {count}, {count}, -1",
            "bnez {count}, 2b",
            "csrc sstatus, {sum}",
            source = inout(reg) source => _,
            destination = inout(reg) destination => _,
            count = inout(reg) length => _,
            byte = out(reg) _,
            sum = in(reg) SSTATUS_SUM,
            options(nostack),
        );
    }
}
