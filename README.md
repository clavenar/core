# Clavenar Core

Clavenar Core is the engineering repository for the native operating system,
Linux-hosted runtime, and virtual machine integration. The first implementation
targets RV64 on QEMU `virt`.

- `native/kernel/` and `native/linker/` hold kernel source and linker scripts.
- `crates/` holds shared Rust types and ABI definitions. [`clv-abi`](crates/clv-abi/)
  defines the syscall numbers used by the kernel and user programs.
- `userspace/` holds the native programs. They share RV64 syscall wrappers in
  [libclv](userspace/libclv/).
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

Only the boot shell starts with a RamFS working directory (`/`). The initial
`hello` task and children launched without a grant (`run hello`, `run echo`,
or `ask`) start with an empty RamFS handle table. Children access RamFS through
explicit file or directory grants. In a child, `open`, `mkdir`, `unlink`,
`rmdir`, `chdir`, and `getcwd` return -1.

The shell stays interactive after `run`. Every launch starts with a cleared
stack and fresh registers and timer state. Exit or fault frees the slot.

`run hello <directory>` grants the child a read-only directory handle. The
child starts with that single handle and uses `open_at` to read and print
`hello.txt` from the granted directory.

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

`ask hello hello from shell` launches `hello` with an `Echo` request and prints the
echoed reply `hello from shell`. `ask echo hello from shell` sends the same
request to `echo`. The request is available to the child when it starts.

`askat hello /notes todo.txt` grants access to `/notes` and sends a `ReadFile`
request for `todo.txt`. The shell prints up to 63 bytes of file data, reports an
empty file, or reports a failed read.

```text
mkdir /notes
write /notes/todo.txt Hello from notes
askat hello /notes todo.txt
wait hello
```

Put `--echo` or `--read` before the program name to select a request kind
explicitly. In a fresh QEMU shell, the first two launches below grant the
same directory and send the same bytes. The first prints `hello.txt` and
the second prints `Hello from RamFS`. The third prints
`cannot read requested file` because the child has no directory handle.
The three waits report exit statuses 0, 0 and 1.

```text
askat --echo hello / hello.txt
wait hello
askat hello / hello.txt
wait hello
ask --read hello hello.txt
wait hello
```

`askfile hello <path>` grants `hello` read access to one file. The shell opens
the file, launches the child and closes its own handle. The child reads directly
from its handle and replies with up to 63 bytes. A directory is rejected at launch.

```text
mkdir /notes
write /notes/report approved
askfile hello /notes/report
wait hello
```

The shell prints `approved` and reports that `hello` exited with status 0.

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
Directories and writable files live in RAM until reboot. In the shell,
absolute paths start at `/` and relative paths start at its current directory.
Within a path, `.` keeps the directory reached so far and `..` moves to its
parent, staying at `/` when already there. Creating a file or directory
requires an existing parent. Each task can hold four live handles, each with
an independent cursor. Every open or grant receives an opaque ID unique within
the current boot. The kernel accepts that ID from the owning task until the
handle closes or the task stops.

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
`hello` and selector 1 chooses `echo`. The child starts with no RamFS
handles or working directory. The call returns a fresh, nonzero child handle
in `a0` when the task is ready to run and -1 if the selector is unknown or
the task is already running. All spawn calls use the same child handle sequence
for the current boot. Child handles belong to the shell and have their own
namespace.

Call 16 (`wait`) takes the child handle in `a0`. On success, `a0` contains
the result type and `a1` contains the full exit status or fault cause. Result
type 0 means exit and 1 means fault. The call returns -1 in `a0` for an
invalid handle, a caller other than the shell, or an already collected status.

Call 18 (`spawn_at`) takes a program selector in `a0` and a directory handle
from the caller in `a1`. It returns a child handle on success and -1 on error.
The child receives its own directory handle in `a1` and a grant kind of 1 in
`a2`. Launches without a grant set `a2` to 0.

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
The child starts with the same empty RamFS state as `spawn`. This call
assigns request kind `Echo`. Exit or fault discards any unread request.

Call 22 (`spawn_at_with_message`) takes a program selector in `a0`, a
directory handle in `a1`, a user stack request address in `a2`, and a length
from 1 to 64 bytes in `a3`. It validates the grant and copies the request
before scheduling the child. It returns a child handle on success or -1 on
error.

This call assigns request kind `ReadFile` to `hello` and `Echo` to `echo`,
matching the original behavior of each program.

Call 23 (`spawn_with_file`) takes a program selector in `a0` and an open
regular file handle owned by the shell in `a1`. It returns a child handle on
success or -1 on error. The child starts with no working directory. Its sole
RamFS handle arrives in `a1`, with grant kind 2 in `a2`. The handle is
read-only even if the parent opened the file for writing. Its cursor starts
at byte zero. Closing the parent's handle or unlinking the name leaves the
child's handle valid. Writes, directory-relative opens through the file
handle, and ordinary path lookups by this child return -1.

Call 24 (`spawn_request`) takes a program selector in `a0`, request kind in
`a1`, user stack payload address in `a2`, byte count in `a3`, and an optional
directory handle in `a4`. Use `NO_DIRECTORY_GRANT` (`usize::MAX`) for a
launch without a grant. Handle 0 is valid. The kernel accepts `Echo` (1) and
`ReadFile` (2) with 1 to 64 payload bytes. It copies the payload and records
the caller's task ID before the child runs. Success returns a fresh child
handle. Invalid arguments or a busy child slot return -1.

Call 25 (`request_recv`) lets a launched child receive its request into the
user stack buffer in `a0` with nonzero capacity in `a1`. It returns the
byte count in `a0`, the sender's task ID in `a1`, and the request kind in
`a2`. An empty mailbox returns zero in all three registers. A bad buffer,
insufficient capacity, or a caller without a child launch returns -1 in
`a0`. A failed copy preserves the pending request. Calls 20 and 25 consume
the same request once. The shell collects replies with call 20.

`hello` echoes an `Echo` payload verbatim. For `ReadFile`, it opens the
payload path through its directory grant using `open_at`. A successful file
reply starts with status byte 0 followed by up to 63 bytes of file data.
A missing directory grant or failed read produces the single status byte 1.
`echo` accepts `Echo` and returns the file error for `ReadFile`. The child exits
with status 0 after a successful reply and status 1 if the request or send fails.

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
  userspace/libclv/lib.rs userspace/echo/main.rs userspace/hello/main.rs userspace/shell/main.rs
```
