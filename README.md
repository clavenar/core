# Clavenar Core

Clavenar Core is the engineering repository for the native operating system,
Linux-hosted runtime, and virtual machine integration. The first implementation
targets RV64 on QEMU `virt`.

- `native/kernel/` and `native/linker/` hold kernel source and linker scripts.
- `crates/` holds shared Rust types and ABI definitions. [`clv-abi`](crates/clv-abi/)
  defines the syscall numbers used by the kernel and user programs.
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

The build links [hello](userspace/hello/), [the shell](userspace/shell/), and
[echo](userspace/echo/) as separate RV64 ELFs and embeds them in the kernel.
The loader accepts static, little-endian ELF64 executables with one
read/execute segment of at most 8 KiB at `0x00400000`. The segment's memory
size determines whether it uses one or two pages. Boot reports each program's
validated entry address.
If loading fails, it prints `User ELF load failed` with the program name and
idles.

The kernel activates paging, prints `Sv39 paging enabled`, and starts hello
as task 1 and the shell as task 2. Task 3 starts when the shell launches
`echo`. Each receives its task ID in `a0` and runs with its own Sv39 root
and read/execute image. Each task has a 4 KiB read/write, non-executable stack
at `0x00402000`. The kernel and UART mappings are shared in supervisor mode.
The kernel, supervisor stack, and 21-frame boot pool fit within one aligned
2 MiB physical region.

OpenSBI timer interrupts drive round-robin scheduling. A `timer switch` line
shows which task was interrupted and which task will run next. The kernel
starts a fresh quantum when the timer fires or a stopped task is replaced.

At the shell prompt, `run hello` and `run echo` start the selected program
in its own task slot with an empty handle table and working directory `/`.
The shell stays interactive while either child runs. Every launch starts
with a cleared stack and fresh registers and timer state. Exit or fault
frees the slot.

`run hello <directory>` grants the child a read-only directory handle. The
child starts with that single handle and resolves paths through `open_at`.
Ordinary path calls (`open`, `mkdir`, `unlink`, `rmdir`, `chdir`, and `getcwd`)
return -1 for a scoped launch. The program prints `hello.txt` from the
granted directory.

```text
mkdir /notes
write /notes/hello.txt Hello from notes
run hello /notes
recv hello
wait hello
```

`run hello` sends `hello ready` to the shell, and `run echo` sends
`echo ready`. The shell can collect each reply and status independently.

```text
run hello
run echo
recv echo
wait echo
recv hello
wait hello
```

`ask hello hello from shell` launches `hello` with a request and prints the
echoed reply `hello from shell`. `ask echo hello from shell` sends the same
request to `echo`. The request is available to the child when it starts.

`askat hello /notes todo.txt` grants access to `/notes` and asks `hello` to
read `todo.txt` through that directory handle. The shell prints the first
63 bytes, reports an empty file, or reports a failed read.

```text
mkdir /notes
write /notes/todo.txt Hello from notes
askat hello /notes todo.txt
wait hello
```

The shell records the latest successful launch handle for each program.
`wait hello` and `wait echo` collect the selected child's exit status or
fault cause once, waiting if the child is still running. `recv hello` and
`recv echo` wait for the selected child's reply or completion. A queued
reply remains available after exit or fault. Relaunching a program
invalidates its previous handle and discards the results for that handle.
A failed launch preserves the previous handle and its results.

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
`rm todo.txt` removes it. Use `cd ..` to go up one directory and `rmdir notes`
to remove the empty directory. Use `exit` to stop the shell.

RamFS starts with read-only `/hello.txt` and `/about.txt` and holds up to four
directories besides the root and four writable files of 256 bytes each.
Directories and writable files live in RAM until reboot. For ordinary path
lookups, absolute paths start at `/` and relative paths start at the task's
current directory, initially `/`. Within a path, `.` keeps the directory
reached so far and `..` moves to its parent, staying at `/` when already
there. Creating a file or directory requires an existing parent. Each task
can hold four live handles, each with an independent cursor. Every open or
directory grant receives an opaque ID unique within the current boot. The
kernel accepts that ID from the owning task until the handle closes or the
task stops.

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
| 14 `rmdir` | `a0` path address, `a1` path length up to 64 bytes | 0 |
| 17 `open_at` | `a0` directory handle, `a1` path address, `a2` path length up to 64 bytes | Read-only handle |

