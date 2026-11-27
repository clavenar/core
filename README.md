# Clavenar Core

Clavenar is a Rust operating system for running programs and agents with
explicit authority and resource limits. The native RV64 prototype boots on
QEMU `virt` with OpenSBI. It runs isolated ELF programs, a shell and a
volatile filesystem. The goal owner can grant a file, set a deadline, limit
reads and replies, and review a staged rename before making it visible.

## Build and run the RV64 kernel

With stable Rust installed via `rustup`, add the bare-metal target. On Debian 13,
`qemu-system-riscv` provides the emulator and pulls in OpenSBI.

```sh
rustup target add --toolchain stable riscv64gc-unknown-none-elf
sudo apt-get install --no-install-recommends qemu-system-riscv
```

Run these commands from the `core` repository root.

```sh
cargo +stable rustc --locked --manifest-path native/kernel/Cargo.toml \
  --target riscv64gc-unknown-none-elf --target-dir target --release \
  -- -C "link-arg=-T${PWD}/native/linker/riscv64.ld" -C link-arg=--no-relax

qemu-system-riscv64 -machine virt -smp 1 -m 128M -bios default -nographic \
  -kernel target/riscv64gc-unknown-none-elf/release/clavenar-kernel
```

After OpenSBI, look for `Clavenar kernel booted` with the package version,
ELF entry addresses for `hello`, `shell`, `echo` and `grep`, then
`Sv39 paging enabled` and the green `clsh>` prompt. Type `help` for the
command reference. Press `Ctrl+a`, then `x`, to quit QEMU.

## Limit file reads to a directory

At a fresh `clsh>` prompt, give `hello` a file that the shell can read but
that falls outside the goal's permitted directory.

```text
cat /hello.txt
mkdir /work
goal create --read-under /work
goal grant-file /hello.txt
agent run hello
wait hello
recv hello
goal audit --reads
goal close
```

The shell prints `Hello from RamFS` and accepts the grant. When the agent
tries to read it, `hello` exits with status 1 and `recv hello` prints
`cannot read requested file`. The audit identifies the child and reports
`denied=outside-read-scope` and `read_bytes=0`.
[Compare an allowed read with a denied read](docs/qemu-walkthroughs.md#limit-file-reads-to-a-directory)

## Documentation

- [QEMU walkthroughs](docs/qemu-walkthroughs.md) give commands and expected results for child programs, goal budgets, read-only observers, delegated resumes, tool delegation, auditing and reviewed renames.
- [Native syscall ABI](docs/native-syscall-abi.md) documents registers, wire formats and error handling.
- [Roadmap](ROADMAP.md) links the current prototype to the native release criteria and the file-organizer demonstration.
- [Repository layout](https://github.com/clavenar/specifications/blob/main/engineering/repository-layout.md) maps the native source and the planned Hosted and Virtual components.

Kernel sources live in [native/kernel](native/kernel/) and the linker script
in [native/linker](native/linker/). The kernel and [user programs](userspace/)
share [ABI definitions](crates/clv-abi/) and the programs call the kernel
through [libclv](userspace/libclv/).

## Formatting

Check Rust formatting with `rustfmt` from the stable toolchain.

```sh
cargo +stable fmt --manifest-path native/kernel/Cargo.toml --check
cargo +stable fmt --manifest-path crates/clv-abi/Cargo.toml --check
rustfmt +stable --edition 2024 --check \
  userspace/libclv/lib.rs userspace/echo/main.rs userspace/grep/main.rs \
  userspace/hello/main.rs userspace/shell/main.rs
```
