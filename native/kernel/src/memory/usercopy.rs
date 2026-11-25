use core::arch::asm;

pub(crate) const SSTATUS_SUM: usize = 1 << 18;

unsafe extern "C" {
    static __user_stack_start: u8;
    static __user_stack_top: u8;
}

pub(crate) fn copy_from_user_stack(source: usize, destination: &mut [u8]) -> bool {
    if destination.is_empty() {
        return true;
    }

    let Some(end) = source.checked_add(destination.len()) else {
        return false;
    };
    if source < (&raw const __user_stack_start).addr() || end > (&raw const __user_stack_top).addr()
    {
        return false;
    }

    // SAFETY: The validated source range is wholly within the boot user
    // stack mapping. The destination is writable supervisor memory, and
    // the boot hart keeps interrupts disabled. SUM is cleared before return.
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
            destination = inout(reg) destination.as_mut_ptr() => _,
            count = inout(reg) destination.len() => _,
            byte = out(reg) _,
            sum = in(reg) SSTATUS_SUM,
            options(nostack),
        );
    }
    true
}
