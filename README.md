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

The build links [the hello program](userspace/hello/) as a separate RV64 ELF and
embeds it in the kernel. The loader accepts a static, little-endian ELF64
executable with one read/execute segment of at most 4 KiB at `0x00400000`. Boot
reports the validated entry address as `User ELF loaded entry=0x...`, or prints
`User ELF load failed` and idles if loading fails.

The kernel activates paging, prints `Sv39 paging enabled`, and starts two
tasks at the ELF entry point. Each receives its task ID in `a0`. Both tasks
share the read/execute code page and supervisor-only kernel and UART mappings.
Each task has its own Sv39 root and read/write, non-executable stack mapped
at virtual address `0x00401000`. The kernel, supervisor stack, and 16-frame
boot pool fit within one aligned 2 MiB physical region.

OpenSBI timer interrupts drive round-robin scheduling. A `timer switch` line
shows which task was interrupted and which task will run next. The kernel
starts a fresh quantum after each timer interrupt and exit switch.

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

Call 1 takes an exit status in `a0` and removes the calling task from the
schedule. In a successful run, both tasks report
`user exit status=0x0000000000000000`, then the kernel prints `all tasks exited`
and idles. Unknown call numbers return -1 in `a0`.

Unexpected traps print `fatal trap` with `scause` (trap cause), `sepc`
(instruction address), and `stval` (additional information, when provided).
To exit QEMU, press `Ctrl+a`, then `x`.

Check Rust formatting with `rustfmt` from the stable toolchain.

```sh
cargo +stable fmt --manifest-path native/kernel/Cargo.toml --check
rustfmt +stable --edition 2024 --check userspace/hello/main.rs
```
