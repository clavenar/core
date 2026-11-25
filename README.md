# Clavenar Core

Clavenar Core is the engineering repository for the native operating system,
Linux-hosted runtime, and virtual machine integration. The first implementation
targets RV64 on QEMU `virt`.

- `native/kernel/` and `native/linker/` hold kernel source and linker scripts.
- `crates/` holds shared Rust types and ABI definitions.
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
commands. Use `ls` to list the RamFS root, `cat /hello.txt` or
`cat /about.txt` to read a file, and `exit` to stop the shell.

The read-only RamFS contains `/hello.txt` and `/about.txt`. Paths can be
absolute or relative to the root. Each task has four file handles, and each
open handle has an independent cursor.

| Call | Arguments | Result in `a0` |
| --- | --- | --- |
| 5 `open` | `a0` path address, `a1` path length up to 64 bytes | Handle |
| 6 `read` | `a0` file handle, `a1` destination address, `a2` capacity | Bytes copied, or 0 at EOF |
| 7 `readdir` | `a0` root directory handle, `a1` destination address, `a2` capacity | One entry name's byte count, or 0 at end |
| 8 `close` | `a0` handle | 0 |

File calls return -1 on error. Paths and destination buffers must reside in
the calling task's stack. Reads and directory listings advance their cursor
only after a successful copy. Directory entry names are returned as bytes
without a trailing newline or NUL.

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
rustfmt +stable --edition 2024 --check userspace/hello/main.rs userspace/shell/main.rs
```
