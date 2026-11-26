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

The build links the shell and each registered program as separate RV64 ELFs
and embeds them in the kernel. To add a boot program, put its entry point in
`userspace/<name>/main.rs` and append its name to the program registry. The
build, shell and kernel use that registry for names and stable launch
selectors. A versioned file tool maps an exact tool ID to one of those
programs. [Program registry](crates/clv-abi/src/program.rs) · [File tool registry](crates/clv-abi/src/tool.rs)
The loader accepts static, little-endian ELF64 executables with one
read/execute segment of at most 20 KiB at `0x00400000`. The loader allocates
one to five pages based on the segment's memory size (`p_memsz`) and zero-fills
any bytes beyond the file data. Boot reports each program's validated entry
address. If loading fails, it prints `User ELF load failed` with the program
name and idles.

The kernel activates paging, prints `Sv39 paging enabled`, and starts hello
as task 1 and the shell as task 2. Tasks 3 and 4 start on the first launch of
`echo` and `grep`, respectively. Each receives its task ID in `a0` and runs
with its own Sv39 root and read/execute image. Each task has a 4 KiB
read/write, non-executable stack. It starts at `0x00402000` for one- and
two-page images and immediately after the image for larger programs. The
initial stack pointer is at the top of that page. Nonempty syscall buffers
must fit within the calling task's stack. The kernel and UART mappings are
shared in supervisor mode. The kernel, 48 KiB supervisor stack, and 39-frame
boot pool fit within one aligned 2 MiB physical region.

OpenSBI timer interrupts drive round-robin scheduling. A `timer switch` line
shows which task was interrupted and which task will run next. The kernel
starts a fresh quantum when the timer fires or a stopped task is replaced.

Only the boot shell starts with a RamFS working directory (`/`). The initial
`hello` task and children launched without a grant (`run hello`, `run echo`,
or `ask`) start with an empty RamFS handle table. Children access RamFS through
explicit file or directory grants. In a child, `open`, `mkdir`, `unlink`,
`rmdir`, `chdir`, and `getcwd` return -1.

The shell stays interactive after `run`. Every launch starts with a cleared
stack and fresh registers and timer state. A parent can reuse its child's
program slot after exit, fault or goal cancellation.

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

`event` waits for the next child completion. It prints the child's name and
`exited`, `faulted`, or `cancelled`. Try the following after the boot greeting.

```text
run hello
run echo
event
event
recv echo
wait hello
wait echo
event
```

The first two events report both children in completion order. The echo reply
is still available, both waits report status 0, and the last `event` prints
`no child event`.

The shell can own one goal at a time. By default a goal can launch one child.
`goal create --children 2` or `--children 3` allows further launches while
at least one member is live. Each successful launch uses one place for the
lifetime of the goal, including the agent itself. The goal finishes when its
last live member stops.
`goal create` prints the new goal handle. `goal run hello` or `goal run echo`
prints the ordinary child's launch handle. `goal status` reports `Ready`,
`Running`, `Completed`, `Failed`, or `Cancelled`. The kernel keeps each
member's original launch handle and outcome until the shell closes the goal
or the shell itself stops.

```text
goal create
goal run echo
event
wait echo
ask --read echo hello.txt
wait echo
goal status
goal close
```

The first `wait echo` reports exit status 0. The second launch reuses the
same program slot and exits with status 1 because `echo` rejects file-read
requests. `goal status` still prints `Completed`, the handle from `goal run`
and exit status 0. A running goal returns `goal busy` on close. Closing a
ready or finished goal makes the slot available for a fresh goal.

`agent run <program>` creates an agent in the current goal and prints two
handles. Each goal can admit at most one agent. The agent handle identifies
the record read by `agent status`.
The child handle identifies replies, completion events and the wait result.
The shell remembers both handles for you. Try this at the QEMU prompt.

```text
goal create
agent run echo
wait echo
ask --read echo hello.txt
wait echo
agent status
goal close
```

The two waits report exit statuses 0 and 1. `agent status` prints the agent
and goal handles, followed by `Completed` with the original child handle and
exit status 0. Closing the goal releases the agent. Plain `run` and `goal run`
launch ordinary children.

`agent suspend` pauses a live agent. `agent resume` continues its child from
the same instruction with its saved registers and stack. To see the paused
state with the stock `echo`, start it suspended at its entry point.

```text
goal create
agent run --suspended echo
agent status
event
agent resume
wait echo
agent status
goal close
```

On a fresh boot, the first status prints `Suspended child=<handle>` and
`event` immediately prints `no child event`. After resume, the wait reports
exit status 0 and the final status prints `Completed` with the same child
handle. While an agent is suspended, its goal is `Running`, `goal close`
reports `goal busy`, and `goal cancel` stops the suspended child. If the child
finishes before you type `agent suspend`, the shell prints `agent already finished`.

Use `goal create --reply-bytes N` to limit the total bytes its children can send
in replies. Both `goal run` and `agent run` inherit the available balance.
Pass `--reply-bytes N` to reserve any amount up to that balance. The shell
accepts unsigned decimal counts, including zero. A plain `goal create` has
an unlimited budget.

```text
goal create --reply-bytes 16
agent run --reply-bytes 17 echo
agent run --suspended --reply-bytes 12 echo
goal budget
agent resume
wait echo
recv echo
goal budget
goal close
```

The 17-byte request fails and leaves the goal ready. The first budget read
prints `available=4 reserved=12 spent=0`. The message `echo ready` costs 10
bytes when queued. On exit the two unused bytes return to the goal, so the
second read prints `available=6 reserved=0 spent=10`. To see a send denied,
create another 16-byte goal and run `agent run --reply-bytes 9 echo`.
`wait echo` reports exit 1, `recv echo` prints `no message` and `goal budget`
prints `available=16 reserved=0 spent=0`.

To share 24 reply bytes between two children, start the first suspended so
its reservation stays live while the shell launches the second.

```text
goal create --children 2 --reply-bytes 24
agent run --suspended --reply-bytes 12 hello
goal run --reply-bytes 13 echo
goal budget
goal run --reply-bytes 12 echo
wait echo
goal budget
agent resume
wait hello
goal budget
goal close
```

| Checkpoint | Output from `goal budget` |
| --- | --- |
| The kernel rejects the 13-byte request | `available=12 reserved=12 spent=0` |
| `echo` sends `echo ready` and exits | `available=2 reserved=12 spent=10` |
| `hello` resumes, sends `hello ready` and exits | `available=3 reserved=0 spent=21` |

Use `agent delegate --reply-bytes N <program>` to start a helper with
bytes from the current agent's remaining reservation. The shell can do this
while the agent is running or suspended. Each helper uses one launch slot
and starts with an empty RamFS handle table by default.

```text
goal create --children 2 --reply-bytes 64
agent run --suspended --reply-bytes 24 hello
agent delegate --reply-bytes 25 echo
goal budget
agent delegate --reply-bytes 12 echo
wait echo
goal budget
agent resume
wait hello
goal budget
goal close
```

| Checkpoint | Output from `goal budget` |
| --- | --- |
| The 25-byte request exceeds the agent's 24 bytes | `available=40 reserved=24 spent=0` |
| `echo` spends 10 bytes and returns 2 to the suspended agent | `available=40 reserved=14 spent=10` |
| `hello` spends 11 bytes and returns its last 3 to the goal | `available=43 reserved=0 spent=21` |

