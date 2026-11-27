# Clavenar Core

Clavenar is a Rust operating system for running programs and agents with
explicit authority and resource limits. The native RV64 prototype boots on
QEMU `virt` with OpenSBI. It runs isolated ELF programs, a shell and RamFS.
The goal owner can grant a file, set a deadline and limit reads and replies.
At goal creation the owner can require a separate supervisor to review a
staged rename. The owner decides whether to publish it.
A durable goal can save one or two root-level files to a dedicated VirtIO
block journal and restore them at boot.
[Try a required supervisor review](docs/qemu-walkthroughs.md#require-a-supervisor-review-before-commit) · [Persist a reviewed rename](docs/qemu-walkthroughs.md#persist-a-reviewed-rename) · [Persist a two-file batch](docs/qemu-walkthroughs.md#persist-two-renames-in-one-commit)

A checkpoint can save the result of the local `find-greeting@2` search
and resume the reply step after reboot. A completed reply stays readable
on the checkpoint image.
The pinned host MCP search can recover its broker receipt using the same
checkpoint image and SQLite journal across boots.
[Resume a local search checkpoint](docs/qemu-walkthroughs.md#resume-a-local-search-checkpoint) · [Recover a host MCP search checkpoint](docs/qemu-walkthroughs.md#recover-a-host-mcp-search-checkpoint)

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

qemu-system-riscv64 -machine virt -smp 1 -m 128M -bios default -nographic -nic none \
  -kernel target/riscv64gc-unknown-none-elf/release/clavenar-kernel
```

After OpenSBI, look for `Clavenar kernel booted` with the package version,
ELF entry addresses for `hello`, `shell`, `echo` and `grep`, then
`Sv39 paging enabled` and the green `clsh>` prompt. Type `help` for the
command reference. Press `Ctrl+a`, then `x`, to quit QEMU.

[Boot with a read-only VirtIO block image](docs/qemu-walkthroughs.md#probe-a-virtio-block-device) · [Recover a marker after reboot](docs/qemu-walkthroughs.md#recover-a-virtio-block-marker-after-reboot) · [Recover a previous journal record](docs/qemu-walkthroughs.md#recover-a-previous-journal-record)

[Verify the VirtIO-net Ethernet exchange](docs/qemu-walkthroughs.md#exchange-an-ethernet-frame-over-virtio-net)

## Run the QEMU host broker

After building the kernel, use Python 3.10 or later with its standard
`sqlite3` module. Run this from the `core` repository root.

```sh
python3 platforms/qemu-riscv64-virt/host_bridge.py --budget-units 2
```

The broker gives this VM a two-unit mock allowance and stores its
reservations and replies in `target/host-bridge.sqlite3`.
[Run two goals against the host allowance](docs/qemu-walkthroughs.md#run-a-host-backed-mock-plan)

### Use a local Ollama model

Install Ollama on the Linux host. In a host terminal, start its loopback
server with cloud features disabled. The broker checks this through
`/api/status`. This setup was verified with Ollama 0.40.18.
[Ollama Linux setup](https://docs.ollama.com/linux) · [Local-only settings](https://docs.ollama.com/faq#how-do-i-disable-ollamas-cloud-features)

```sh
OLLAMA_NO_CLOUD=1 ollama serve
```

In a second host terminal in `core`, download the model and start the broker.

```sh
ollama pull qwen2.5:0.5b
python3 platforms/qemu-riscv64-virt/host_bridge.py \
  --mode ollama --ollama-model qwen2.5:0.5b --budget-tokens 2080
```

For a loopback server on another port, pass `--ollama-port` to the broker.
At `clsh>`, use revision 6 to request the model decision.
[Run a model-backed plan](docs/qemu-walkthroughs.md#run-a-model-backed-plan)

### Call a read-only MCP server on the host

Use the broker's `--mode mcp` with a stateless JSON MCP server on
`127.0.0.1`. The broker uses the port given by `--mcp-port` and defaults
to one call per VM. Revision 7 requires the goal's explicit
`--allow-tool host.text.search@1` grant. The walkthrough starts a fixture
server with the MCP Python SDK and shows the guest and journal commands.
[Search a host MCP fixture](docs/qemu-walkthroughs.md#search-a-host-mcp-fixture)

### Run Ollama and both MCP searches in one goal

Start local-only Ollama and the host MCP fixture, then use `--mode combined`
and `find-greeting@8`. Grant both exact tool revisions and a local file.
The broker applies independent model token and host tool call allowances
and records both requests under the same goal and agent launch.
[Run the combined walkthrough](docs/qemu-walkthroughs.md#run-ollama-and-both-mcp-searches-in-one-goal)

### Replay a recorded goal offline

After recording a combined run, use `--mode replay` with the journal,
recorded session and goal. A saved model decision can skip the searches or
run the local MCP search under the guest's grants and budgets. Replay then
serves the recorded host MCP result or budget refusal for a search.
[Replay a combined goal offline](docs/qemu-walkthroughs.md#replay-a-combined-goal-offline)

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
