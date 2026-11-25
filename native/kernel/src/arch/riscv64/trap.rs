use core::arch::{asm, global_asm};

use super::{idle, timer};
use crate::console::{write_hex, write_task};
use crate::drivers::uart;
use crate::fs::ramfs::RamFs;
use crate::memory::usercopy::SSTATUS_SUM;
use crate::scheduler::Scheduler;
use crate::syscall;

global_asm!(
    include_str!("trap.S"),
    frame_size = const size_of::<TrapFrame>(),
    sepc_offset = const core::mem::offset_of!(TrapFrame, sepc),
    sstatus_offset = const core::mem::offset_of!(TrapFrame, sstatus),
    sum_mask = const SSTATUS_SUM,
);

const USER_ECALL: usize = 8;
const INTERRUPT_FLAG: usize = 1 << (usize::BITS - 1);
// Clear SUM, FS, VS, SPP, SPIE and SIE for a new user context.
const USER_STATUS_CLEAR: usize =
    SSTATUS_SUM | (3 << 13) | (3 << 9) | (1 << 8) | (1 << 5) | (1 << 1);

#[derive(Clone, Copy)]
#[repr(C, align(16))]
pub(crate) struct TrapFrame {
    pub(crate) registers: [usize; 32],
    pub(crate) sepc: usize,
    sstatus: usize,
}

impl TrapFrame {
    pub(crate) fn new_user(entry: usize, stack_top: usize, task_id: usize, sstatus: usize) -> Self {
        let mut registers = [0; 32];
        registers[2] = stack_top;
        registers[10] = task_id;
        Self {
            registers,
            sepc: entry,
            sstatus: sstatus & !USER_STATUS_CLEAR,
        }
    }
}

struct KernelState {
    scheduler: Scheduler,
    ramfs: RamFs,
}

static mut KERNEL_STATE: Option<KernelState> = None;

unsafe extern "C" {
    fn enter_user(frame: &TrapFrame) -> !;
}

pub(crate) fn supervisor_status() -> usize {
    let status: usize;
    // SAFETY: Kernel code runs in supervisor mode.
    unsafe { asm!("csrr {status}, sstatus", status = out(reg) status, options(nomem, nostack)) };
    status
}

/// # Safety
///
/// Only the boot hart may start the scheduler, once the first task's address
/// space and the supervisor trap vector are active.
pub(crate) unsafe fn start(scheduler: Scheduler) -> ! {
    let first_frame = *scheduler.current_frame();
    // SAFETY: No user task has run, and only this hart accesses the state.
    unsafe {
        KERNEL_STATE = Some(KernelState {
            scheduler,
            ramfs: RamFs::new(),
        });
    }
    arm_timer();
    // SAFETY: The first task's code and stack are mapped in the active space.
    unsafe { enter_user(&first_frame) }
}

// SAFETY: The trap stub uses this unique symbol with the RISC-V C ABI.
#[unsafe(no_mangle)]
extern "C" fn supervisor_user_trap(frame: &mut TrapFrame, scause: usize, stval: usize) {
    // SAFETY: On the single boot hart this handler runs with supervisor
    // interrupts disabled. Supervisor traps take the fatal path instead.
    let state = unsafe { &mut *(&raw mut KERNEL_STATE) }
        .as_mut()
        .expect("kernel state is initialized before user entry");
    let scheduler = &mut state.scheduler;
    let current = scheduler.current_id();
    if scause == timer::INTERRUPT_CAUSE {
        if scheduler.preempt(frame) {
            switch_task(scheduler, current, b"timer switch ");
        }
        arm_timer();
        // The frame now contains the selected task's interrupted instruction.
        return;
    }
    if scause != USER_ECALL {
        if scause & INTERRUPT_FLAG != 0 {
            supervisor_trap_fatal(scause, frame.sepc, stval);
        }
        write_task(current);
        uart::write(b" fault");
        write_trap_registers(scause, frame.sepc, stval);
        finish_task(scheduler, frame, b"fault switch ");
        return;
    }

    if let syscall::Outcome::Exit(status) = syscall::dispatch(scheduler, &mut state.ramfs, frame) {
        write_task(current);
        uart::write(b" user exit status=");
        write_hex(status);
        uart::write(b"\r\n");
        finish_task(scheduler, frame, b"exit switch ");
    }
}

fn finish_task(scheduler: &mut Scheduler, frame: &mut TrapFrame, reason: &[u8]) {
    let previous = scheduler.current_id();
    if !scheduler.stop_current(frame) {
        uart::write(b"all tasks exited\r\n");
        idle();
    }
    switch_task(scheduler, previous, reason);
    arm_timer();
}

fn switch_task(scheduler: &Scheduler, previous: usize, reason: &[u8]) {
    if !scheduler.address_space().activate() {
        uart::write(b"Sv39 task switch failed\r\n");
        idle();
    }
    uart::write(reason);
    write_task(previous);
    uart::write(b" -> ");
    write_task(scheduler.current_id());
    uart::write(b"\r\n");
}

fn arm_timer() {
    if !timer::arm() {
        uart::write(b"SBI timer setup failed\r\n");
        idle();
    }
}

// SAFETY: The trap stub uses this unique symbol with the RISC-V C ABI.
#[unsafe(no_mangle)]
extern "C" fn supervisor_trap_fatal(scause: usize, sepc: usize, stval: usize) -> ! {
    uart::write(b"fatal trap");
    write_trap_registers(scause, sepc, stval);
    idle();
}

fn write_trap_registers(scause: usize, sepc: usize, stval: usize) {
    uart::write(b" scause=");
    write_hex(scause);
    uart::write(b" sepc=");
    write_hex(sepc);
    uart::write(b" stval=");
    write_hex(stval);
    uart::write(b"\r\n");
}