`goal grant-file <path>` attaches one read-only file to a ready goal. Both
`goal run` and `agent run` pass that file to their child. Try this in QEMU.

```text
write /note old
goal create --reply-bytes 8
goal grant-file /note
rm /note
write /note new
agent run hello
wait hello
recv hello
goal budget
cat /note
goal close
```

`wait hello` reports exit 0. The reply contains `old` and the two-byte line
ending stored by `write`, so the budget prints `available=3 reserved=0 spent=5`.
`cat /note` prints `new` because the shell recreated the path after the grant.
If you try to grant a directory or attach a second file, the shell prints
`cannot grant file`. The goal keeps its existing grant and balance.

To let the agent pass that file to a helper, attach it with
`goal grant-file --delegable` and request it with `agent delegate --with-file`.
The helper receives its own read-only handle at offset zero.

```text
write /note old
goal create --children 2 --reply-bytes 24
goal grant-file --delegable /note
agent run --suspended --reply-bytes 16 echo
rm /note
write /note new
agent delegate --with-file --reply-bytes 5 hello
wait hello
recv hello
cat /note
agent resume
wait echo
goal budget
goal close
```

`recv hello` prints `old` from the original file object. `cat /note` prints
`new` from the recreated file. Both waits report exit 0. The helper spends
5 reply bytes including the line ending, and `echo ready` costs 10, giving
`available=9 reserved=0 spent=15`.

Use `goal grant-file --delegate-to <program> <path>` to choose which helper
may receive the agent's file. The direct agent can read the file itself. The
kernel checks the recipient when the agent requests a helper with `--with-file`.

```text
write /note old
goal create --children 2 --reply-bytes 24
goal grant-file --delegate-to echo /note
agent run --suspended --reply-bytes 16 echo
agent delegate --with-file --reply-bytes 5 hello
goal audit
goal budget
goal cancel
goal close
```

The audit shows `program=hello bytes=5 file=yes denied=file-recipient-not-allowed`.
The budget reads `available=8 reserved=16 spent=0`, and the helper slot is free.
Repeat from `goal create` with `--delegate-to hello`. After `agent delegate`,
run `wait hello` and `recv hello`. The reply is `old`, the audit shows
`accepted child=<handle>`, and the budget reads `available=8 reserved=11 spent=5`.

Use `goal audit` to see why the kernel accepted or denied each helper launch.
This example uses the boot file and a suspended agent so the three decisions
can be reproduced at the shell prompt.

```text
goal create --children 2 --reply-bytes 24
goal grant-file /hello.txt
agent run --suspended --reply-bytes 16 echo
agent delegate --with-file --reply-bytes 5 hello
agent delegate --reply-bytes 17 hello
agent delegate --reply-bytes 11 hello
wait hello
goal audit
goal budget
goal cancel
goal close
```

The audit shows `denied=no-delegable-file` for the ordinary file grant,
`denied=insufficient-reply-bytes` for the 17-byte request, and
`accepted child=<handle>` for the 11-byte request. The first two attempts
leave the agent's 16 bytes and the helper slot available. After `wait hello`,
the balance is `available=8 reserved=5 spent=11`.

The goal retains its four most recent decisions in chronological order.
`overwritten` counts older decisions that have been discarded. You can read
the same entries again after `wait`, `event`, or `goal cancel`, until you
close the goal. Each new goal starts with an empty audit.

`goal audit --replies` shows whether each child's reply fit its own remaining
allowance. For example, the `echo` helper tries to send the 10 bytes in
`echo ready`.

```text
goal create --children 2 --reply-bytes 24
agent run --suspended --reply-bytes 16 hello
agent delegate --reply-bytes 9 echo
wait echo
goal audit
goal audit --replies
goal budget
goal cancel
goal close
```

Repeat with a 10-byte allowance in the `agent delegate` command. `goal audit`
shows an accepted launch in both runs. The reply audit identifies that same
child handle and shows the decision at the time of the send.

| Helper allowance | Exit status | Reply audit after the child handle | Goal budget after the wait |
| --- | --- | --- | --- |
| 9 | 1 | `bytes=10 remaining_before=9 denied=insufficient-reply-bytes` | `available=8 reserved=16 spent=0` |
| 10 | 0 | `bytes=10 remaining_before=10 sent` | `available=8 reserved=6 spent=10` |

Each goal keeps the last four reply-budget decisions independently of its
delegation history. Both histories can be read repeatedly until `goal close`.

You can cancel a ready goal directly at the shell prompt.

```text
goal create
goal cancel
goal status
event
goal close
```

The status is `Cancelled`. On a fresh boot, `event` prints `no child event`
because this goal never launched a child. Cancelling a running goal stops
all its live members before the command returns. Its status is then
`Cancelled child=<handle>`. For each child it launched, the shell can
collect a completion event, a wait result and any reply already sent through
`recv <name>`. The stock `echo`
exits quickly. If it finished first, `goal cancel` prints `goal already finished`.

`ask hello hello from shell` launches `hello` with an `Echo` request and prints the
echoed reply `hello from shell`. `ask echo hello from shell` sends the same
request to `echo`. The request is available to the child when it starts.

`askat hello /notes todo.txt` grants access to `/notes` and sends a `ReadFile`
request for `todo.txt`. The shell prints up to 64 bytes of file data, reports an
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
`read denied: no directory grant` because the child has no directory handle.
The fourth grants the root directory but asks for a missing file, so it prints
`cannot read requested file`. The four waits report exit statuses 0, 0, 1 and 1.

```text
askat --echo hello / hello.txt
wait hello
askat hello / hello.txt
wait hello
ask --read hello hello.txt
wait hello
askat hello / missing.txt
wait hello
```

`askfile hello <path>` grants `hello` read access to one file. The shell opens
the file, launches the child and closes its own handle. The child reads directly
from its handle and replies with up to 64 bytes. A directory is rejected at launch.

```text
mkdir /notes
write /notes/report approved
askfile hello /notes/report
wait hello
```

The shell prints `approved` and reports that `hello` exited with status 0.

`askfile --stream hello <path>` grants the same read-only file and a write
end of a 32-byte pipe. The shell reads until EOF and prints the file bytes.
In QEMU, the first two commands below print all 77 bytes of `/about.txt`,
including its line ending. The wait reports exit status 0.

```text
cat /about.txt
askfile --stream hello /about.txt
wait hello
```

An empty file produces no bytes. The shell returns to the prompt at EOF,
and `wait hello` collects the child's exit status or fault separately.
While the shell drains a pipe, the kernel pauses routine switch and exit
traces on the shared UART so they do not interrupt the file contents.
Fault diagnostics still appear immediately.

The shell records the latest successful launch handle for each program.
`wait <program>` collects the selected child's exit status or fault cause
once, waiting if the child is still running. `recv <program>` waits for the
selected child's reply or completion. A queued reply remains available
after exit or fault. Relaunching a program
invalidates its previous handle and discards the results for that handle.
A failed launch preserves the previous handle and its results.

### Search a granted file with grep

