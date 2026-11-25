use core::arch::asm;

use super::sv39::{USER_STACK_BASE, USER_STACK_TOP};

pub(crate) const SSTATUS_SUM: usize = 1 << 18;

pub(crate) fn copy_from_user_stack(source: usize, destination: &mut [u8]) -> bool {
    if !in_user_stack(source, destination.len()) {
        return false;
    }
    // SAFETY: The source is in the current task's stack and the destination
    // slice is writable supervisor memory.
    unsafe {
        copy_bytes(
            source,
            destination.as_mut_ptr().expose_provenance(),
            destination.len(),
        );
    }
    true
}

pub(crate) fn copy_to_user_stack(source: &[u8], destination: usize, capacity: usize) -> bool {
    if source.len() > capacity || !in_user_stack(destination, capacity) {
        return false;
    }
    // SAFETY: The source slice is readable supervisor memory and the copied
    // bytes fit within the validated user destination.
    unsafe {
        copy_bytes(
            source.as_ptr().expose_provenance(),
            destination,
            source.len(),
        )
    };
    true
}

pub(crate) fn in_user_stack(address: usize, length: usize) -> bool {
    length == 0
        || (address >= USER_STACK_BASE
            && address
                .checked_add(length)
                .is_some_and(|end| end <= USER_STACK_TOP))
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
