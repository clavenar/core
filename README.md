# Clavenar Core

Clavenar Core is the engineering repository for the native operating system,
Linux-hosted runtime, and virtual machine integration. The first implementation
targets RV64 on QEMU `virt`.

- `native/kernel/` and `native/linker/` hold kernel source and linker scripts.
- `crates/` holds shared Rust types and ABI definitions. [`clv-abi`](crates/clv-abi/)
  defines the syscall numbers used by the kernel and both user programs.
- `userspace/` holds native programs and libraries.
- `platforms/qemu-riscv64-virt/` holds the reference platform definition.
- `tests/` holds cross-component tests and fixtures.
- `docs/` holds implementation notes. `xtask/` will hold build orchestration.

See the [repository layout](../specifications/engineering/repository-layout.md)
in a sibling checkout for the day-one bootstrap and future components.

## Build and run the RV64 kernel

With stable Rust installed via `rustup`, add the bare-metal target. On Debian 13,
`qemu-system-riscv` provides the emulator and pulls in OpenSBI.

```sh
rustup target add --toolchain stable riscv64gc-unknown-none-elf
sudo apt-get install --no-install-recommends qemu-system-riscv
```

Run these commands from the `core` repository root.

```sh
cargo +stable rustc --manifest-path native/kernel/Cargo.toml \
  --target riscv64gc-unknown-none-elf --target-dir target --release \
  -- -C "link-arg=-T${PWD}/native/linker/riscv64.ld" -C link-arg=--no-relax

qemu-system-riscv64 -machine virt -smp 1 -m 128M -bios default -nographic \
  -kernel target/riscv64gc-unknown-none-elf/release/clavenar-kernel
```

After the OpenSBI banner, the kernel prints `Clavenar kernel booted` with the
version from `native/kernel/Cargo.toml`. It prints `boot frame=0x...` with the
physical address of task 1's Sv39 root table.

The build links [hello](userspace/hello/) and [the shell](userspace/shell/) as
separate RV64 ELFs and embeds both in the kernel. The loader accepts static,
little-endian ELF64 executables with one read/execute segment of at most
4 KiB at `0x00400000`. Boot reports each program's validated entry address.
If loading fails, it prints `User ELF load failed` with the program name and
idles.

The kernel activates paging, prints `Sv39 paging enabled`, and starts hello
as task 1 and the shell as task 2. Each receives its task ID in `a0` and runs
with its own Sv39 root, read/execute code page, and read/write, non-executable
stack mapped at `0x00401000`. The kernel and UART mappings are shared in
supervisor mode. The kernel, supervisor stack, and 16-frame boot pool fit
within one aligned 2 MiB physical region.

OpenSBI timer interrupts drive round-robin scheduling. A `timer switch` line
shows which task was interrupted and which task will run next. The kernel
starts a fresh quantum when the timer fires or a stopped task is replaced.

The user program passes the call number in `a7` and executes `ecall`. Call 0
prints `task N user probe` and returns 42 in `a0`. Call 3 returns 1 in `a0` once
the calling task has received a timer interrupt and 0 until then. The hello
program stores its task ID on its stack before waiting for the interrupt. It
checks that value and its saved registers after resuming.

Call 2 writes to the console. It takes a user stack address in `a0` and a byte
count in `a1`. The kernel accepts up to 128 bytes, copies the full range before
writing, and returns the byte count in `a0`. An empty write returns 0. An
oversized write or a range outside the user stack returns -1 without output.
The hello program uses this call to print `Hello from user mode`.

Call 4 polls the console. It returns one UART byte in `a0` when input is ready
and -1 otherwise. At the `clv> ` prompt, type `help` to see the available
commands.

`mkdir /notes` creates a directory, and `cd /notes` enters it. Use `pwd` to
print the current directory and `ls` to list it. Use `ls /` to list the root
or `ls <path>` to list another directory. In `/notes`, `write todo.txt hello`
creates or replaces a file with one line of text, `cat todo.txt` reads it, and
`rm todo.txt` removes it. Use `cd ..` to go up one directory and `exit` to stop
the shell.

RamFS starts with read-only `/hello.txt` and `/about.txt` and holds up to four
directories besides the root and four writable files of 256 bytes each.
Directories and writable files live in RAM until reboot. Each task starts
in `/` and resolves relative paths from its own current directory. Absolute
paths start at `/`. Within a path, `.` keeps the directory reached so far
and `..` moves to its parent, staying at `/` when already there. Creating a
file or directory requires an existing parent. Each task has four handles,
and each open handle has an independent cursor.

| Call | Arguments | Result in `a0` |
| --- | --- | --- |
| 5 `open` | `a0` path address, `a1` path length up to 64 bytes, `a2` mode | Handle |
| 6 `read` | `a0` file handle, `a1` destination address, `a2` capacity | Bytes copied, or 0 at EOF |
| 7 `readdir` | `a0` directory handle, `a1` destination address, `a2` capacity | One entry name's byte count, or 0 at end |
| 8 `close` | `a0` handle | 0 |
| 9 `write` | `a0` writable handle, `a1` source address, `a2` byte count | Bytes written |
| 10 `unlink` | `a0` path address, `a1` path length up to 64 bytes | 0 |
| 11 `mkdir` | `a0` path address, `a1` path length up to 64 bytes | 0 |
| 12 `chdir` | `a0` path address, `a1` path length up to 64 bytes | 0 |
| 13 `getcwd` | `a0` destination address, `a1` capacity | Absolute path's byte count |

Open mode 0 opens a path for reading. Mode 1 opens a writable file for reading
and writing, creating it if absent and truncating it to zero length otherwise.
File calls return -1 on error. Paths and data buffers must reside in the
calling task's stack. Writes validate and copy the entire input before
changing bytes or the cursor. Reads and directory listings advance their
cursor only after a successful copy. Directory entries contain the name bytes
without a trailing newline or NUL.

`chdir` leaves the current directory unchanged on error. `getcwd` writes the
full absolute path, up to 260 bytes, without a trailing NUL. If the destination
buffer is too small or outside the user stack, it returns -1 and writes nothing.

Unlink removes a writable file's name immediately. Open handles retain access
to that file, and its slot becomes reusable once the last handle closes.
Recreating the path while an old handle is open uses a separate slot.
Task exit and user faults close the task's handles.

Call 1 takes an exit status in `a0` and removes the calling task from the
schedule. In a successful run, both tasks report
`user exit status=0x0000000000000000`, then the kernel prints `all tasks exited`
and idles. Unknown call numbers return -1 in `a0`.

User exceptions print `task N fault` with `scause` (trap cause), `sepc`
(instruction address), and `stval` (additional information, when provided).
The kernel removes the faulting task from the schedule and prints
`fault switch` if another task is runnable. Once the last task stops, the
kernel prints `all tasks exited` and idles.

Supervisor faults and unexpected interrupts print `fatal trap` and stop
execution. To exit QEMU, press `Ctrl+a`, then `x`.

Check Rust formatting with `rustfmt` from the stable toolchain.

```sh
cargo +stable fmt --manifest-path native/kernel/Cargo.toml --check
cargo +stable fmt --manifest-path crates/clv-abi/Cargo.toml --check
rustfmt +stable --edition 2024 --check userspace/hello/main.rs userspace/shell/main.rs
```