`grep -F <pattern> <path>` runs the native search program on one read-only
file. It selects whole lines containing a case-sensitive literal. Single or
double quotes can enclose a pattern with spaces. An empty quoted pattern
selects every line. The shell passes the opened file and the pattern together.
A delegated search receives the same arguments using the agent's file grant.
The interactive shell accepts up to 128 bytes for the entire command line.
The launch argument can hold up to 64 bytes.

```text
write /build.log ERROR disk full
grep -F ERROR /build.log
goal create --children 2 --reply-bytes 32
goal grant-file --delegate-to grep /build.log
agent run --suspended --reply-bytes 32 echo
agent delegate --with-file --reply-bytes 17 grep -F ERROR
wait grep
recv grep
goal audit --replies
goal cancel
goal close
```

Both searches print `ERROR disk full`. The stored CRLF brings the reply to
17 bytes, so the audit shows `bytes=17 remaining_before=17 sent`. Repeat from
`goal create` with a helper allowance of 16. The helper exits with status 2,
`recv grep` prints `no message`, and the audit records
`bytes=17 remaining_before=16 denied=insufficient-reply-bytes`.

The search reads through the file and returns all selected bytes in one reply,
preserving line endings and a final unterminated line. Exit status 0 means a
match, 1 means no matches, and 2 means an input or output error. If the
matching lines exceed 64 bytes in total, the shell prints
`grep result exceeds 64 bytes`. The error reply contains zero file bytes.
An empty match result prints nothing. `wait grep` can also collect the exit
status after a direct search.

### Restrict delegated helper launches

Use `goal create --allow-helper <program>` to choose the only program an
agent may launch as a helper for that goal. The choice is fixed at creation
and applies whether or not the helper requests a file. The goal owner can
still launch any eligible program directly with `goal run` or `agent run`.
A delegated file must also permit the chosen recipient.

At the QEMU prompt, give a suspended `echo` agent 18 reply bytes and allow
`grep` as its helper.

```text
goal create --children 2 --reply-bytes 18 --allow-helper grep
goal grant-file --delegable /hello.txt
agent run --suspended --reply-bytes 18 echo
agent delegate --reply-bytes 18 hello
goal budget
agent delegate --with-file --reply-bytes 18 grep -F RamFS
wait grep
recv grep
goal audit
goal audit --replies
goal budget
goal cancel
goal close
```

The `hello` request prints `cannot delegate child`. The first budget check
shows `available=0 reserved=18 spent=0`. The second request uses the open
helper slot, and `recv grep` prints `Hello from RamFS`. With its CRLF the
reply is exactly 18 bytes. The delegation audit shows
`program=hello bytes=18 file=no denied=helper-not-allowed` followed by
an accepted `grep` launch. The reply audit shows `bytes=18 remaining_before=18 sent`,
and the final balance is `available=0 reserved=0 spent=18`.

### Pin the argument for a delegated file

Use `goal grant-file --delegate-to <program> --argument <literal> <path>`
to bind the helper's launch argument to the file grant. The kernel compares
the bytes, including case and spaces, for both shell- and agent-issued
file delegations. Single or double quotes can enclose spaces or an empty
literal. The maximum is 64 bytes.

At a fresh `clv>` prompt, let the shell request `Hello` and the agent
request `RamFS`. Both strings occur in `/hello.txt`.

```text
goal create --children 2 --allow-helper grep --read-bytes 18 --reply-bytes 36
goal grant-file --delegate-to grep --argument RamFS /hello.txt
agent run --suspended hello --delegate --with-file --reply-bytes 18 grep -F RamFS
agent delegate --with-file --reply-bytes 18 grep -F Hello
goal audit
goal budget
goal audit --reads
agent resume
wait hello
recv hello
goal audit
goal audit --reads
goal budget
goal close
```

The first audit shows `denied=argument-not-allowed caller=shell`.
The agent still holds its full reply reservation (`reserved=36 spent=0`),
and `read_bytes=0`. After `agent resume`, the audit shows an accepted
helper with `caller=agent`. `recv hello` prints `Hello from RamFS` and
the final totals are 18 bytes read and 36 bytes spent on the helper's reply
and the agent's forwarded reply.

The grant controls the argument delivered at launch. An admitted helper
receives a read-only handle to the file and spends from the applicable read
budgets. A helper launched without the file uses the goal's ordinary
delegation rules.

### Share a RamFS read budget

`goal create --read-bytes N` sets the total regular-file bytes RamFS may
deliver to the goal's children, agent and helpers combined. Each open file
keeps its own cursor. Each successful read spends from the same goal total,
even when two members read the same file. The default allowance is unlimited.
The reply budget controls IPC output independently.

At the QEMU prompt, let a `grep` helper read the 18-byte boot file before
resuming the `hello` agent that granted it access.

```text
goal create --children 2 --reply-bytes 36 --allow-helper grep --read-bytes 18
goal grant-file --delegate-to grep /hello.txt
agent run --suspended hello
agent delegate --with-file --reply-bytes 18 grep -F RamFS
wait grep
recv grep
agent resume
wait hello
recv hello
goal audit --reads
goal close
```

Repeat with `--read-bytes 36` in the first command.

| Read limit | `recv grep` | `recv hello` | `goal audit --reads` |
| --- | --- | --- | --- |
| 18 | `Hello from RamFS` | `cannot read requested file` (exit 1) | `hello` is denied with `remaining_before=0`. The total is `read_bytes=18`. |
| 36 | `Hello from RamFS` | `Hello from RamFS` (exit 0) | Both reads deliver 18 bytes. The total is `read_bytes=36`. |

In both runs `grep` asks for 128 bytes and receives 18, including the CRLF.
Its next read returns zero at EOF. `hello` then asks for 64 bytes with 18
available in its file. A budget denial returns an error before copying data.
If some allowance remains, the caller can retry from the same position with
a smaller buffer.

### Limit reads by delegated helpers

`goal create --helper-read-bytes N` sets a cumulative RamFS read allowance
shared by the goal's delegated helpers. It applies to helpers launched by
the shell or by the agent. Each helper read must fit both the helper
allowance and the goal's remaining read budget. Direct children and the agent
spend only from the goal budget. Either limit can be set independently.

At the `clv>` prompt, let `grep` try the 18-byte file first, then resume `hello`.

```text
goal create --children 2 --reply-bytes 36 --allow-helper grep --read-bytes 18 --helper-read-bytes 17
goal grant-file --delegate-to grep /hello.txt
agent run --suspended hello
agent delegate --with-file --reply-bytes 18 grep -F RamFS
wait grep
recv grep
agent resume
wait hello
recv hello
goal audit --reads
goal close
```

Repeat with the limits below in the first command. The shell's `recv` prints
`Hello from RamFS` after a successful read and `cannot read requested file`
after a denial.

| Goal bytes | Helper bytes | `recv grep` | `recv hello` | Read audit |
| --- | --- | --- | --- | --- |
| 18 | 17 | Read denied | File contents | `grep` denied with `limit=helper`. Total 18 bytes. |
| 18 | 18 | File contents | Read denied | `hello` denied with `limit=goal`. Total 18 bytes. |
| 36 | 18 | File contents | File contents | Both succeed. Total 36 bytes. |

The kernel checks the bytes available at the cursor. With a 17-byte helper
allowance, reading 10 bytes leaves 7. A subsequent 8-byte read fails if
8 bytes remain in the file. Retrying for 7 reads from the same offset.

### Let a running agent delegate a search

