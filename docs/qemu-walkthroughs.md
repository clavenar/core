# QEMU walkthroughs

These examples use the native `clsh>` shell. Start each independent example
with a fresh boot unless it explicitly continues the previous one. RamFS and
goal handles live for one boot. [Build and boot the kernel](../README.md#build-and-run-the-rv64-kernel)

- [Boot and shell](#boot-and-shell)
- [Goals and file grants](#goals-and-file-grants)
- [Requests, replies and file streams](#requests-replies-and-file-streams)
- [Delegation and read limits](#delegation-and-read-limits)
- [Mock planning and registered tools](#mock-planning-and-registered-tools)
- [Reviewed renames](#reviewed-renames)
- [Helper accounting and revocation](#helper-accounting-and-revocation)

## Boot and shell

In an ANSI terminal, the shell displays the `clsh>` prompt and help headings
in green. `goal commit` prints `rename committed` in green, `review expired`
or `goal deadline expired` in yellow, and `commit conflict` in red. The shell
resets the color after each prompt, heading, or message.

Type `help` for the full command reference. It groups commands by task, shows
their arguments beside short descriptions, and lists options under the command
that accepts them. Type `goal` or `agent` on its own to see just that section.

The build links the shell and each registered program as separate RV64 ELFs
and embeds them in the kernel. To add a boot program, put its entry point in
`userspace/<name>/main.rs` and append its name to the program registry. The
build, shell and kernel use that registry for names and stable launch
selectors. A versioned file tool maps an exact tool ID to one of those
programs. [Program registry](../crates/clv-abi/src/program.rs) · [File tool registry](../crates/clv-abi/src/tool.rs)
The loader accepts static, little-endian ELF64 executables with one
read/execute segment of at most 28 KiB at `0x00400000`. The loader allocates
one to seven pages based on the segment's memory size (`p_memsz`) and zero-fills
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
shared in supervisor mode. The kernel, 48 KiB supervisor stack, and 47-frame
boot pool fit within one aligned 2 MiB physical region.

OpenSBI timer interrupts drive round-robin scheduling. A `timer switch` line
shows which task was interrupted and which task will run next. The kernel
starts a fresh quantum when the timer fires or a stopped task is replaced.

### Launch a child and collect its result

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

## Goals and file grants

### Create a goal and inspect its children

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

### Suspend and resume an agent

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

### Reserve reply bytes for children

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

### Grant a file and limit delegation

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

### Inspect delegation and reply decisions

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

### Cancel a goal

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

To cancel a goal with both a live child and a pending rename, start a fresh boot.

```text
write /draft.txt keep
goal create
goal stage-rename /draft.txt final.txt
agent run --suspended hello
goal cancel
wait hello
goal diff
goal audit --publication
cat /draft.txt
goal close
```

The shell prints `goal cancelled` and `staged rename discarded`. The wait
reports that `hello` was cancelled, the diff is empty, and the audit shows
`outcome=discarded-by-cancel`. The original file still prints `keep`.

### Cancel a reviewed rename

On a fresh boot, let the agent finish and review its proposed rename.

```text
write /draft.txt keep
goal create --preserve-file /draft.txt
goal grant-file --stage-rename /draft.txt
agent run hello --stage-rename final.txt
wait hello
goal diff
```

When the diff reports `ready to commit`, cancel the goal twice.

```text
goal cancel
goal cancel
goal audit --publication
goal diff
cat /draft.txt
goal close
```

Both calls print `goal already finished`. The first also prints
`staged rename discarded`. The audit contains exactly one row for
`/draft.txt` and `final.txt`, with `base=2 revision=2 outcome=discarded-by-cancel`.
The diff is empty and `/draft.txt` still contains `keep`.

To check the reverse order, repeat the first block on a fresh boot, then run
these commands while the review is valid.

```text
goal commit
goal cancel
goal audit --publication
cat /final.txt
goal close
```

The audit contains one row with `base=2 revision=3 outcome=published`.
Cancellation prints only `goal already finished`, and `/final.txt` contains `keep`.

### Give a goal a deadline

Paste this block at a fresh `clsh>` prompt. The 15 seconds start at
`goal create`. The first status shows `Running` and the absolute deadline
in guest nanoseconds.

```text
write /draft.txt keep
goal create --timeout-ms 15000
goal stage-rename /draft.txt final.txt
agent run --suspended hello
goal status
```

Leave QEMU running for 15 seconds, then enter

```text
wait hello
goal status
goal diff
goal audit --publication
cat /draft.txt
goal close
```

`wait hello` prints `hello cancelled`. The goal status shows `Cancelled`
and `deadline=expired`. The diff is empty and the audit contains one row
with `base=2 revision=2 outcome=discarded-by-expiry`. The source still
contains `keep`.

To see how the deadline applies after a child finishes, start a fresh boot
and paste this block.

```text
write /draft.txt keep
goal create --timeout-ms 15000 --preserve-file /draft.txt
goal grant-file --stage-rename /draft.txt
agent run hello --stage-rename final.txt
wait hello
goal diff
```

The child exits successfully and the diff reports `ready to commit`.
Leave QEMU running for 15 seconds, then enter

```text
goal commit
goal status
goal audit --publication
cat /draft.txt
goal close
```

Commit prints `goal deadline expired`. The goal still reports `Completed`,
and the audit records `discarded-by-expiry`. To try the other ordering,
repeat the setup on a fresh boot and enter `goal commit` immediately after
`goal diff`. Once the deadline passes, the audit still shows one
`outcome=published` and `cat /final.txt` prints `keep`.

### Observe a goal from a separate process

Paste these commands at a fresh `clsh>` prompt. The shell starts `echo`
as a read-only observer and prints the snapshot it collected.

```text
goal create --timeout-ms 15000 --reply-bytes 24
agent run --suspended --reply-bytes 12 hello
goal observe echo
```

The first report shows `Running` with the original `hello` child handle,
an absolute `deadline_ns`, and `available=12 reserved=12 spent=0`. The
observer runs outside the goal even though its only member slot is occupied.
Leave QEMU running for 15 seconds, then enter

```text
goal observe echo
goal audit --replies
wait hello
goal close
```

The second report shows `Cancelled` for the same child, `deadline=expired`
and `available=24 reserved=0 spent=0`. The reply audit shows `entries=0`
and `wait hello` reports `hello cancelled`. Each observation launches a
fresh `echo` and collects its reply and exit status. If `echo` is already
running, the command reports `cannot start goal observer`.

## Requests, replies and file streams

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

## Delegation and read limits

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

At a fresh `clsh>` prompt, let the shell request `Hello` and the agent
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

### Limit file reads to a directory

`goal create --read-under /home/work` allows a goal member to read a granted
file only while it belongs to that directory or a descendant. RamFS checks
the opened file on each read by a child, agent or delegated helper. The shell
can grant a file from elsewhere to exercise the denial at read time.

At a fresh `clsh>` prompt, run the same agent in two goals.

```text
mkdir /home
mkdir /home/work
mkdir /private
write /home/work/draft.txt keep
write /private/draft.txt secret
goal create --read-under /home/work
goal grant-file /home/work/draft.txt
agent run hello
wait hello
recv hello
goal audit --reads
goal close

goal create --read-under /home/work
goal grant-file /private/draft.txt
agent run hello
wait hello
recv hello
goal audit --reads
goal close
```

The first reply contains `keep`. The audit reports `read_bytes=6`, including
the line ending written by `write`. In the second run the grant succeeds,
`hello` exits 1 and the reply is `cannot read requested file`. The audit
identifies its child launch handle and shows `bytes=0 denied=outside-read-scope`
and `read_bytes=0`.

A retained file that has been unlinked has no parent in the tree and fails
the same check. The boot files belong to `/`. The shell can combine
`--read-under` with a byte budget or a separate `--rename-under` directory.

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

At the `clsh>` prompt, let `grep` try the 18-byte file first, then resume `hello`.

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

## Mock planning and registered tools

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
completion or cancellation, until the owner closes the goal. [Definition source](../userspace/hello/definitions/find-greeting-v1.toml)

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
`ReadFailed` and `ResultTooLarge`, respectively. [Revision 2 definition](../userspace/hello/definitions/find-greeting-v2.toml)

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

## Reviewed renames

### Stage a private file rename

The goal owner can propose one rename while a goal is ready or running.
Choose a writable RamFS file and an unused basename in its current directory.
At a fresh `clsh>` prompt, try a path containing `..` and then a valid basename.

```text
mkdir /work
write /work/draft.txt hello
goal create
goal stage-rename /work/draft.txt ../final.txt
goal diff
goal stage-rename /work/draft.txt final.txt
goal diff
ls /work
goal cancel
goal diff
cat /work/draft.txt
goal close
```

The first attempt reports `cannot stage rename`, and the first diff says
`no staged changes`. The second diff prints `rename /work/draft.txt -> final.txt`
and `preview only` while the goal is ready. You can read it repeatedly.
The public directory still lists only `draft.txt`.
After cancellation the diff is empty and `cat` prints `hello`.

Staging retains the original file and records both names at that moment.
If the shell removes the source and creates a new file at the same path,
the pending change still refers to the original file. A completed or failed
goal keeps its diff available for review. Cancelling the goal, closing it
successfully, or exiting the owner releases the pending change.

### Publish a reviewed RamFS rename

After a goal completes successfully, run `goal diff` to review the staged
rename. When RamFS validates the change, the shell prints `ready to commit`
with the deadline on the guest clock and saves its single-use token. Enter
`goal commit` within 60 guest seconds to authorize that exact rename and
check it again. At a fresh QEMU prompt, use
`echo` to bring the goal to a successful completion.

```text
mkdir /work
write /work/draft.txt hello
goal create
goal stage-rename /work/draft.txt final.txt
goal run echo
wait echo
goal diff
goal commit
ls /work
cat /work/final.txt
goal diff
goal close
```

The first diff prints `rename /work/draft.txt -> final.txt` and `ready to commit`.
Commit reports `rename committed`. The directory now lists only `final.txt`.
Its contents are still `hello`, and the final diff says `no staged changes`.
Open handles to the file keep their cursors and access rights.

To see a conflict, reboot and repeat through the first `goal diff`. Then run

```text
write /work/draft.txt revised
goal commit
ls /work
cat /work/draft.txt
goal close
```

Commit reports `commit conflict`. The directory still lists `draft.txt`, and
`cat` prints `revised`. RamFS records a single boot-local revision at staging,
so even a public write in another directory invalidates the proposal. You can
inspect the frozen diff after a conflict. Start a fresh goal to stage and
review a rename against the updated filesystem.

### Let a rename review expire

At a fresh QEMU prompt, create a goal and review the same rename twice.

```text
write /draft.txt hello
goal create --preserve-file /draft.txt
goal stage-rename /draft.txt final.txt
goal run echo
wait echo
goal diff
goal diff
```

Both reviews show the same absolute deadline, in seconds on the guest's
monotonic clock. Leave QEMU running at the prompt for at least 61 seconds
after the first review, then enter

```text
goal commit
cat /draft.txt
goal diff
goal commit
cat /final.txt
goal close
```

The first commit reports `review expired` and `draft.txt` still contains
`hello`. The next diff checks the staged rename again and shows a later
deadline. Commit promptly and `final.txt` contains `hello`.

To check an edit after expiry, repeat the first block from a fresh boot and
leave QEMU running for the same interval. Then enter

```text
write /draft.txt revised
goal diff
goal discard
cat /draft.txt
goal close
```

The diff shows the staged rename and `conflict`. The owner can discard it,
and `draft.txt` contains `revised`.

### Preserve a goal's input file

`goal create --preserve-file <path>` pins one writable RamFS file and copies
its contents at goal creation. To publish a rename, the proposed destination
must receive that same file with exactly those bytes. At a fresh QEMU prompt,
run

```text
mkdir /work
write /work/draft.txt hello
goal create --preserve-file /work/draft.txt
goal stage-rename /work/draft.txt final.txt
goal run echo
wait echo
goal diff
goal commit
ls /work
cat /work/final.txt
goal close
```

To verify each outcome, start the sequence above from a fresh boot. Insert
`write /work/draft.txt revised` at the indicated point. For the three runs
with an extra write, use `cat /work/draft.txt` after `ls /work`.

| When the extra write runs | `goal diff` | `goal commit` | Public file |
| --- | --- | --- | --- |
| No extra write | `ready to commit` | `rename committed` | `final.txt` contains `hello` |
| Between `goal create` and `goal stage-rename` | `postcondition failed` | Requires a passing review | `draft.txt` contains `revised` |
| Between `wait echo` and `goal diff` | `conflict` | Requires a passing review | `draft.txt` contains `revised` |
| Between `goal diff` and `goal commit` | `ready to commit` | `commit conflict` | `draft.txt` contains `revised` |

A rejected review still prints the frozen source and destination. It clears
any earlier token. Repeating `goal diff` after a commit conflict shows that
same rename and reports `conflict`.

Replacing the input with a new file of identical contents before staging
also fails the postcondition. The option can be combined with the existing
goal budgets and helper restrictions. A child needs a separate file grant
to read the input.

### Limit renames to a directory

Use `goal create --rename-under <dir>` to limit a goal's renames to files
in that directory and its descendants. RamFS checks the file's actual parent
at staging, review and commit. The directory stays pinned until the goal closes
or its owner exits. An agent still needs a rename grant for the individual file.
Combine the option with `--preserve-file <path>` to require its original bytes.

At a fresh `clsh>` prompt, try a path that starts in the permitted directory
and resolves to a file in its sibling.

```text
mkdir /work
mkdir /private
write /work/draft.txt keep
write /private/draft.txt private
goal create --rename-under /work
goal stage-rename /work/../private/draft.txt final.txt
goal audit --renames
goal diff
```

The audit shows `denied=outside-rename-scope caller=shell`, and the diff
reports `no staged changes`. In the same goal, authorize the agent to rename
the file in `/work`.

```text
goal grant-file --stage-rename /work/draft.txt
agent run hello --stage-rename final.txt
wait hello
goal diff
goal commit
cat /work/final.txt
cat /private/draft.txt
goal close
```

The diff shows `rename /work/draft.txt -> final.txt`. Commit reports
`rename committed`, and the two reads print `keep` and `private`.

### Let the goal's agent propose a rename

At a fresh QEMU prompt, give `hello` permission to stage a rename of one
writable file. Its request supplies only the new basename. The kernel uses
the file handle from the agent's own launch to identify the source.

```text
mkdir /work
write /work/draft.txt hello
goal create --preserve-file /work/draft.txt
goal grant-file --stage-rename /work/draft.txt
agent run hello --stage-rename final.txt
wait hello
goal diff
ls /work
goal commit
cat /work/final.txt
goal close
```

`hello` exits with status 0. `goal diff` shows `rename /work/draft.txt -> final.txt`,
and `ls /work` still lists only `draft.txt`. After `goal commit` reports
`rename committed`, `cat` prints `hello`. The request uses zero file-read
and reply bytes.

To check a read-only grant, repeat from a fresh boot through `goal diff`
using `goal grant-file /work/draft.txt`. The agent exits with status 1
and the diff is empty.

### Revoke a rename grant after review

At a fresh `clsh>` prompt, let the agent propose a rename. Read the diff
after the agent exits to obtain a commit token, then revoke the grant twice.

```text
write /draft.txt keep
goal create --preserve-file /draft.txt
goal grant-file --stage-rename /draft.txt
agent run hello --stage-rename final.txt
wait hello
goal diff
goal revoke-file
goal revoke-file
goal commit
goal audit --publication
cat /draft.txt
goal close
```

Both revocations print `file revoked`. Only the first also prints
`staged rename discarded`. Commit asks for a new passing review, the file
still contains `keep`, and the audit shows one row.

```text
source="/draft.txt" new_name="final.txt" base=2 revision=2 outcome=discarded-by-revoke
```

In the same boot, give a new goal the same grant and stage the rename from
the shell. Revoke after review and commit using the token already issued.

```text
goal create --preserve-file /draft.txt
goal grant-file --stage-rename /draft.txt
goal stage-rename /draft.txt final.txt
goal run echo
wait echo
goal diff
goal revoke-file
goal commit
goal audit --publication
cat /final.txt
goal close
```

Revocation prints only `file revoked`. Commit prints `rename committed`,
`cat` prints `keep`, and the sole audit row has `base=2 revision=3 outcome=published`.

### Discard a staged rename

`goal discard` lets the owner withdraw the pending rename and keep the goal
open. At a fresh QEMU prompt, stage a rename, discard it, then let the agent
propose a different name using the same file grant.

```text
write /draft.txt hello
goal create --preserve-file /draft.txt
goal grant-file --stage-rename /draft.txt
goal stage-rename /draft.txt first.txt
goal discard
goal diff
agent run hello --stage-rename final.txt
wait hello
goal diff
```

The first discard prints `rename discarded` and the first diff prints
`no staged changes`. After the agent exits, the second diff shows
`rename /draft.txt -> final.txt` and `ready to commit`. In the same session,
decline that reviewed rename.

```text
goal discard
goal commit
goal discard
goal diff
goal status
goal audit --renames
ls /
cat /draft.txt
goal close
```

Discard invalidates the saved token, so `goal commit` asks for a passing
review. The repeated discard and the diff both print `no staged changes`.
The goal is still completed, and its audit lists both staging decisions.
The file is still named `draft.txt` and contains `hello`. Discard also works
after a review reports `conflict` or `postcondition failed`.

### Inspect rename staging decisions

`goal audit --renames` shows the last four staging decisions for the current
goal. Each row includes the requested basename, the result and the caller.
For an agent, `caller_child` matches the original child handle printed by
`agent run`. At a fresh QEMU prompt, give `hello` read access and ask for a rename.

```text
write /draft.txt hello
goal create
goal grant-file /draft.txt
agent run hello --stage-rename final.txt
wait hello
goal audit --renames
goal diff
goal close
```

The agent exits with status 1. The audit shows `name="final.txt"`,
`denied=no-rename-grant` and `caller=agent`. The diff is empty.
In the same boot, give a new goal permission to stage the rename.

```text
goal create
goal grant-file --stage-rename /draft.txt
agent run hello --stage-rename final.txt
wait hello
goal diff
goal audit --renames
goal revoke-file
goal diff
goal audit --renames
cat /draft.txt
goal close
```

The first diff shows `rename /draft.txt -> final.txt`. Revocation clears
the diff, and both audit reads show the same `staged` decision. The public
file still contains `hello`. A successful `goal commit` also keeps the staging
history available until the owner closes the goal.

### Inspect rename publication decisions

`goal audit --publication` shows the current goal's last four publication
decisions, oldest first. These include commits, explicit discards and renames
discarded by cancellation, file grant revocation or goal expiry. Each row gives
the absolute source path and destination basename captured at staging. `base`
is the RamFS revision at staging. `revision` is the public revision when the
kernel made the decision.
Only the owner can read the history. Closing the goal releases it.

At a fresh QEMU prompt, edit the source after reviewing its rename.

```text
write /draft.txt hello
goal create --preserve-file /draft.txt
goal stage-rename /draft.txt final.txt
goal run echo
wait echo
goal diff
write /draft.txt revised
goal commit
goal discard
goal audit --publication
cat /draft.txt
goal close
```

The commit reports `commit conflict`, and `cat` prints `revised`. The audit
shows two decisions for the same proposal.

```text
source="/draft.txt" new_name="final.txt" base=2 revision=4 outcome=conflict
source="/draft.txt" new_name="final.txt" base=2 revision=4 outcome=discarded
```

In the same boot, stage the rename against the edited file in a fresh goal.

```text
goal create --preserve-file /draft.txt
goal stage-rename /draft.txt final.txt
goal run echo
wait echo
goal diff
goal commit
goal audit --publication
cat /final.txt
goal close
```

The new goal shows one row with `base=4 revision=5 outcome=published`, and
`final.txt` contains `revised`. To see `outcome=review-expired`, leave QEMU
running for at least 61 seconds between `goal diff` and `goal commit`.

## Helper accounting and revocation

### Share a read allowance across two helper launches

Two different helper programs can run at the same time. Each program has
one task slot, so two launches of `grep` run in sequence.
At a fresh `clsh>` prompt, let the shell launch the first `grep` while
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

At the `clsh>` prompt, give `hello` a file, then revoke it while `hello` is
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
