use clv_abi::TaskCompletion;
use core::arch::{asm, global_asm};

use super::{idle, timer};
use crate::console::{write_hex, write_task};
use crate::drivers::uart;
use crate::fs::ramfs::RamFs;
use crate::memory::sv39::AddressSpace;
use crate::memory::usercopy::SSTATUS_SUM;
use crate::programs::TASK_COUNT;
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

    pub(crate) fn set_startup_args(&mut self, a1: usize, a2: usize, a3: usize) {
        self.registers[11] = a1;
        self.registers[12] = a2;
        self.registers[13] = a3;
    }
}

struct KernelState {
    scheduler: Option<Scheduler>,
    ramfs: RamFs,
}

static mut KERNEL_STATE: KernelState = KernelState {
    scheduler: None,
    ramfs: RamFs::new(),
};

unsafe extern "C" {
    fn enter_user(frame: &TrapFrame) -> !;
}

fn supervisor_status() -> usize {
    let status: usize;
    // SAFETY: Kernel code runs in supervisor mode.
    unsafe { asm!("csrr {status}, sstatus", status = out(reg) status, options(nomem, nostack)) };
    status
}

/// # Safety
///
/// Only the boot hart may start the scheduler, after installing the trap
/// vector and preparing all user address spaces.
pub(crate) unsafe fn start(programs: [(AddressSpace, usize); TASK_COUNT]) -> ! {
    // SAFETY: No user task has run, and only this hart accesses the state.
    let state = unsafe { &mut *(&raw mut KERNEL_STATE) };
    let scheduler = state
        .scheduler
        .insert(Scheduler::new(programs, supervisor_status()));
    let space = scheduler.address_space();
    uart::write(b"boot frame=");
    write_hex(space.root_address());
    uart::write(b"\r\n");
    if !space.activate() {
        uart::write(b"Sv39 paging setup failed\r\n");
        idle();
    }
    uart::write(b"Sv39 paging enabled\r\n");
    let first_frame = *scheduler.current_frame();
    arm_timer();
    // SAFETY: The first task's code and stack are mapped in the active space.
    unsafe { enter_user(&first_frame) }
}

// SAFETY: The trap stub uses this unique symbol with the RISC-V C ABI.
#[unsafe(no_mangle)]
extern "C" fn supervisor_user_trap(frame: &mut TrapFrame, scause: usize, stval: usize) {
    // SAFETY: On the single boot hart this handler runs with supervisor
    // interrupts disabled. Supervisor traps take the fatal path instead.
    let state = unsafe { &mut *(&raw mut KERNEL_STATE) };
    let scheduler = state
        .scheduler
        .as_mut()
        .expect("scheduler is initialized before user entry");
    let fs = &mut state.ramfs;
    let current = scheduler.current_id();
    if scause == timer::INTERRUPT_CAUSE {
        if scheduler.preempt(frame) {
            switch_task(scheduler, current, SwitchReason::Timer);
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
        finish_task(
            scheduler,
            fs,
            frame,
            TaskCompletion::Faulted(scause),
            SwitchReason::Fault,
        );
        return;
    }

    match syscall::dispatch(scheduler, fs, frame) {
        syscall::Outcome::Resume => {}
        syscall::Outcome::Exit(status) => {
            if !scheduler.has_active_pipe_reader() {
                write_task(current);
                uart::write(b" user exit status=");
                write_hex(status);
                uart::write(b"\r\n");
            }
            finish_task(
                scheduler,
                fs,
                frame,
                TaskCompletion::Exited(status),
                SwitchReason::Exit,
            );
        }
        syscall::Outcome::Wait => {
            switch_task(scheduler, current, SwitchReason::Wait);
            arm_timer();
        }
    }
}

fn finish_task(
    scheduler: &mut Scheduler,
    fs: &mut RamFs,
    frame: &mut TrapFrame,
    completion: TaskCompletion,
    reason: SwitchReason,
) {
    let previous = scheduler.current_id();
    if !scheduler.stop_current(fs, frame, completion) {
        uart::write(b"all tasks exited\r\n");
        idle();
    }
    switch_task(scheduler, previous, reason);
    arm_timer();
}

enum SwitchReason {
    Timer,
    Wait,
    Exit,
    Fault,
}

fn switch_task(scheduler: &Scheduler, previous: usize, reason: SwitchReason) {
    if !scheduler.address_space().activate() {
        uart::write(b"Sv39 task switch failed\r\n");
        idle();
    }
    // Routine switch traces share the shell's UART with the bytes it is draining.
    let trace: &[u8] = match reason {
        SwitchReason::Fault => b"fault switch ",
        _ if scheduler.has_active_pipe_reader() => return,
        SwitchReason::Timer => b"timer switch ",
        SwitchReason::Wait => b"wait switch ",
        SwitchReason::Exit => b"exit switch ",
    };
    uart::write(trace);
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