At the QEMU prompt, give `hello` a request to run `grep` on its granted file.
The shell launches only `hello`. When scheduled, `hello` asks the kernel to
start its helper with 18 reply bytes and a read-only copy of its file handle.
It waits for the helper to finish, collects the typed reply and forwards it.

```text
goal create --children 2 --allow-helper grep --read-bytes 18 --reply-bytes 36
goal grant-file --delegate-to grep /hello.txt
agent run hello --delegate --with-file --reply-bytes 18 grep -F RamFS
wait hello
recv hello
goal audit
goal audit --reads
goal audit --replies
goal budget
goal close
```

`wait hello` reports exit 0 and `recv hello` prints `Hello from RamFS`.
The accepted entry shows `caller=agent`. Its `caller_child` matches the
`child` printed by `agent run`, and `accepted child` identifies the new
`grep` launch. Only the helper reads the 18-byte file. The reply audit
records two sends of 18 bytes, one by each launch. The final balance is
`available=0 reserved=0 spent=36`.

Repeat with `--allow-helper echo` in the first command. The audit shows
`denied=helper-not-allowed`, `hello` exits 1, and `recv hello` prints
`cannot delegate child`. Both the read and reply totals are zero.

To inspect the goal before delegation, add `--suspended` before `hello` and
run `goal audit`. It shows zero entries until you issue `agent resume`.
An empty match result is forwarded as an empty `Matches` reply with exit 1.

### Run the mock planner

`agent run hello --mock find-greeting` selects revision 1 and sends its
two-byte definition ID to `hello`.
Its deterministic planner chooses `grep -F Hello` and requests 18 reply
bytes for the helper. The kernel checks the live grant when `hello`
requests the delegation. Both `Hello` and `RamFS` occur in `/hello.txt`.

At the QEMU prompt, grant the exact argument the planner will choose.

```text
goal create --children 2 --allow-helper grep --read-bytes 18 --reply-bytes 36
goal grant-file --delegate-to grep --argument Hello /hello.txt
agent run hello --mock find-greeting
wait hello
recv hello
goal audit
goal audit --reads
goal audit --replies
goal budget
goal close
```

Repeat the sequence, changing only the grant command to
`goal grant-file --delegate-to grep --argument RamFS /hello.txt`.

| Pinned argument | `recv hello` | `goal audit` | Bytes read | Reply bytes spent |
| --- | --- | --- | ---: | ---: |
| `Hello` | `Hello from RamFS` | Accepted, `caller=agent` | 18 | 36 |
| `RamFS` | `cannot delegate child` | `denied=argument-not-allowed caller=agent` | 0 | 0 |

On success, `grep` sends 18 bytes and `hello` forwards them, then exits 0.
On denial, `hello` sends an empty failure reply and exits 1. Both
delegation records identify the requesting `hello` launch in `caller_child`.

### Inspect a pinned agent definition

The build compiles the helper request from the TOML definition and records
the SHA-256 of the exact file bytes. The kernel binds that identity to the
agent at launch. `agent status` reports the pin from suspension through
completion or cancellation, until the owner closes the goal. [Definition source](userspace/hello/definitions/find-greeting-v1.toml)

```text
goal create --children 2 --allow-helper grep --read-bytes 18 --reply-bytes 36
goal grant-file --delegate-to grep --argument Hello /hello.txt
agent run hello --mock find-greeting@3
goal status
goal budget
agent run --suspended hello --mock find-greeting@1
agent status
agent resume
wait hello
recv hello
agent status
goal audit
goal close
```

Revision 3 prints `cannot start agent`. The goal remains `Ready` with
`available=36 reserved=0 spent=0`. Revision 1 starts in the same goal.
Both status snapshots show `definition=find-greeting@1 sha256=` followed
by the same 64 hex digits. Compare those digits on the build host with
`sha256sum userspace/hello/definitions/find-greeting-v1.toml` from the
repository root. The agent returns `Hello from RamFS`.

Repeat the sequence with `goal revoke-file` immediately before `agent resume`.
The pin stays visible. The delegation audit shows `denied=no-delegable-file
caller=agent`, and `recv hello` prints `cannot delegate child`. Before
`goal close`, `goal audit --reads` and `goal budget` show zero bytes read
and zero reply bytes spent.

### Invoke a registered file tool

`find-greeting@2` asks the kernel to invoke `text.search@1` with the literal
`Hello` and an 18-byte reply reservation. This tool accepts a read-only
regular-file grant and a literal of 0–64 bytes. The existing `grep`
executable returns matching lines in a `Matches` reply. An empty literal
matches every line. A search with no matches returns an empty `Matches`
reply. Read failures and results exceeding the 64-byte mailbox return
`ReadFailed` and `ResultTooLarge`, respectively. [Revision 2 definition](userspace/hello/definitions/find-greeting-v2.toml)

```text
goal create --children 2 --allow-helper grep --read-bytes 18 --reply-bytes 36
goal grant-file --delegate-to grep --argument Hello /hello.txt
agent run hello --mock find-greeting@2
wait hello
recv hello
agent status
goal audit
goal audit --reads
goal audit --replies
goal budget
goal close
```

`recv hello` prints `Hello from RamFS`. The delegation audit shows
`program=grep`, the accepted child, `caller=agent`, `tool=text.search@1`
and `elf_sha256=` for the exact linked `grep.elf` embedded in this build.
`agent status` separately reports the SHA-256 of the revision 2 TOML file.
The helper reads 18 bytes and sends 18 reply bytes. Forwarding the result
spends another 18 reply bytes.

Repeat the sequence with `goal grant-file --delegate-to grep --argument RamFS /hello.txt`.
The audit identifies the same tool and image and reports
`denied=argument-not-allowed caller=agent`. The read history is empty.
The only reply is the agent's zero-byte failure, and all 36 reply bytes are
available when the agent exits. The shorthand `find-greeting` still selects
revision 1, whose ordinary program delegation has no tool identity.

### Require an exact tool revision for a goal

`goal create --allow-tool text.search@1` restricts every helper delegation
for that goal to the registered `text.search@1` invocation. The rule applies
to the agent and to the shell when it delegates on the agent's behalf.
Choose one of `--allow-tool` and `--allow-helper` at creation. The file
grant still controls the recipient and literal, and the goal's read and
reply budgets apply.

```text
goal create --children 2 --allow-tool text.search@1 --read-bytes 18 --reply-bytes 36
goal grant-file --delegate-to grep --argument Hello /hello.txt
agent run hello --mock find-greeting@2
wait hello
recv hello
goal audit
goal audit --reads
goal audit --replies
goal budget
goal close
```

Repeat the sequence with `agent run hello --mock find-greeting@1`. Both
definitions request the same executable, literal and 18-byte helper allowance.

| Definition | `recv hello` | Delegation audit | Bytes read | Reply bytes spent |
| --- | --- | --- | ---: | ---: |
| `find-greeting@2` | `Hello from RamFS` | Accepted, `caller=agent`, `tool=text.search@1` | 18 | 36 |
| `find-greeting@1` | `cannot delegate child` | `denied=tool-not-allowed caller=agent` | 0 | 0 |

Revision 1 requests `grep` directly. The kernel records the denial before
allocating the helper. Only the agent's zero-byte failure reply appears in
the reply audit, and all 36 reply bytes are available after it exits.

