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
version from `native/kernel/Cargo.toml`. It then prints `boot frame=0x...` with
the physical address of the first zeroed frame and idles. The boot allocator
has a fixed pool of 16 frames of 4 KiB each. To exit QEMU, press `Ctrl+a`, then
`x`.

A supervisor trap prints one `fatal trap` line over UART before the CPU idles.
The hexadecimal fields are `scause` (trap cause), `sepc` (instruction address),
and `stval` (additional information for the trap, when provided).

Check Rust formatting with `rustfmt` from the stable toolchain.

```sh
cargo +stable fmt --manifest-path native/kernel/Cargo.toml --check
```
