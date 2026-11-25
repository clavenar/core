#![no_std]

//! Syscall numbers and register values for the Clavenar boot user ABI.

pub const SYS_PROBE: usize = 0;
pub const SYS_EXIT: usize = 1;
pub const SYS_WRITE_CONSOLE: usize = 2;
pub const SYS_TIMER_FIRED: usize = 3;
pub const SYS_READ_CONSOLE: usize = 4;
pub const SYS_OPEN: usize = 5;
pub const SYS_READ: usize = 6;
pub const SYS_READDIR: usize = 7;
pub const SYS_CLOSE: usize = 8;

pub const SYSCALL_ERROR: usize = usize::MAX;
pub const PROBE_VALUE: usize = 42;