### Share a read allowance across two helper launches

Two different helper programs can run at the same time. Each program has
one task slot, so two launches of `grep` run in sequence.
At a fresh `clv>` prompt, let the shell launch the first `grep` while
`hello` is suspended. Collect its wait result, reply and completion event
so the running agent can reuse the `grep` program slot for the second search.

```text
goal create --children 3 --allow-helper grep --read-bytes 36 --helper-read-bytes 18 --reply-bytes 54
goal grant-file --delegate-to grep /hello.txt
agent run --suspended hello --delegate --with-file --reply-bytes 18 grep -F RamFS
agent delegate --with-file --reply-bytes 18 grep -F RamFS
wait grep
recv grep
event
agent resume
wait hello
recv hello
goal audit
goal audit --reads
goal budget
goal close
```

Both launches are accepted. `goal audit` shows distinct child handles,
first with `caller=shell`, then with `caller=agent`. Each helper receives a
read-only file handle at offset zero. The first returns `Hello from RamFS`
and spends all 18 helper bytes. The second gets `limit=helper` in the read
audit. The agent forwards the failure, so `recv hello` prints
`cannot read requested file`. The read total is
`read_bytes=18` and the reply budget is `available=36 reserved=0 spent=18`.

Repeat with `--helper-read-bytes 36`. Both searches return `Hello from RamFS`
and the read total is `read_bytes=36`. The two helpers each send 18 bytes
and the agent forwards the second result, bringing reply spending to 54 bytes.

### Identify who requested a delegation

At the QEMU prompt, start `hello` suspended in a goal that permits only
`echo` as a helper. Ask for the same `grep` delegation twice, first from the
shell and then by resuming `hello`.

```text
goal create --children 2 --allow-helper echo --read-bytes 18 --reply-bytes 36
goal grant-file --delegate-to grep /hello.txt
agent run --suspended hello --delegate --with-file --reply-bytes 18 grep -F RamFS
agent delegate --with-file --reply-bytes 18 grep -F RamFS
agent resume
wait hello
goal audit
goal close
```

`goal audit` shows two entries with the same `agent` handle, `program=grep`
and `denied=helper-not-allowed`. The first shows `caller=shell`. The second
shows `caller=agent caller_child=<handle>`, where the handle identifies the
original `hello` launch even after its program slot is reused.

### Revoke a goal's file grant

At the `clv>` prompt, give `hello` a file, then revoke it while `hello` is
suspended with the descriptor already in its handle table.

```text
goal create --read-bytes 18
goal grant-file /hello.txt
agent run --suspended hello
goal revoke-file
agent resume
wait hello
recv hello
goal audit --reads
cat /hello.txt
goal close
```

`wait hello` reports exit 1 and `recv hello` prints `cannot read requested file`.
The read audit shows `entries=0` and `read_bytes=0`. The shell's `cat`
still prints `Hello from RamFS`.

To check delegation, repeat the two-caller example above with
`--allow-helper grep`. Replace the shell's `agent delegate` command with
`goal revoke-file`, then run `agent resume`, `wait hello` and `goal audit`.
The audit shows `denied=no-delegable-file caller=agent`. If `grep` already
holds a handle when the shell revokes the grant, its next read fails as well.

### Native syscall ABI

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

Call 15 (`spawn`) takes a program selector in `a0`. Selectors 0, 1 and 2
choose `hello`, `echo` and `grep`, respectively. The child starts with no RamFS
handles or working directory. The call returns a fresh, nonzero child handle
in `a0` when the task is ready to run and -1 if the selector is unknown or
the program slot is occupied. Child launches, goals and agents draw distinct
handles from one boot-wide sequence. RamFS allocates its handles independently.

Call 16 (`wait`) takes the child handle in `a0`. On success, `a0` contains
the result type. For exit (0) or fault (1), `a1` holds the full exit status
or fault cause. For cancellation (2), `a1` is zero. The call returns -1 in
`a0` for an invalid handle, a caller other than the launch parent, or an
already collected status. Waiting on a suspended child returns -1 immediately.
A parent can wait for an agent that is itself waiting on a helper.

Call 18 (`spawn_at`) takes a program selector in `a0` and a directory handle
from the caller in `a1`. It returns a child handle on success and -1 on error.
The child receives its own directory handle in `a1` and a grant kind of 1 in
`a2`. Launches without a grant set `a2` to 0.

Call 19 (`message_send`) takes a user stack address in `a0` and a length from
1 to 64 bytes in `a1`. A launched child can queue one message for its
live parent. The call returns the byte count on success and -1 on error. A
send to a full mailbox preserves its queued message.

Call 20 (`message_recv`) takes a user stack destination in `a0` and its nonzero
capacity in `a1`. The launch parent passes a child handle in `a2` to receive a reply.
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
`ReadFile` (2) with 1 to 64 payload bytes, and `Argument` (3) with 0 to 64.
It copies the payload and records the caller's task ID before the child
runs. Success returns a fresh child
handle. Invalid arguments or a busy child slot return -1.

Call 25 (`request_recv`) lets a launched child receive its request into the
user stack buffer in `a0` with nonzero capacity in `a1`. It returns the
byte count in `a0`, the sender's task ID in `a1`, and the request kind in
`a2`. An empty mailbox returns zero in all three registers. A bad buffer,
insufficient capacity, or a caller without a child launch returns -1 in
`a0`. A failed copy preserves the pending request. Calls 20 and 25 consume
the same request once.

Call 26 (`reply_send`) takes a user stack payload address in `a0`, length in
`a1`, and reply kind in `a2`. A launched child can queue one reply for
its live parent. The kernel records the sending task's ID. The call returns the
payload length on success. For an empty payload it ignores the address and
returns 0. Invalid arguments or a full mailbox return -1 and preserve any
queued reply.

| Reply kind | Payload | Meaning in the boot programs |
| --- | --- | --- |
| 1 `Echo` | 1–64 bytes | Literal bytes or the child's ready message |
| 2 `FileData` | 0–64 bytes | File contents, including an empty file |
| 3 `Denied` | Empty | `hello` received a read request without a directory grant |
| 4 `ReadFailed` | Empty | The program could not read its input or serve the request |
| 5 `Matches` | 0–64 bytes | All selected lines, or zero bytes for no matches |
| 6 `ResultTooLarge` | Empty | The matched lines exceed the reply capacity |
| 7 `DelegationFailed` | Empty | The agent could not start the requested helper |

For `ReadFile`, `hello` opens the requested path through its directory grant
using `open_at`. The child exits with status 0 after sending a successful reply
and status 1 if the request or send fails.

Call 27 (`reply_recv`) takes a user stack destination in `a0`, its capacity
in `a1`, and a child handle owned by the caller in `a2`. It waits for a reply or child
completion. On delivery it returns the byte count in `a0`, the kernel-recorded
sender in `a1`, and the child's reply kind in `a2`. A zero-capacity buffer
can receive a zero-byte reply. During suspension or after completion an empty
mailbox returns zero in all three registers. An invalid handle, invalid buffer
or insufficient capacity returns -1 and leaves the reply available for retry.
The shell uses the reply kind to display the result and collects exit status
separately with `wait`.

