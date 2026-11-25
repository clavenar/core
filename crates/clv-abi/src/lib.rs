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
pub const SYS_WRITE: usize = 9;
pub const SYS_UNLINK: usize = 10;
pub const SYS_MKDIR: usize = 11;
pub const SYS_CHDIR: usize = 12;
pub const SYS_GETCWD: usize = 13;
pub const SYS_RMDIR: usize = 14;
pub const SYS_SPAWN: usize = 15;
pub const SYS_WAIT: usize = 16;
pub const SYS_OPEN_AT: usize = 17;
pub const SYS_SPAWN_AT: usize = 18;
pub const SYS_MESSAGE_SEND: usize = 19;
pub const SYS_MESSAGE_RECV: usize = 20;
pub const SYS_SPAWN_WITH_MESSAGE: usize = 21;
pub const SYS_SPAWN_AT_WITH_MESSAGE: usize = 22;

pub const MESSAGE_MAX_BYTES: usize = 64;
pub const FILE_REPLY_OK: u8 = 0;
pub const FILE_REPLY_ERROR: u8 = 1;
pub const PROGRAM_HELLO: usize = 0;
pub const WAIT_EXITED: usize = 0;
pub const WAIT_FAULTED: usize = 1;
pub const OPEN_READ: usize = 0;
pub const OPEN_WRITE_TRUNCATE: usize = 1;
pub const SYSCALL_ERROR: usize = usize::MAX;
pub const PROBE_VALUE: usize = 42;