Open mode 0 opens a path for reading. Mode 1 opens a writable file for reading
and writing, creating it if absent and truncating it to zero length otherwise.
File calls return -1 on error. Paths and data buffers must reside in the
calling task's stack. Writes validate and copy the entire input before
changing bytes or the cursor. Reads and directory listings advance their
cursor only after a successful copy. Directory entries contain the name bytes
without a trailing newline or NUL.

`open_at` starts from a directory handle in the calling task's handle table
and opens an existing file or directory for reading. It rejects absolute
paths and any `..` step above the starting directory.

`chdir` leaves the current directory unchanged on error. `getcwd` writes the
full absolute path, up to 260 bytes, without a trailing NUL. If the destination
buffer is too small or outside the user stack, it returns -1 and writes nothing.

Unlink removes a writable file's name immediately. Open handles retain access
to that file, and its slot becomes reusable once the last handle closes.
Recreating the path while an old handle is open uses a separate slot.
Task exit and user faults close the task's handles.

`rmdir` frees a directory slot as soon as it removes the name. Remove all
linked files and subdirectories first. Any live task's working directory or
open directory handle keeps that directory in use. Attempts to remove `/` or
a path ending in `.` or `..` return -1.

Call 1 takes an exit status in `a0` and removes the calling task from the
schedule. A task that exits successfully reports
`user exit status=0x0000000000000000`. When the last task stops, the kernel
prints `all tasks exited` and idles. Unknown call numbers return -1 in `a0`.

Call 15 (`spawn`) takes a program selector in `a0`. Selector 0 chooses
`hello` and selector 1 chooses `echo`.
It returns a fresh, nonzero child handle in `a0` when the task is ready to run
and -1 if the selector is unknown or the task is already running. All four
spawn calls use the same child handle sequence for the current boot. Child
handles belong to the shell and have their own namespace.

Call 16 (`wait`) takes the child handle in `a0`. On success, `a0` contains
the result type and `a1` contains the full exit status or fault cause. Result
type 0 means exit and 1 means fault. The call returns -1 in `a0` for an
invalid handle, a caller other than the shell, or an already collected status.

Call 18 (`spawn_at`) takes a program selector in `a0` and a directory handle
from the caller in `a1`. It returns a child handle on success and -1 on error.
The child receives its own directory handle in `a1` and a grant flag of 1 in
`a2`. Ordinary launches start with `a2` set to 0.

Call 19 (`message_send`) takes a user stack address in `a0` and a length from
1 to 64 bytes in `a1`. A shell-launched child can queue one message for its
live parent. The call returns the byte count on success and -1 on error. A
send to a full mailbox preserves its queued message.

Call 20 (`message_recv`) takes a user stack destination in `a0` and its nonzero
capacity in `a1`. The shell passes a child handle in `a2` to receive a reply.
A launched child passes 0 in `a2` to receive its request. The result gives
the byte count in `a0` and the sender's task ID in `a1`. An empty reply mailbox
blocks while the selected child runs. Other empty reads return 0 in both
registers. An invalid handle or an invalid or undersized buffer returns -1
and preserves any queued message.

Call 21 (`spawn_with_message`) takes a program selector in `a0`, a user stack
address in `a1`, and a length from 1 to 64 bytes in `a2`. It copies the request
before making the child runnable and returns a child handle on success or
-1 on error.
The child consumes the request with `message_recv`. Exit or fault discards
any unread request.

Call 22 (`spawn_at_with_message`) takes a program selector in `a0`, a
directory handle in `a1`, a user stack request address in `a2`, and a length
from 1 to 64 bytes in `a3`. It validates the grant and copies the request
before scheduling the child. It returns a child handle on success or -1 on
error.

When launched this way, `hello` resolves the requested path with `open_at`.
A reply with status byte 0 carries zero to 63 file bytes. Status byte 1 reports a
failed read and carries just that byte. The child exits with status 0 after
sending a successful reply and status 1 on failure.

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
rustfmt +stable --edition 2024 --check \
  userspace/echo/main.rs userspace/hello/main.rs userspace/shell/main.rs
```