Calls 20 and 27 consume the same reply once. Call 20 returns `Echo` bytes
verbatim. For `FileData` it returns the legacy status byte 0 followed by up
to 63 data bytes. `Denied` and `ReadFailed` return the single status byte 1.
`Matches` returns the selected bytes and `ResultTooLarge` an empty payload.
Use call 27 to distinguish their tags. In the other direction, call 27 reads
a legacy call 19 message as `Echo`. For a `ReadFile`
request or single-file grant it recognizes the original file format, mapping
status 0 to `FileData` and a lone status byte 1 to `ReadFailed`.

Call 28 (`spawn_with_file_pipe`) takes the program selector in `a0` and the
shell's regular file handle in `a1`. The kernel grants the child an independent
read-only file handle using the same rules as call 23. It also creates a
one-way pipe for this launch. The returned child handle identifies the shell's
read end. At startup the child gets its file in `a1`, grant kind 2 in `a2`,
and the handle for its write end in `a3`. Other launches start with `a3` zero.

| Call | Arguments | Result in `a0` |
| --- | --- | --- |
| 29 `pipe_read` | `a0` launch handle, `a1` shell stack destination, `a2` capacity | Bytes read in order, or 0 at EOF |
| 30 `pipe_write` | `a0` launch handle, `a1` child stack source, `a2` length | Bytes accepted, possibly a prefix |
| 31 `pipe_close` | `a0` launch handle | 0 after closing the caller's pipe end |

The pipe holds 32 bytes. A nonempty write sleeps when the pipe is full and
resumes when the shell reads. A nonempty read sleeps while the pipe is empty
and the writer is open. Each transfer validates the full user buffer before
moving bytes. Zero-length I/O on a connected endpoint returns 0 immediately.
Invalid handles, wrong endpoints or invalid buffers return -1.

Closing the writer or stopping the child preserves buffered bytes for the
reader. Once drained, further reads return EOF. Closing the reader or stopping
the shell discards unread bytes and wakes a blocked writer with an error.
Closing either end leaves the launch status available to `wait`. If the
shell waits for a result or reply while that child is blocked on a full pipe,
the wait or receive returns -1 so the shell can drain or close the pipe and
retry. A queued reply or completed status can still be collected. A successful
relaunch invalidates the previous pipe handle along with the previous launch.

Call 32 (`event_wait`) takes no arguments. Only the shell can collect events.
It receives the oldest pending completion's launch handle in `a0` and its
kind in `a1` (0 for exit, 1 for fault, 2 for cancellation). While the shell
is alive, the kernel records one notification per completed shell-owned launch,
up to three at a time. Agent-launched helpers deliver replies and wait results
to their agent. The boot greeting has no parent launch and produces no event.
Other callers receive -1 in `a0`.

With an empty queue, the call sleeps while a shell-owned child can make progress.
If every outstanding child is suspended or blocked writing to a full pipe,
or there are no outstanding children, it returns zero in both registers.
The shell can resume an agent, drain or close a pipe, and retry. Reading an
event preserves the child's reply, buffered pipe data and wait result.
Collecting any of those first also leaves the event available. A successful
relaunch discards the previous handle's unread event. A failed launch preserves it.

| Call | Arguments | Result in `a0` |
| --- | --- | --- |
| 33 `goal_create` | None | Fresh goal handle, or -1 if the slot is occupied or handles are exhausted |
| 34 `goal_run` | `a0` goal handle, `a1` program selector | Fresh child launch handle, or -1 on failure |
| 35 `goal_status` | `a0` goal handle | State tag, with the member reporting that state in `a1` and detail in `a2` |
| 36 `goal_close` | `a0` goal handle | 0 on release, 1 while running |
| 37 `goal_cancel` | `a0` goal handle | 0 for a new or repeated cancellation, 1 if the goal already completed or failed |
| 38 `agent_run` | `a0` goal handle, `a1` program selector | Fresh agent handle, with its child launch handle in `a1` |
| 39 `agent_status` | `a0` agent handle | State tag, with the original child in `a1`, detail in `a2` and owning goal in `a3` |
| 40 `agent_suspend` | `a0` agent handle | 0 when suspended, 1 if the agent already finished |
| 41 `agent_resume` | `a0` agent handle | 0 when runnable, 1 if the agent already finished |
| 42 `agent_run_suspended` | `a0` goal handle, `a1` program selector | Fresh agent handle, with its suspended child launch handle in `a1` |
| 43 `goal_create_with_reply_budget` | `a0` total reply bytes | Fresh goal handle |
| 44 `goal_budget` | `a0` goal handle | Limit flags, with available bytes in `a1`, reserved bytes in `a2` and spent bytes in `a3` |
| 45 `goal_run_with_reply_budget` | `a0` goal handle, `a1` program selector, `a2` requested reply bytes | Fresh ordinary child launch handle |
| 46 `agent_run_with_reply_budget` | Same as call 45, plus `a3` start mode (0 runnable, 1 suspended) | Fresh agent handle, with the child launch handle in `a1` |
| 47 `goal_grant_file` | `a0` goal handle, `a1` open RamFS file handle owned by the shell | 0 when attached, -1 on rejection |
| 48 `goal_create_with_options` | `a0` lifetime child limit (1, 2 or 3), `a1` option flags, `a2` total reply bytes if bit 0 is set, `a3` allowed helper program (bit 1) or exact tool ID (bit 4), `a4` total RamFS read bytes if bit 2 is set, `a5` helper RamFS read bytes if bit 3 is set | Fresh goal handle, or -1 on rejection |
| 49 `agent_delegate` | `a0` live agent handle, `a1` program selector, `a2` reply bytes to transfer | Fresh helper launch handle, or -1 on rejection |
| 50 `goal_grant_delegable_file` | `a0` goal handle, `a1` shell file handle | 0 when attached with permission for the goal's agent to delegate read access, or -1 on rejection |
| 51 `agent_delegate_with_file` | `a0` live agent handle, `a1` program selector, `a2` reply bytes to transfer | Fresh helper launch handle with read access to the agent's file, or -1 on rejection |
| 52 `goal_audit` | `a0` goal handle, `a1` shell stack destination, `a2` capacity in bytes | Retained entry count (0–4), with overwritten count in `a1`, or -1 on failure |
| 53 `goal_reply_audit` | Same arguments as call 52 | Retained reply-budget entry count (0–4), with overwritten reply entry count in `a1`, or -1 on failure |
| 54 `goal_grant_file_to` | `a0` goal handle, `a1` shell file handle, `a2` allowed helper program selector | 0 when attached, -1 on rejection, including an unknown selector |
| 55 `spawn_with_file_arg` | `a0` program selector, `a1` shell file handle, `a2` argument address, `a3` argument length | Fresh child handle with a read-only file and an `Argument` request |
| 56 `agent_delegate_with_file_arg` | `a0` agent handle, `a1` program selector, `a2` reply bytes, `a3` argument address, `a4` argument length | Fresh helper handle with the delegated read-only file and an `Argument` request |
| 57 `goal_read_audit` | Same arguments as call 52 | Retained read entry count (0–4), overwritten count in `a1` and lifetime bytes delivered in `a2`, or -1 on failure |
| 58 `agent_run_with_request` | `a0` goal, `a1` program, `a2` flags, `a3` optional agent reply allowance, `a4` request address, `a5` request length | Fresh agent handle, with its child launch handle in `a1` |
| 59 `goal_audit_with_caller` | Same arguments as call 52 | Retained delegation entry count (0–4), with overwritten count in `a1`, or -1 on failure |
| 60 `goal_revoke_file` | `a0` goal handle | 0 for an owned goal, including a repeat call, or -1 for an invalid handle or caller |
| 61 `goal_read_audit_with_limit` | Same arguments as call 57 | Same count, overwritten count and lifetime read bytes as call 57, with the effective limit identified in each entry |
| 62 `goal_grant_file_to_with_arg` | `a0` goal handle, `a1` shell file handle, `a2` helper selector, `a3` literal address, `a4` literal length (0–64 bytes) | 0 when the file and its argument restriction are attached, -1 on rejection |
| 63 `agent_status_with_definition` | `a0` agent handle, `a1` shell stack destination, `a2` capacity in bytes | Same status registers as call 39, plus a 34-byte definition record in the buffer |
| 64 `agent_invoke_file_tool` | `a0` agent handle or zero for self, `a1` versioned tool ID, `a2` reply reservation, `a3` literal address, `a4` literal length (0–64) | Fresh helper handle on admission, or -1 on rejection |
| 65 `goal_audit_with_tool` | Same arguments as call 52 | Retained delegation count and overwritten count as in call 59, with 104 bytes per entry |

