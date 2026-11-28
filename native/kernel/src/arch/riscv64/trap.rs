use clv_abi::TaskCompletion;
use core::arch::{asm, global_asm};

use super::{idle, timer};
use crate::console::{write_hex, write_task};
use crate::drivers::uart;
use crate::drivers::virtio::net::{self, Nic};
use crate::drivers::virtio::rng::BootSeeds;
use crate::fs::ramfs::RamFs;
use crate::memory::sv39::AddressSpace;
use crate::memory::usercopy::SSTATUS_SUM;
use crate::programs::{NETD_TASK_INDEX, TASK_COUNT};
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
    nic: Option<Nic>,
}

static mut KERNEL_STATE: KernelState = KernelState {
    scheduler: None,
    ramfs: RamFs::new(),
    nic: None,
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
/// vector and preparing all user address spaces. The NIC's MMIO and DMA
/// storage must remain mapped in supervisor mode.
pub(crate) unsafe fn start(
    tasks: [(AddressSpace, usize); TASK_COUNT],
    nic: Option<Nic>,
    net_seeds: BootSeeds,
    unix_time_offset: i64,
    after_paging: impl FnOnce(&mut Scheduler, &mut RamFs),
) -> ! {
    // SAFETY: No user task has run, and only this hart accesses the state.
    let state = unsafe { &mut *(&raw mut KERNEL_STATE) };
    state.nic = nic;
    let scheduler = state
        .scheduler
        .insert(Scheduler::new(tasks, supervisor_status()));
    if let Some(nic) = &state.nic {
        scheduler.start_netd(
            nic.mac(),
            net_seeds.transport,
            unix_time_offset,
            net_seeds.crypto,
        );
    }
    let space = scheduler.address_space();
    uart::write(b"boot frame=");
    write_hex(space.root_address());
    uart::write(b"\r\n");
    if !space.activate() {
        uart::write(b"Sv39 paging setup failed\r\n");
        idle();
    }
    uart::write(b"Sv39 paging enabled\r\n");
    after_paging(scheduler, &mut state.ramfs);
    let first_frame = *scheduler.current_frame();
    arm_timer(scheduler);
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
    if scause & INTERRUPT_FLAG != 0 && scause != timer::INTERRUPT_CAUSE {
        supervisor_trap_fatal(scause, frame.sepc, stval);
    }
    if scheduler.expire_goal_if_due(fs) {
        // The frame belongs to a cancelled member. Skip its pending syscall or interrupted PC.
        resume_after_stop(scheduler, frame, SwitchReason::Expiry);
        return;
    }
    if scause == timer::INTERRUPT_CAUSE {
        let (receive, transmit) = net::poll(&mut state.nic)
            .map_or((false, false), |ready| (ready.receive, ready.transmit));
        scheduler.wake_netd(receive, transmit);
        if scheduler.preempt(frame) {
            switch_task(scheduler, current, SwitchReason::Timer);
        }
        arm_timer(scheduler);
        // The frame now contains the selected task's interrupted instruction.
        return;
    }
    if scause != USER_ECALL {
        write_task(current);
        uart::write(b" fault");
        write_trap_registers(scause, frame.sepc, stval);
        if scheduler.is_netd() {
            net::shutdown(&mut state.nic);
        }
        scheduler.stop_current(fs, TaskCompletion::Faulted(scause));
        resume_after_stop(scheduler, frame, SwitchReason::Fault);
        return;
    }

    match syscall::dispatch(scheduler, fs, &mut state.nic, frame) {
        syscall::Outcome::Resume => {}
        syscall::Outcome::Exit(status) => {
            if scheduler.is_netd() {
                net::shutdown(&mut state.nic);
            }
            if !scheduler.has_active_pipe_reader() {
                write_task(current);
                uart::write(b" user exit status=");
                write_hex(status);
                uart::write(b"\r\n");
            }
            scheduler.stop_current(fs, TaskCompletion::Exited(status));
            resume_after_stop(scheduler, frame, SwitchReason::Exit);
        }
        syscall::Outcome::Wait => {
            switch_task(scheduler, current, SwitchReason::Wait);
            arm_timer(scheduler);
        }
    }
}

fn resume_after_stop(scheduler: &mut Scheduler, frame: &mut TrapFrame, reason: SwitchReason) {
    let previous = scheduler.current_id();
    if !scheduler.switch(frame) {
        uart::write(b"all tasks exited\r\n");
        idle();
    }
    switch_task(scheduler, previous, reason);
    arm_timer(scheduler);
}

enum SwitchReason {
    Timer,
    Wait,
    Exit,
    Fault,
    Expiry,
}

fn switch_task(scheduler: &Scheduler, previous: usize, reason: SwitchReason) {
    if !scheduler.address_space().activate() {
        uart::write(b"Sv39 task switch failed\r\n");
        idle();
    }
    // Keep routine switch traces off the UART for I/O and network service wakeups.
    let trace: &[u8] = match reason {
        SwitchReason::Fault => b"fault switch ",
        SwitchReason::Expiry => return,
        _ if previous == NETD_TASK_INDEX + 1
            || scheduler.is_netd()
            || scheduler.has_active_pipe_reader()
            || scheduler.host_request_pending() =>
        {
            return;
        }
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

fn arm_timer(scheduler: &Scheduler) {
    if !timer::arm(scheduler.netd_deadline()) {
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