The shell owns the goal and controls its agents. Calls 49, 51, 56 and 64 also
accept `CURRENT_AGENT` (zero) when the calling task is itself the goal's live
agent. The kernel resolves its original launch and agent handle. Only the shell
can delegate by supplying an explicit agent handle. An unauthorized caller
or a stale handle returns -1 in `a0`. Calls 33 and 43 create single-child
goals with unrestricted helper selection and unlimited reads. Call 48
accepts up to three lifetime launches. Bits 1 and 4 are mutually exclusive.
When both are clear, the kernel ignores `a3`. Each budget register is
ignored when its flag is clear.
Setting bit 2 or bit 3 with its register zero creates a zero-byte goal or
helper allowance respectively. Omitting either flag leaves that allowance unlimited.
Unknown flag bits, conflicting selector flags, an unknown program or an
unregistered tool revision return -1 before the kernel allocates a goal handle.
The run calls accept a ready goal or a running goal with a place left. They
return -1 for an unknown selector or start mode, a busy program slot, a full
goal, a second agent, insufficient available bytes, or exhausted handles.
A failed launch preserves goal membership, grants, reservations and launch handles.

The kernel binds the child to the goal before its first instruction.
The agent run calls also create and bind the agent in that operation. Direct
children inherit any file granted to the goal. Call 58 queues a typed
request before the agent starts. Bit 0 starts it suspended. Bit 1 selects
the explicit agent reply allowance in `a3`. Otherwise the agent inherits
the available balance. Bits 2 and 3 select the request format.

| Format flags | Request kind | Payload |
| --- | --- | --- |
| Both clear | `DelegateWithFile` (tag 4) | Two little-endian RV64 words for the helper selector and its reply allowance, followed by 0–64 literal argument bytes |
| Bit 2 only | `PlanIntent` (tag 5) | One intent byte. Intent 1 for `hello` pins `find-greeting@1`. Other legacy intents retain their original delivery behavior. |
| Bit 3 only | `PlanDefinition` (tag 6) | Two unsigned bytes for the intent and revision. The pairs `[1, 1]` and `[1, 2]` select the corresponding `find-greeting` revisions for `hello`. |

Unknown flag bits, both format bits set, an unreadable buffer or the wrong
length fail before admission. For tag 6 the kernel also verifies that the
definition exists for the selected executable before consuming handles,
reserving bytes or launching the child. The kernel supplies the digest
from its boot image. The agent interprets the workflow and the kernel
checks the live grant when the agent requests delegation.

Call 63 returns lifecycle state and the original agent's definition pin in
one snapshot. The first two bytes hold the intent and revision. The next
32 hold the binary SHA-256 in digest order. An unpinned launch returns
34 zero bytes. Only the shell that owns the agent can inspect it. A stale
handle, a short buffer or an invalid destination returns -1 before copying.

Calls 47, 50, 54 and 62 accept one regular file while the goal is ready. The kernel
retains the file independently of the shell's descriptor, cursor and pathname. Closing
or cancelling a ready goal, or stopping its owner, releases this reference.
On a successful direct launch the child receives a fresh read-only handle
at offset zero in `a1`, file kind 2 in `a2` and 0 in `a3`. Direct siblings
get independent cursors on the same retained file, even after pathname reuse.
The kernel releases the goal's reference when all places have been used or the
goal finishes. Each child can close its own handle. Exit, fault or cancellation
closes any handle it still holds.

Call 60 withdraws the goal's retained file and closes all descriptors
derived from that grant, including copies already held by helpers. Only the
owning shell can call it. Each read and revocation runs to completion on
the boot hart. Bytes delivered earlier stay charged. A later read using a
revoked descriptor fails before copying or charging bytes. Other opens of
the same file keep their own authority and cursor.

Revocation succeeds when the goal has no outstanding grant. The owner can
attach a fresh grant while the goal is still ready. After revocation in a
running goal, later direct launches start with an empty handle table, and
requests to delegate the withdrawn file fail.

Call 50 lets the direct agent delegate to any eligible helper. Call 54 binds the
agent's handle to one helper program for delegation. Ordinary direct children
receive read access. Closing the agent's handle ends its ability
to pass that file on, even while the goal still holds a reference.

Call 62 binds the selected program and an owned copy of its literal argument
to the grant. A zero length pins an explicitly empty argument and ignores the
address. A nonempty literal must fit in the caller's stack. Calls 56 and 64 must
present the exact bytes to receive the file. Call 51 omits the argument and
therefore receives denial code 12 on a pinned grant, including an empty pin.
A mismatch allocates no helper, file handle or reply reservation. Revoking a
ready goal's file also releases its pinned argument.

Call 64 resolves the requested file tool in the boot registry before helper
admission. The high 32 bits of the ID select the tool family and the low 32
bits select the exact revision. `text.search@1` has ID `0x0000000100000001`.
An unknown revision fails before any helper launch or budget reservation.

Calls 49, 51, 56 and 64 check the goal's delegation rule before checking
the program slot or reserving resources. A program rule requires the helper
to resolve to the allowed executable. A tool rule requires call 64 with the
exact registered ID and revision, even when a program request names the same
executable. Direct goal and agent launches use their ordinary admission rules.
On admission they transfer a byte count from zero up to the agent's remaining
allowance before the helper's first instruction. Both shares count toward
the goal's reserved total. Call 49 starts the helper with zero in `a1`, `a2`
and `a3`. Calls 51, 56 and 64 check the selected helper against the recipient on the
agent's open delegable handle before allocating a read-only handle with an
independent cursor.
The new file handle arrives in `a1`, file kind 2 in `a2` and zero in `a3`.
The kernel commits the file grant, reply reservation and launch together.
The caller receives the helper handle and owns its reply and wait result.
The shell receives events for the launches it issues, including helpers it
launches on behalf of an agent. Across different live parents, a stopped
program slot stays busy while its old parent has a pending reply, wait
result, shell event or open pipe reader. This lets the parent collect the
result even when another caller tries to launch the same program.

Calls 55, 56 and 64 copy up to 64 literal argument bytes into the launch mailbox
before the child can run. The child reads the argument with call 25. An empty
argument has tag 3. Invalid arguments fail before admission or reservation.

Calls 52, 59 and 65 read the same four-entry delegation history for calls 49,
51, 56 and 64. Only the shell that owns the goal can read it. The kernel records
both accepted and denied requests after authenticating the caller and
resolving the agent handle. Each 48-byte call 52 entry begins with the agent
handle, requested program selector, requested reply bytes and file flag
(0 or 1). The final two words
hold the decision code and helper launch handle. All six words are little-endian
RV64 values. On acceptance, code 0 accompanies the new child's handle.
On denial, the code identifies the first failed check and the handle is zero.

Call 59 appends a seventh little-endian RV64 word to each entry, making
56 bytes per entry. Zero identifies the shell. A nonzero value is the
calling agent's original child launch handle. The kernel captures it at
the delegation decision. Both formats use the same order and overwritten
count.

Call 65 extends the 56-byte caller record with a state word, the requested
tool ID and 32 digest bytes in SHA-256 order. State 0 identifies an ordinary
program request and has zeroed tool fields. State 1 records an unresolved ID
with a zero digest. State 2 records the exact registered ID and the digest of
its embedded ELF, including when the live grant denies the request. For an
unresolved ID the program word is all ones and the unknown-selector denial
uses code 2. The shell displays that denial as `unknown-tool`.

| Denial code | Reason |
| --- | --- |
| 1 | Agent already finished |
| 2 | Unknown program selector |
| 3 | Program slot busy |
| 4 | Launch handles exhausted |
| 5 | Goal child limit reached |
| 6 | Insufficient reply bytes in the agent's reservation |
| 7 | Agent has no open delegable file |
| 8 | RamFS file handles exhausted |
| 9 | The agent's file grant does not permit the selected helper program |
| 10 | Invalid argument length or caller buffer |
| 11 | The goal does not allow this program as a delegated helper |
| 12 | The file grant requires an argument and the launch omits it or supplies different bytes |
| 13 | The goal requires an exact tool revision and this delegation requests a program or a different registered tool |

Call 53 returns the decisions recorded when a goal member's `message_send`
or `reply_send` reaches the budget check. The kernel validates the mailbox,
message arguments and user buffer first. Each 32-byte entry contains four
little-endian RV64 words in order. They hold the caller's original child
launch handle, byte count submitted to the budget check, remaining allowance
before the send, and flags. For a sent reply, the flags are 0 for a finite
allowance or 1 for an unlimited allowance (remaining word zero). A value of
2 means the reservation was too small. A valid zero-byte send gets an entry.

Calls 57 and 61 read the same history of RamFS call 6 budget decisions. Each
call 57 entry contains five little-endian RV64 words and occupies 40 bytes.
In order, they carry the original child launch handle, requested buffer
capacity, bytes available for this read at the file cursor, prior allowance
and flags. Flags are 0 for a finite successful read, 1 for an unlimited
successful read (prior allowance word zero), and 2 for a budget denial.
EOF and valid zero-capacity reads have zero cost and receive entries. Invalid
handles and buffers fail before the budget check. The lifetime total counts
successful copies and saturates at the largest RV64 unsigned value for
unlimited goals.

For a helper, the prior allowance is the smaller applicable balance. Call 61
appends a sixth little-endian RV64 word to each entry, making 48 bytes.
Zero identifies the goal allowance and one identifies the helper allowance.
The goal takes precedence when the remaining amounts are equal. A successful
copy charges the goal once and, for a helper, spends the same number of bytes
from its helper allowance. Denials preserve both balances and the cursor.

Calls 52, 53, 57, 59, 61 and 65 each copy up to four retained entries, oldest
first. A short buffer or an invalid destination range returns -1 before copying
any bytes. Zero capacity succeeds for an empty audit. Reads preserve the entries
and the overwritten count, which saturates at the largest RV64 unsigned value.

`agent_status` uses tags 1–5 below and tag 6 for a suspended child (`a2` is
zero). It always returns the original child handle on success. `goal_status`
reports tag 1 throughout a live child's suspension.

| `goal_status` tag | State | `a1` | `a2` |
| --- | --- | --- | --- |
| 0 | Ready | 0 | 0 |
| 1 | Running | Child handle | 0 |
| 2 | Completed | Child handle | Exit status 0 |
| 3 | Failed | Child handle | Nonzero exit status |
| 4 | Failed | Child handle | Fault cause |
| 5 | Cancelled | Cancelled child handle, or 0 if cancelled while ready | 0 |

`goal_status` reports the earliest-launched live member. Once all members
have stopped, it reports a cancelled member first, then a member that failed,
then a successful member.
Launch order breaks ties. `agent_status` always reports the agent's own
child, including after that child stops while a sibling runs.

If an agent exits or faults, the kernel records its completion and cancels
all its live helpers in launch order before scheduling another task. Each
helper releases its unused reply reservation. The shell can collect events
and wait results for the agent and for each helper the shell launched.
An agent-issued helper's results belong to that agent. `agent_status`
retains the agent's own exit status or fault cause.

Repeating suspend or resume on a live agent already in the requested state
succeeds. A paused child keeps its slot and any queued reply. Suspension and
resumption produce no completion event.

`goal_cancel` stops each live member by its original launch handle in launch
order. The kernel queues one event per cancelled shell-owned child. Repeating
cancellation returns 0 even after another launch reuses a slot. Cancelling a
ready goal produces no event.

Goal and agent status reads are repeatable and survive reuse of the child's
program slot. Closing the goal releases its agent identity. The child's pending
reply, event and wait status remain available under the usual launch rules.
If the shell exits or faults, the kernel cancels every live member by its
original launch handle, even when suspended. It releases the goal and its agent
before scheduling another task. Children launched outside the goal can still
run to completion, including a newer launch in the same program slot.

The kernel reserves each child's reply allowance when it binds the launch.
Calls 34, 38 and 42 reserve all available bytes. Calls 45 and 46 can also
delegate a finite allowance from an unlimited goal. With a finite goal,
`available + reserved + spent` always equals the original limit. Call 44
returns 0 for finite balances, 1 for unlimited available bytes with a finite
reservation, and 3 while both are unlimited. An unlimited field has register
value 0 and prints as `unlimited`. Finite counts span the full RV64 unsigned
range. For an unlimited goal, a finite reservation total or spent total
larger than that maximum prints as the maximum. Each child's reservation is
tracked separately, so the reserved total becomes exact as soon as it fits.

Each successful `message_send` or `reply_send` moves the byte count returned
by the call from reserved to spent. A valid zero-byte reply costs zero. A
failed send, including one that exceeds the reservation, leaves the mailbox
and budget intact. Reading a reply or suspending and resuming the child
preserves the counters. Exit, fault and cancellation return that child's
unused bytes to its funding source. A helper returns its balance to its live
agent. When the agent stops, all remaining bytes from that delegation become
available to the goal. The shell can inspect the final counters until it closes
the goal.
Children launched outside the goal use their own mailbox independently.

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
  userspace/libclv/lib.rs userspace/echo/main.rs userspace/grep/main.rs \
  userspace/hello/main.rs userspace/shell/main.rs
```
