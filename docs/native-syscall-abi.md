# Native syscall ABI

The RV64 prototype implements 77 calls numbered from 0 through 76.
Kernel 0.0.79 adds durable recovery of the pinned `find-greeting@7` host MCP
search through the broker's SQLite journal.
The caller places the call number in `a7`, passes up to six arguments in
`a0`–`a5`, and executes `ecall`.
[Shared call numbers](../crates/clv-abi/src/lib.rs) · [User-space wrappers](../userspace/libclv/lib.rs)

- [RamFS](#ramfs)
- [Process lifecycle and launch messages](#process-lifecycle-and-launch-messages)
- [Typed requests and replies](#typed-requests-and-replies)
- [Pipes and completion events](#pipes-and-completion-events)
- [Goal and agent calls](#goal-and-agent-calls)
  - [Goal creation](#goal-creation)
  - [Goal deadlines](#goal-deadlines)
  - [Host plan requests](#host-plan-requests)
  - [Read-only goal observers](#read-only-goal-observers)
  - [Background goal monitors](#background-goal-monitors)
  - [Delegating one agent resume](#delegating-one-agent-resume)
- [Rename review and publication](#rename-review-and-publication)
  - [Delegating a rename review](#delegating-a-rename-review)
- [Goal membership and delegated authority](#goal-membership-and-delegated-authority)
  - [Local search checkpoint](#local-search-checkpoint)
  - [Host MCP search checkpoint](#host-mcp-search-checkpoint)
- [Audit records](#audit-records)
  - [Read audit](#read-audit)
  - [Publication audit](#publication-audit)
- [Status and reply accounting](#status-and-reply-accounting)

## Console and timer calls

Call 0 prints `task N user probe` and returns 42 in `a0`. Call 3 returns 1 in `a0` once
the calling task has received a timer interrupt and 0 until then. The hello
program stores its task ID on its stack before waiting for the interrupt. It
checks that value and its saved registers after resuming.

Call 2 writes to the console. It takes a user stack address in `a0` and a byte
count in `a1`. The kernel accepts up to 128 bytes, copies the full range before
writing, and returns the byte count in `a0`. An empty write returns 0. An
oversized write or a range outside the user stack returns -1 without output.
The hello program uses this call to print `Hello from user mode`.

Call 4 polls the console. It returns one UART byte in `a0` when input is ready
and -1 otherwise. At the `clsh>` prompt, type `help` to see the available
commands.

## RamFS

`mkdir /notes` creates a directory, and `cd /notes` enters it. Use `pwd` to
print the current directory and `ls` to list it. Use `ls /` to list the root
or `ls <path>` to list another directory. In `/notes`, `write todo.txt hello`
creates or replaces a file with one line of text, `cat todo.txt` reads it, and
`rm todo.txt` removes it. Use `cd ..` to go up one directory and `rmdir notes`
to remove the empty directory. Use `exit` to stop the shell.

RamFS starts with read-only `/hello.txt` and `/about.txt` and holds up to four
directories besides the root and four writable files of 256 bytes each.
Ordinary writes, unlinks and directory changes live for one boot. A durable
rename journal can restore one or two writable files. In the shell, absolute
paths start at `/` and relative paths start at its current directory.
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

Unlink removes a writable file's name immediately. Open handles keep the
file alive until the last handle closes. A goal with a read-directory scope
requires a linked file for each read.
Recreating the path while an old handle is open uses a separate slot.
Task exit and user faults close the task's handles.

`rmdir` frees a directory slot as soon as it removes the name. Remove all
linked files and subdirectories first. A live task's working directory, an
open directory handle or an open goal's read or rename scope keeps that
directory in use. Attempts to remove `/` or a path ending in `.` or `..` return -1.

## Process lifecycle and launch messages

Call 1 takes an exit status in `a0` and removes the calling task from the
schedule. A task that exits successfully reports
`user exit status=0x0000000000000000`. When the last task stops, the kernel
prints `all tasks exited` and idles. Unknown call numbers return -1 in `a0`.

Call 15 (`spawn`) takes a program selector in `a0` and a launch mode in `a1`.
Selectors 0, 1 and 2 choose `hello`, `echo` and `grep`, respectively. Mode 0 is
a plain launch. Mode 1 attaches [stdin and stdout pipes](#pipes-and-completion-events).
The child starts with no RamFS handles or working directory. The call returns
a fresh, nonzero child handle in `a0` when the task is ready to run and -1 if
the selector or mode is unknown or the program slot is occupied. Child
launches, goals and agents draw distinct handles from one boot-wide sequence.
RamFS allocates its handles independently.

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

## Typed requests and replies

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
| 8 `GoalObservation` | 64 bytes | Kernel snapshot requested by the observer through call 68. Call 26 rejects this tag. |
| 9 `AgentResumed` | 32 bytes | Kernel receipt for a supervisor's successful call 70. Call 26 rejects this tag. |
| 10 `RenameReviewed` | 40 bytes | Kernel receipt for a passing or blocked delegated review through call 75. Call 26 rejects this tag. |
| 11 `HostPlanFailed` | 1 byte | Reason the agent could not obtain an approved host plan. |
| 12 `HostToolFailed` | 1 byte | Reason the agent could not obtain a host tool result. |
| 13 `PublicationObserved` | 64 bytes | Kernel publication snapshot requested by the monitor through call 68. Call 26 rejects this tag. |
| 14 `ReadDenialAlert` | 24 bytes | First [read-scope denial](#read-denial-watch) from the bound `echo` monitor. Call 26 requires the exact kernel-supplied event. |

For `ReadFile`, `hello` opens the requested path through its directory grant
using `open_at`. The child exits with status 0 after sending a successful reply
and status 1 if the request or send fails.

With `a3` set to zero, call 27 (`reply_recv`) takes a user stack destination
in `a0`, its capacity in `a1`, and a child handle owned by the caller in `a2`.
It waits for a reply or child completion. On delivery it returns the byte
count in `a0`, the kernel-recorded
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

## Pipes and completion events

Call 15 in mode 1 gives the shell and its child two pipes identified by the
new launch handle. The child starts with zero in `a1` and `a2` and receives
that handle in `a3`. The parent writes the child's stdin and reads its
stdout. The child reads stdin and writes stdout. Each peer can close its
endpoint of either stream independently.

Call 60 in mode 1 or 2 attaches the same two pipes to a registered helper.
The calling agent or shell owns the parent endpoints. The helper receives
its file handle in `a1`, file kind 2 in `a2` and the pipe handle in `a3`.

Call 28 (`spawn_with_file_pipe`) takes the program selector in `a0` and the
shell's regular file handle in `a1`. The kernel grants the child an independent
read-only file handle using the same rules as call 23. It also creates a
stdout pipe. At startup the child gets its file in `a1`, grant kind 2 in
`a2` and the stdout handle in `a3`. The parent uses the returned launch
handle to read stdout.

| Call | Arguments | Result in `a0` |
| --- | --- | --- |
| 29 `pipe_read` | `a0` launch handle, `a1` stack destination, `a2` capacity | Bytes read in order, or 0 at EOF |
| 30 `pipe_write` | `a0` launch handle, `a1` stack source, `a2` length | Bytes accepted, possibly a prefix |
| 31 `pipe_close` | `a0` launch handle, `a1` stream (0 stdout or 1 stdin) | 0 after closing the caller's endpoint |

Each pipe holds 32 bytes. A nonempty write can sleep while the pipe is full.
A nonempty read can sleep while it is empty and the writer is open. Both
wake when the peer makes progress or closes its endpoint. If the peer is
already blocked on pipe I/O for this launch, a second operation that would
block returns -1 so its caller can service the other stream. Each transfer
validates the full user buffer before moving bytes. Zero-length I/O on an
connected endpoint returns 0 immediately. An invalid handle, caller, buffer
or stream selection returns -1. An unavailable stream also returns -1.

For a goal member's stdout, the kernel checks the available pipe prefix
against the member's reply reservation before making the bytes readable.
An oversized prefix returns -1 and queues no bytes from that write.
Waits and stdin writes leave the reply reservation untouched.

Closing the writer preserves buffered bytes for the reader. Once drained,
further reads return EOF. Closing the reader discards unread bytes and wakes
a blocked writer, which retries and returns -1. Stopping either task closes
its endpoints. If the parent waits for a result or reply while the child
needs pipe I/O, the wait or receive returns -1 so the parent can service
the stream. A queued reply or completed status can still be collected.
A successful relaunch invalidates the previous launch handle and its pipes.

Call 32 (`event_wait`) takes no arguments. Only the shell can collect events.
It receives the oldest pending completion's launch handle in `a0` and its
kind in `a1` (0 for exit, 1 for fault, 2 for cancellation). While the shell
is alive, the kernel records one notification per completed shell-owned launch,
up to five at a time. Agent-launched helpers deliver replies and wait results
to their agent. The boot greeting has no parent launch and produces no event.
Other callers receive -1 in `a0`.

With an empty queue, the call sleeps while a shell-owned child can make progress.
The call returns zero in both registers when there are no outstanding
children or all of them are suspended, waiting for a goal to finish or
blocked writing to a full pipe.
The shell can resume an agent, drain or close a pipe, and retry. Reading an
event preserves the child's reply, buffered pipe data and wait result.
Collecting any of those first also leaves the event available. A successful
relaunch discards the previous handle's unread event. A failed launch preserves it.

## Goal and agent calls

| Call | Arguments | Result in `a0` |
| --- | --- | --- |
| 33 `goal_create` | `a0` request address in the owner's stack, `a1` request size (88 bytes for version 1, 96 bytes for version 2) | Fresh goal handle, or -1 on rejection |
| 34 `goal_run` | `a0` goal handle, `a1` program selector | Fresh child launch handle, or -1 on failure |
| 35 `goal_status` | `a0` goal handle | State tag, member in `a1`, detail in `a2`, and goal deadline in `a3` (0 for unlimited, absolute guest nanoseconds while armed, all ones for expired) |
| 36 `goal_close` | `a0` goal handle | 0 on release, 1 while running |
| 37 `goal_cancel` | `a0` goal handle | `a0` is 0 for a new or repeated cancellation, 1 for already completed or failed, or -1 on error. On success, `a1` is 1 if this call discarded a staged rename, 0 otherwise. |
| 38 `agent_run` | `a0` goal handle, `a1` program selector | Fresh agent handle, with its child launch handle in `a1` |
| 39 `agent_status` | `a0` agent handle | State tag, with the original child in `a1`, detail in `a2` and owning goal in `a3` |
| 40 `agent_suspend` | `a0` agent handle | 0 when suspended, 1 if the agent already finished |
| 41 `agent_resume` | `a0` agent handle, or 0 to resume a recovered checkpoint | 0 when applied, 1 if the agent already finished, or fresh agent, child and goal handles in `a0`–`a2` for a recovered checkpoint. On -1, `a3` carries the resume status. |
| 42 `agent_run_suspended` | `a0` goal handle, `a1` program selector | Fresh agent handle, with its suspended child launch handle in `a1` |
| 43 `goal_budget` | `a0` goal handle | Limit flags, with available bytes in `a1`, reserved bytes in `a2` and spent bytes in `a3` |
| 44 `goal_run_with_reply_budget` | `a0` goal handle, `a1` program selector, `a2` requested reply bytes | Fresh ordinary child launch handle |
| 45 `agent_run_with_reply_budget` | Same as call 44, plus `a3` start mode (0 runnable, 1 suspended) | Fresh agent handle, with the child launch handle in `a1` |
| 46 `goal_grant_file` | `a0` goal handle, `a1` open RamFS file handle owned by the shell | 0 when attached, -1 on rejection |
| 47 `agent_delegate` | `a0` live agent handle, `a1` program selector, `a2` reply bytes to transfer | Fresh helper launch handle, or -1 on rejection |
| 48 `goal_grant_delegable_file` | `a0` goal handle, `a1` shell file handle | 0 when attached with permission for the goal's agent to delegate read access, or -1 on rejection |
| 49 `agent_delegate_with_file` | `a0` live agent handle, `a1` program selector, `a2` reply bytes to transfer | Fresh helper launch handle with read access to the agent's file, or -1 on rejection |
| 50 `goal_audit` | `a0` goal handle, `a1` shell stack destination, `a2` capacity in bytes | Retained delegation count (0–4), with overwritten count in `a1`, or -1 on failure |
| 51 `goal_reply_audit` | Same arguments as call 50 | Retained reply-budget entry count (0–4), with overwritten reply entry count in `a1`, or -1 on failure |
| 52 `goal_grant_file_to` | `a0` goal handle, `a1` shell file handle, `a2` allowed helper program selector | 0 when attached, -1 on rejection, including an unknown selector |
| 53 `spawn_with_file_arg` | `a0` program selector, `a1` shell file handle, `a2` argument address, `a3` argument length | Fresh child handle with a read-only file and an `Argument` request |
| 54 `agent_delegate_with_file_arg` | `a0` agent handle, `a1` program selector, `a2` reply bytes, `a3` argument address, `a4` argument length | Fresh helper handle with the delegated read-only file and an `Argument` request |
| 55 `goal_read_audit` | Same arguments as call 50, plus `a3` [history view](#read-audit) (0 or 1) | Retained read entry count (0–4), overwritten count in `a1` and lifetime bytes delivered in `a2`, or -1 on failure |
| 56 `agent_run_with_request` | `a0` goal, `a1` program, `a2` flags, `a3` optional agent reply allowance, `a4` request address, `a5` request length | Fresh agent handle, with its child launch handle in `a1` |
| 57 `goal_revoke_file` | `a0` goal handle | `a0` is 0 for an owned goal, including a repeat call, or -1 for an invalid handle or caller. On success, `a1` is 1 if this call discarded a batch that used the grant, 0 otherwise. |
| 58 `goal_grant_file_to_with_arg` | `a0` goal handle, `a1` shell file handle, `a2` helper selector, `a3` literal address, `a4` literal length (0–64 bytes) | 0 when the file and its argument restriction are attached, -1 on rejection |
| 59 `agent_status_with_definition` | `a0` agent handle, `a1` shell stack destination, `a2` capacity in bytes | Same status registers as call 39, plus a 34-byte definition record in the buffer |
| 60 `agent_invoke_file_tool` | `a0` agent handle or zero for self, `a1` versioned tool ID, `a2` reply reservation, `a3` literal address, `a4` literal length (0–64), `a5` mode (0 mailbox, 1 raw stdio, 2 MCP stdio) | Fresh helper handle on admission, or -1 on rejection |
| 61 `goal_stage_rename` | `a0` goal handle, `a1` owner's file handle, `a2` new basename address, `a3` length (1–64 bytes) | 0 when staged, or -1 on rejection |
| 62 `goal_commit` | `a0` goal handle, `a1` review token | 0 on publication, 1 on conflict, 2 on a failed file postcondition, 3 for an expired matching token, or -1 for an invalid caller, goal, token, or unavailable revision |
| 63 `goal_grant_file_for_rename` | `a0` goal handle, `a1` owner's file handle | 0 when the ready goal receives the grant, or -1 on rejection |
| 64 `agent_stage_rename` | `a0` granted file handle, `a1` new basename address, `a2` length (1–64 bytes) | 0 when the calling agent stages the rename, or -1 on rejection |
| 65 `goal_rename_audit` | `a0` goal handle, `a1` owner's stack destination, `a2` capacity in bytes | Retained staging decision count (0–4), with overwritten count in `a1`, or -1 on failure |
| 66 `goal_discard` | `a0` goal handle | 0 when a pending rename was discarded, 1 when the diff was already empty, or -1 for an invalid caller or goal |
| 67 `goal_launch_observer` | `a0` open goal handle, `a1` program selector | Fresh shell-owned child handle, or -1 on rejection |
| 68 `goal_observe` | `a0` observer's stack destination, `a1` capacity in bytes | 64 after copying the snapshot and queuing the same record for the parent, or -1 on failure |
| 69 `agent_launch_supervisor` | `a0` suspended agent handle, `a1` program selector | Fresh shell-owned supervisor launch handle, or -1 on rejection |
| 70 `agent_resume_granted` | None | 0 after resuming the bound child and queuing its receipt, or -1 on failure |
| 71 `goal_monitor` | `a2` operation. Arm (0) takes the goal in `a0` and trigger in `a1` (0 finish, 1 publication, 2 read-scope denial). Trigger 2 takes the threshold in `a3` (0 for the default of 1). See [read-denial watch](#read-denial-watch) for operations 1–3. | Fresh shell-owned launch handle or the selected operation's result. Failure returns -1. |
| 72 `goal_launch_rename_reviewer` | `a0` completed goal handle | Fresh shell-owned `echo` launch handle bound to the pending rename, or -1 on rejection |
| 73 `goal_publication_decisions_with_reviewer` | `a0` goal handle, `a1` owner's stack destination, `a2` capacity in bytes, `a3` zero-based row index, `a4` [history view](#publication-audit) (0–3) | Retained row count (0–4) in `a0`, overwritten count in `a1`, reviewer in `a2` and goal handle in `a3`, or -1 in `a0` on failure |
| 74 `goal_rename_diff` | `a0` goal handle, `a1` owner's stack destination, `a2` capacity in bytes, `a3` = 0 for a preview or 1 for a review | Total bytes in `a0`, number of renames in `a1`, approval token or zero in `a2`, deadline or verdict in `a3`. Failure returns -1 in `a0`. |
| 75 `goal_review_batch_granted` | `a0` reviewer's stack destination, `a1` capacity in bytes | Total bytes in `a0`, number of renames in `a1`, verdict (1–4) in `a2`. Failure to deliver returns -1 in `a0`. |
| 76 `host_bridge` | `a0` operations 0 submit plan, 1 poll, or 2 submit tool. Submit takes zeros in `a1` and `a2`. Poll takes a stack address in `a1` and capacity 32 in `a2`. | Submit returns the request ID. Poll returns the length copied or zero while pending. An error returns -1 in `a0` with a reason in `a1`. |

### Host bridge requests

Call 76 accepts live agent launches pinned to revisions 5 through 8 of
`find-greeting`. The kernel selects the request from the pin and the
operation. Revisions 5–7 use the launch handle as the ID of their single
host request. Revision 8 allocates a fresh ID for each of its two requests.
There is one exchange at a time, and calls return without waiting for the
host. Mock plans have a two-second deadline, model plans a 30-second
deadline, and host tool calls a ten-second deadline. An earlier goal
deadline takes priority. Stopping the launch releases the exchange.

The optional VirtIO console carries the following byte stream after the
boot probe. Each frame starts with its total length in bytes.

| Offset | Field |
| --- | --- |
| 0 | Frame length (11–36 bytes) |
| 1 | Version (1) |
| 2 | Kind (1 plan request, 2 plan reply, 3 tool request, 4 tool reply, or 5 checkpoint lookup reply) |
| 3–10 | Request ID as a little-endian 64-bit integer |
| 11 | Request selector or reply status |

The kernel assembles frames across reads, discards complete frames whose
request ID differs from the pending exchange and retires the port if a
length falls outside the frame bounds. The guest client checks the returned
version, kind and request ID.

#### Host plan requests

Plan replies are 12 bytes. Revisions 5 and 6 send 12-byte requests with
selectors 1 and 2 respectively.
The host broker handles the selector for its configured planner.
In a reply, status 1 chooses the pinned search, 0 reports host budget
exhaustion, 2 reports an invalid decision, 3 skips the search and 4
reports an unavailable model. The guest client must accept the action
before it asks the kernel to invoke the registered file tool.

Failure codes in `a1` are 1 for unavailable or denied, 2 for timeout and 3
for an invalid reply. The agent uses the same codes in a `HostPlanFailed`
reply, code 4 for host budget exhaustion, code 5 for a skipped search,
code 6 for an unavailable model and code 7 for a missing tool grant.
The typed reply spends one byte of the
agent's reply allowance.
[Mock planner walkthrough](qemu-walkthroughs.md#run-a-host-backed-mock-plan) · [Model planner walkthrough](qemu-walkthroughs.md#run-a-model-backed-plan)

#### Host MCP tool requests

Revision 7 requires exact tool ID `0x0000000200000001`
(`host.text.search@1`) in the goal's creation policy. The kernel returns
reason 7 for a missing grant. Submit operations 0 and 2 both work for this
revision. The 20-byte tool request has selector 3
and carries the goal ID as a little-endian 64-bit integer at offsets 12–19.
The kernel supplies both handles from the live agent's membership.

The broker selects its configured endpoint, tool and argument. The tool
reply uses status 1 for success, 0 for host call budget exhaustion,
2 for an invalid tool result and 4 for an unavailable tool or unconfirmed
outcome. A successful reply appends up to 20 printable ASCII bytes with
an optional final line feed. Other statuses have no trailing bytes. The
agent forwards successful text as `Matches`, spending its reply allowance,
or sends a one-byte `HostToolFailed` with reason 1, 2, 3, 4 or 7.
[Host MCP walkthrough](qemu-walkthroughs.md#search-a-host-mcp-fixture)

#### Combined plan and tool requests

Revision 8 requires both `text.search@1` (`0x0000000100000001`) and
`host.text.search@1` (`0x0000000200000001`). The kernel checks both before
submitting a request and returns reason 7 when either grant is absent.
Operation 0 submits the model plan once. A validated `search` reply enables
one host tool request with operation 2. The agent runs the local MCP
search before it submits the host tool request.

Both requests are 28 bytes long. The model uses kind 1 and selector 4,
and the tool uses kind 3 and selector 5. Offsets 12–19 carry the goal ID
and offsets 20–27 carry the launch ID, both little-endian. The kernel
supplies these handles and the request ID. The replies use the plan and
tool formats above. The agent sends both matches in one `Matches` reply
of up to 64 bytes.
[Combined walkthrough](qemu-walkthroughs.md#run-ollama-and-both-mcp-searches-in-one-goal)

### Goal creation

Call 33 copies a single request from the owner's stack. Version 1 uses the
first eleven little-endian 64-bit words. Version 2 adds word 11, and version
3 adds word 12.

| Word | Meaning |
| --- | --- |
| 0 | Request version (1, 2 or 3) |
| 1 | Lifetime child limit (1, 2 or 3) |
| 2 | Policy flags |
| 3 | Total reply bytes when flag bit 0 is set |
| 4 | Allowed helper program selector when bit 1 is set, or exact tool ID when bit 4 is set |
| 5 | Total RamFS read bytes when bit 2 is set |
| 6 | Helper RamFS read bytes when bit 3 is set |
| 7 | Optional read-directory descriptor |
| 8 | Optional rename-directory descriptor |
| 9 | Optional first preserved-file descriptor |
| 10 | Timeout in milliseconds, zero for an untimed goal |
| 11 | Optional second preserved-file descriptor (versions 2 and 3) |
| 12 | Optional second exact tool ID, or zero (version 3) |

Bits 1 and 4 are mutually exclusive. With neither set, the goal allows
any helper permitted by its grants. A second tool requires bit 4 and two
distinct registered IDs. A host tool always requires an exact grant. Each
budget word applies only when its
flag is set, and zero then means a zero-byte budget. Bit 5 requires a
supervisor rename review. Bit 6 allows two staged renames. Bit 7 makes the
rename batch durable and requires supervisor review. A durable goal requires
a writable journal with flush support. Use the single-file format
(`CLVRNM01`) for bit 7 alone or the pair format (`CLVRNM02`) for bits 6 and 7.
A supplied rename directory must be the root for either format.

An omitted descriptor is `NO_RAMFS_HANDLE` (all ones). Zero is a valid
descriptor. A read or rename directory must be open in the owner's handle
table. The goal pins the selected directories until close or owner exit.
RamFS checks the source's directory ancestry at rename staging, review and
commit. On call 6 it checks the open file's current parent ancestry before
accounting for bytes or changing the cursor. Each preservation descriptor
must resolve to a distinct linked writable file. The number of protected
files must fit the goal's rename limit. The kernel retains each file and
snapshots all its contents (0–256 bytes), independent of its descriptor
cursor. The owner can close the supplied descriptors after creation.

An unsupported version, wrong size, invalid stack range, unknown flags,
invalid policy, descriptor or timeout, busy slot, or exhausted handles
returns -1 before allocating a goal handle or pinning resources.

### Goal deadlines

A positive timeout starts at creation on the guest monotonic clock.
Time spent ready, running, suspended or awaiting publication counts toward
the same deadline. The kernel rejects a timeout whose absolute deadline in
nanoseconds cannot be represented below `u64::MAX`.

The kernel checks the clock on user trap entry. At or after the deadline,
it processes expiry before dispatching a syscall or resuming a task on a
timer interrupt. A syscall admitted before the deadline runs to completion.
Expiry stops all live goal members, closes their descriptors, releases unused
reply reservations and drops the goal's file grants, preserved files and
staged rename. The owner can still read status and audit records, collect
child results and close the goal. A child that already exited or faulted
keeps its result. A previously published rename stays visible. Call 35
continues to report the deadline until close, including after cancellation
or successful publication.

### Read-only goal observers

The goal owner uses call 67 to start an ordinary child in a separate address
space. The kernel binds a one-use inspection grant to the new launch and the
open goal. Call 67 and finish monitors queue the zero-byte `ObserveGoal`
request (kind 8).
Reading this assignment through call 25 leaves the inspection grant live.

An observer starts with an empty RamFS handle table. It runs outside the
goal's membership and budgets. The grant works in every state of an open
goal, including after expiry. Calls 67 and 69 with `echo` share task 5
with the rename reviewer launched by call 72. The kernel reports task 5 as
the sender of their replies. Ordinary `echo` launches, agents and helpers
use task 3. Each launch has its own handle, request and reply mailboxes,
and RamFS handle table. The two slots have separate stacks and Sv39 roots
and share the read/execute image pages. Other program selectors use their ordinary slot.
A busy report slot rejects another `echo` report launch. Reusing either
slot replaces only that slot's previous results.

For a call 67 launch, call 68 takes one atomic sample from the bound goal.
The record contains eight little-endian RV64 words. The first four encode
the state tag, representative child, exit status or fault detail and deadline
as in call 35. The next four encode the budget flags, available reply bytes,
reserved reply bytes and spent reply bytes as in call 43.
The kernel copies the record to the observer and queues identical bytes as a
typed reply under its launch handle. Call 27 reads that reply. The report
uses the observer's mailbox and leaves the goal's reply balance and audit
unchanged.

A successful sample consumes the grant. A short buffer, invalid destination
range or occupied reply mailbox returns -1 before delivery and allows a
retry. Closing the goal, owner exit or observer exit invalidates an unused
call 67 grant. A later goal or launch in the same slot gets a fresh identity.
Goal control, audit, review and publication calls retain their existing
caller checks.

### Background goal monitors

The goal owner uses call 71 to start one background `echo` in task 6. The
monitor has its own stack, Sv39 root and mailboxes, an empty RamFS handle
table and one read-only observation grant. The ordinary `echo` and
synchronous report slots remain available. A second launch returns -1
while the monitor is live or its parent still has a reply or status to collect.

With trigger 0, the monitor waits for goal execution to finish. Call 16
on its handle returns -1 immediately and call 27 returns an empty mailbox.
Once the goal completes, fails or is cancelled, the kernel freezes the
64-byte observation after settling member reservations, then makes the
monitor runnable. Deadline cancellation includes the expired deadline. For
an already terminal goal, the kernel captures the snapshot during call 71.

The monitor uses call 68 to receive and send that frozen record once. The
owner can wait for its exit and receive the typed `GoalObservation` reply
from task 6 by its launch handle. The report survives closing the finished
goal and records the deadline as it stood at capture. Closing a ready goal
before capture cancels the waiting monitor. Its wait result is cancellation
and its reply mailbox is empty. Owner exit stops any live monitor.

With trigger 1, the monitor receives the zero-byte `ObservePublication`
request (kind 14) and waits for the staged batch to be published or
discarded. A review receipt and a rejected commit leave the batch pending.
Successful commit, discard, cancellation, grant revocation, deadline expiry
or closing a goal with a staged batch can resolve it. The kernel retains the
latest snapshot within the open goal for late registration. Starting a new
batch supersedes the previous snapshot. Closing the goal before a batch
exists cancels the waiting monitor.

Call 68 delivers and queues a `PublicationObserved` reply. It contains
eight little-endian RV64 words. The first four are the goal handle, the
[publication outcome code](#publication-audit), the reviewer register and
the number of renames. For a commit, the reviewer is the one whose approval
was consumed. For a discard, it identifies any approval still attached to
the batch. The reviewer register is zero when absent, all ones for an owner
review, or the supervisor launch handle. The last four words contain the
budget flags and available, reserved and spent reply bytes at resolution.
The frozen report stays collectable after goal close.
[Publication monitor walkthrough](qemu-walkthroughs.md#observe-a-reviewed-publication)

#### Read-denial watch

Trigger 2 arms `echo` for read-scope denials after registration on a ready
or running goal. The kernel validates the threshold (1–16) and gives the
monitor a one-byte `WatchReadDenials` request (kind 15) with that value.
The first `outside-read-scope` decision wakes the monitor. From then on,
the watch retains that first event and a saturating 32-bit denial count.

After receiving the request, `echo` uses call 71, operation 3 to wait for
a newer count. It passes a stack destination in `a0` and capacity in
`a1`. The kernel tracks the last delivered count. The call returns 32
bytes when the count advances and otherwise sleeps until a denial. The
last delivered count must be below the threshold. The four little-endian
RV64 words contain the goal handle, the first denied child's launch
handle, the number of bytes first requested and the current count. Each
sample can cover several denials, so the monitor evaluates at most 16
samples.

`echo` sends a `ReadDenialAlert` through call 26 once its sample reaches
the threshold. The kernel checks the grant, the delivered count and the
exact 24-byte first event before accepting the reply. A successful send
consumes the grant. Before the threshold is reached, the owner's call 16
returns -1 and call 27 returns an empty mailbox, allowing the owner to
start more work. The shell displays `pending`.

Operation 1 disarms a watch before it sends an alert and returns 0 on
success. Goal cancellation, expiry and closure revoke that pending grant
as well. After the owner receives the alert and exit status, operation 2
returns the number of denials following the first. Operations 1 and 2
take the monitor launch handle in `a0` and zero in `a1`. Taking the count
ends the watch. A completed alert and its count remain collectable after
closing the goal.
[Read-denial walkthrough](qemu-walkthroughs.md#watch-for-a-read-scope-denial)

### Delegating one agent resume

The owner uses call 69 to give a separate process one resume of a live,
suspended agent. It binds the grant to the agent's original child launch
and queues an empty `ResumeAgent` request (kind 9). An `echo` supervisor
uses the same separate slot as an `echo` observer, so it can resume an
agent that is itself running `echo`. The supervisor starts with an empty
RamFS handle table and runs outside the goal's child limit and reply budget.

Call 70 takes its target from the calling launch's grant. On success, it
makes the child runnable at its saved frame and queues an `AgentResumed`
reply in the supervisor's mailbox for the owner. Call 27 returns the
supervisor launch, goal, agent and resumed child handles as four consecutive
little-endian RV64 words. An occupied mailbox makes call 70 fail before the
resume. The supervisor can retry after its parent collects the pending reply.

A resume by the owner or any supervisor ends every grant for that
suspension. A later suspension of the same agent requires a fresh grant.
Repeating call 40 on an already suspended agent preserves the current
suspension. Goal cancellation, expiry, closure or supervisor exit also
invalidates unused resume authority.

## Rename review and publication

Call 61 requires a linked writable RamFS file and a free staging slot in
a ready or running goal. Each rename stays in the source directory. The
kernel rejects an occupied name, `.`, `..`, and names containing a slash or
NUL. Goals have one slot by default. Bit 6 of the goal creation flags
allows two slots. Both renames must share a base revision, with distinct
sources and distinct destination paths. A
rejected staging attempt preserves the existing diff.

The goal owner uses call 74 to read the complete diff in staging order.
Each entry begins with two little-endian 16-bit lengths for the absolute
source path and destination basename, followed by those two names back
to back. The result in `a1` gives the number of entries, and
`RENAME_BATCH_DIFF_MAX_BYTES` covers the full buffer. An empty diff accepts
zero capacity and returns zero lengths. A short buffer or an invalid
destination range fails before copying any bytes.

Set `a3` to 0 to preview the diff or to 1 to request review. For a goal
that allows owner review, the kernel checks the whole diff once the goal
has completed successfully and the full copy has succeeded. RamFS verifies
the revision, each source's identity at its original name, each free
destination and the goal's rename scope. The batch must include a rename
of every preserved file with the bytes captured at goal creation. A
conflict takes precedence over a failed postcondition.

A durable batch must stage exactly the journal's file count, and each file
must be at the root. The first durable commit or boot recovery binds the
journal to the saved file identities. Review requires subsequent durable
batches to include the full set of those files.

An approved owner review returns a nonzero, single-use token in `a2` and an
absolute guest deadline in nanoseconds in `a3`. With a zero token, `a3`
contains one of these verdicts.

| Verdict | Meaning |
| --- | --- |
| 0 | Preview. Also returned for an empty diff or a goal that allows owner review and has yet to complete successfully. |
| 2 | Conflict. The base revision, original source or destination has changed. |
| 3 | Postcondition failed. A source falls outside the rename scope, the durable batch is incomplete, or a preserved file is missing from the batch or has different contents. |
| 4 | Review unavailable. The kernel cannot prepare publication, represent the deadline or allocate a token. |
| 5 | Supervisor review required. A passing delegated review issues or renews approval. |

The first passing review starts a 60-second token lifetime on the kernel's monotonic
clock. QEMU `virt` provides a 10 MHz RISC-V time counter, so each tick is
100 nanoseconds. A passing review reuses a live token. For a
timed goal, the owner review and delegated receipt report the earlier of
the token and goal deadlines. When the review token
expires before the goal deadline, a passing review
issues a fresh token. The goal deadline stays fixed. A blocked verdict
(2–4) clears the token and leaves the frozen diff available. A failed copy
returns -1 before checking the changes or altering their token. Preview
mode preserves the existing review state.
Discard, cancellation, goal expiry, successful close and owner exit invalidate the token.
Revoking a file grant discards the whole batch if a rename depended on that
grant and invalidates its token.

Call 62 checks the caller, goal state, token and any requirement for
supervisor approval. It consumes a matching, authorized token
and returns 3 if only the token has expired. The batch remains staged for
review or discard. Once the goal expires, call 62 returns -1 for the former
token and the diff is empty. With a live token, it repeats the RamFS checks
before updating any names. A failed check preserves the public files and
staged diff. Success updates all names in the same syscall, advances the
revision once and clears the diff.

For a durable goal, call 62 validates the approval and prepares the batch
before writing the new basenames and current file bytes to the alternate
journal slot. All sector writes and the flush share a 500 ms guest deadline.
After the flush completes, the call publishes the entire batch and returns 0.
If storage fails before any sector is submitted, the call returns 9. Once
a sector has been submitted, a failure before the flush is confirmed
returns 10 for an unknown storage outcome. Both outcomes consume the
approval and leave the batch staged in the running guest. A journal I/O
error disables further journal writes for that boot.

For a flushed durable commit or an unknown outcome, `a2` returns the
nonzero journal generation and `a3` returns the slot's 32-bit CRC-32,
zero-extended to an RV64 word. Both registers return zero for the other
defined results, including a successful volatile rename. Boot reports the
generation and checksum of the selected record. Compare the two values on the
first boot of the same disk image after the attempt, before another durable
write can reuse the generation.
[Single-file walkthrough](qemu-walkthroughs.md#persist-a-reviewed-rename) · [Pair walkthrough](qemu-walkthroughs.md#persist-two-renames-in-one-commit)

Public file and directory creation or removal, truncating opens, nonempty
writes to linked files and publication advance the revision. A mutation
requiring a new revision fails if the counter is exhausted.

Successful publication, cancellation, goal expiry, close or owner exit
releases the preserved files.

Call 63 retains a linked writable RamFS file while the goal is ready. A
goal with two rename slots can use call 63 twice for distinct files. Both
grants must come from call 63 to form a pair. The agent's issued handles
carry read access and permission to stage a rename. Ordinary direct
children receive read access to the first file. Delegating a file to a
helper requires a delegable grant. Call 64 verifies that the caller is the
goal's live agent and that its open descriptor carries this goal's rename
permission. It uses the same staging limit, basename checks and publication
rules as call 61. By default the owner can review it directly or delegate
one review through call 72. Only the owner can commit it.

Call 66 accepts the owner's open goal handle in any goal state. It releases
the references held by the pending batch and invalidates its review token.
The owner can discard a proposal even when its review returned no token.
The public file and RamFS revision stay intact. The goal retains its lifecycle
state, file grants, preserved files, budgets and audit history. A ready or
running goal can stage a replacement under the same rules.

### Delegating a rename review

Bit 5 or bit 7 of the creation flags requires an `echo` supervisor review
for the goal's lifetime. Review can be combined with file scopes, budgets
and timeouts.
An owner review that copies the pending diff returns the names, a zero
token and verdict 5.
Direct inspection preserves an existing approval and its deadline.
Call 62 requires the approval that the kernel recorded for the batch
when it queued a passing supervisor receipt.

Call 72 requires a successfully completed goal with a pending diff and
an available reporting slot. It binds that specific batch to a fresh
`echo` launch and queues an empty `ReviewRename` request (kind 10).
The reviewer starts with an empty RamFS handle table and runs outside the
goal's membership and byte budgets. Reading the request through call 25
leaves the grant available. Call 24 cannot issue this assignment.

Call 75 copies the whole diff in call 74's format. The buffer must fit
the complete diff before RamFS checks the revision, sources, destinations
and goal postconditions. In `a2`, a passing review returns 1 and a blocked
review returns 2–4. The kernel queues a `RenameReviewed`
reply for the owner under the reviewer's launch handle. Call 27 returns
task 5 as the sender and five little-endian RV64 words in the payload.

| Word | Meaning |
| --- | --- |
| 0 | Reviewer launch handle |
| 1 | Goal handle |
| 2 | RamFS base revision shared by the staged renames |
| 3 | Owner's commit token on success, zero on a blocked review |
| 4 | Absolute guest deadline in nanoseconds on success, or the reason the review was blocked (2–4 above) |

If the destination is too small or invalid, or the reply mailbox is full,
call 75 returns -1 and the reviewer can retry. Delivering either verdict
consumes the grant. A blocked review clears
any earlier token. Removing the pending batch or ending the reviewer launch
invalidates unused authority. A later goal or batch needs a fresh grant.
Only the owner's receipt carries the token. Publication requires the
owner's explicit call 62 and its final RamFS checks.

## Goal membership and delegated authority

The shell owns the goal and controls its agents. Calls 47, 49, 54 and 60 also
accept `CURRENT_AGENT` (zero) when the calling task is itself the goal's live
agent. The kernel resolves its original launch and agent handle. Only the shell
can delegate by supplying an explicit agent handle. An unauthorized caller
or a stale handle returns -1 in `a0`.
The run calls accept a ready goal or a running goal with a place left. They
return -1 for an unknown selector or start mode, a busy program slot, a full
goal, a second agent, insufficient available bytes, or exhausted handles.
A failed launch preserves goal membership, grants, reservations and launch handles.

The kernel binds the child to the goal before its first instruction.
The agent run calls also create and bind the agent in that operation.
Call 56 queues a typed request before the agent starts. Bit 0 starts it
suspended. Bit 1 selects the explicit agent reply allowance in `a3`.
Otherwise the agent inherits the available balance. Bits 2–5 select the
request format. Bit 6 requests a durable checkpoint for the revision 2 or 7
`PlanDefinition` request.

| Format flags | Request kind | Payload |
| --- | --- | --- |
| All clear | `DelegateWithFile` (tag 4) | Two little-endian RV64 words for the helper selector and its reply allowance, followed by 0–64 literal argument bytes |
| Bit 2 only | `PlanIntent` (tag 5) | One intent byte. Intent 1 for `hello` pins `find-greeting@1`. Other legacy intents retain their original delivery behavior. |
| Bit 3 only | `PlanDefinition` (tag 6) | Two unsigned bytes for the intent and revision. Intent 1 with revision 1–8 selects the corresponding `find-greeting` definition for `hello`. |
| Bit 4 only | `StageRename` (tag 7) | 1–64 bytes for the proposed basename. `hello` presents the name and its startup file descriptor to call 64. |
| Bit 5 only | `StageRenamePair` (tag 11) | One byte with the first basename's length, followed by both basenames in grant order. Each basename is 1–64 bytes. The request length determines the second length. |

Unknown flag bits, multiple format bits set, an unreadable buffer or the wrong
length fail before admission. For tag 6 the kernel also verifies that the
definition exists for the selected executable before consuming handles,
reserving bytes or launching the child. The kernel supplies the digest
from its boot image. The agent interprets the workflow and the kernel
checks the live grant when the agent requests delegation.

Tag 11 requires two rename grants and two basenames of valid lengths.
The kernel supplies both task-local handles before scheduling the agent.
Register `a1` holds the first handle,
`a2` holds grant kind 3 and `a3` holds the second handle. `hello` uses
call 64 once per name and fails if either stage is rejected. The staging
audit records both attempts under the agent's launch handle. Review and
commit require successful goal completion.

Call 59 returns lifecycle state and the original agent's definition pin in
one snapshot. The first two bytes hold the intent and revision. The next
32 hold the binary SHA-256 in digest order. An unpinned launch returns
34 zero bytes. Only the shell that owns the agent can inspect it. A stale
handle, a short buffer or an invalid destination returns -1 before copying.

Calls 46, 48, 52 and 58 accept one regular file while the goal is ready.
The kernel retains each granted file independently of the shell's
descriptor, cursor and pathname. Closing or cancelling a ready goal, or
stopping its owner, releases these references. A direct launch uses the
first granted file with a fresh handle at offset zero in `a1`, file kind 2
in `a2` and 0 in `a3`. An agent with a tag 11 request receives both files.
Direct siblings get independent cursors, even after pathname reuse.
The kernel releases the goal's grant references when all places have
been used or the goal finishes. Exit, fault or cancellation closes any
handles the child still holds.

Call 57 withdraws all file grants for the goal and closes every descriptor
derived from them, including copies already held by helpers. If the
batch contains a rename staged through one of these grants, the call discards
the batch and invalidates its review token. Selectors 0 and 1 of call 73
include these discard rows. The discard flag in `a1` describes that same
revocation. The goal retains its preserved files.
Only the owning shell can call it. Each read and revocation runs
to completion on the boot hart. Bytes delivered earlier stay charged. A later
read using a revoked descriptor fails before copying or charging bytes. Other
opens of the same file keep their own authority and cursor.

Revocation succeeds when the goal has no outstanding grant. The owner can
attach a fresh grant while the goal is still ready. After revocation in a
running goal, later direct launches start with an empty handle table, and
requests to delegate the withdrawn file fail.

Call 48 lets the direct agent delegate to any eligible helper. Call 52 binds the
agent's handle to one helper program for delegation. Ordinary direct children
receive read access. Closing the agent's handle ends its ability
to pass that file on, even while the goal still holds a reference.

Call 58 binds the selected program and an owned copy of its literal argument
to the grant. A zero length pins an explicitly empty argument and ignores the
address. A nonempty literal must fit in the caller's stack. Calls 54 and 60 must
present the exact bytes to receive the file. Call 49 omits the argument and
therefore receives denial code 12 on a pinned grant, including an empty pin.
A mismatch allocates no helper, file handle or reply reservation. Revoking a
ready goal's file also releases its pinned argument.

Call 60 resolves the requested file tool in the boot registry before helper
admission. The high 32 bits of the ID select the tool family and the low 32
bits select the exact revision. `text.search@1` has ID `0x0000000100000001`.
An unknown revision fails before any helper launch or budget reservation.

Mode 0 delivers an `Argument` request (tag 3). Mode 1 delivers the same
approved literal as `FileToolStdio` (tag 12) and installs both pipes.
In mode 1, `grep` reads stdin through EOF and checks it against the launch
literal before reading the granted file. It writes matching bytes to stdout.
It reports failures in its exit status and can include an empty typed error
reply. `hello` forwards output after collecting the helper's exit status.
A failed helper's partial output remains private to its parent.

Mode 2 delivers the approved literal as `FileToolMcp` (tag 13) and installs
both pipes. `grep` serves the local MCP session in user space. It exposes
`text.search` for the registered `text.search@1` and compares the decoded
pattern with the launch literal before reading from the granted descriptor.
Each stdout byte, including JSON syntax and newline delimiters, consumes
the helper's reply allowance through call 30. Call 60 rejects unrecognized
modes. [Run the MCP session in QEMU](qemu-walkthroughs.md#invoke-a-registered-file-tool-over-mcp)

Calls 47, 49, 54 and 60 check the goal's delegation rule before checking
the program slot or reserving resources. A program rule requires the helper
to resolve to the allowed executable. A tool rule requires call 60 with the
exact registered ID and revision, even when a program request names the same
executable. Direct goal and agent launches use their ordinary admission rules.
On admission they transfer a byte count from zero up to the agent's remaining
allowance before the helper's first instruction. Both shares count toward
the goal's reserved total. Call 47 starts the helper with zero in `a1`, `a2`
and `a3`. Calls 49, 54 and 60 check the selected helper against the recipient on the
agent's open delegable handle before allocating a read-only handle with an
independent cursor.
The new file handle arrives in `a1` and file kind 2 in `a2`. Register `a3`
carries the pipe handle for call 60 in mode 1 or 2 and zero otherwise.
The kernel commits the file grant, reply reservation and launch together.
The caller receives the helper handle and owns its reply and wait result.
The shell receives events for the launches it issues, including helpers it
launches on behalf of an agent. Across different live parents, a stopped
program slot stays busy while its old parent has a pending reply, wait
result, shell event or open pipe reader. This lets the parent collect the
result even when another caller tries to launch the same program.

Calls 53, 54 and 60 copy up to 64 literal argument bytes into the launch mailbox
before the child can run. The child reads the argument with call 25. The
selected delivery mode determines the tag even for an empty argument.
Invalid arguments fail before admission or reservation.

### Local search checkpoint

With bits 3 and 6 set, call 56 admits `find-greeting@2` to a ready,
untimed goal with two child slots and a writable, empty `CLVCHK01` journal.
The goal needs finite read and reply budgets with at least 18 read bytes
and 36 bytes reserved for the agent. Its file grant must pin the built-in
`/hello.txt` to `grep` with the argument `Hello`, and goal policy must allow
`text.search@1`. Normal helper authorization and charging apply to the search.

When the helper exits successfully, the kernel saves its `Matches` reply
and the actual budget counts, then suspends the parent before forwarding.
An owner waiting for the child can issue another command when the child
parks. The owner resumes the checkpoint. Call 41 with the live agent
handle saves the completed reply and final budget counts before it queues
the reply on that launch. Goal cancellation saves a distinct terminal state
before discarding the reply.

After reboot the shell can use call 41 with `a0` set to zero and no open
goal. The kernel validates the saved definition pin and the reply reservation,
flushes the completion record, and completes the reply step under fresh
handles. On success `a0`, `a1` and `a2` contain the agent, child and goal handles
respectively. The new goal carries forward the recorded read and reply
spending and charges for forwarding the saved result. Its continuation
has reply authority and fresh audit histories. A failed or unconfirmed
write leaves the reply private in the running guest.

To read a completed checkpoint, the shell uses call 27 with mode 1 in
`a3` and `NO_CHILD_HANDLE` in `a2`. Registers `a0` and `a1` supply the user
stack destination and capacity. On success the call copies the saved reply
and returns its length in `a0`, read bytes spent in `a1`, and reply bytes spent
in `a2`. The low byte of `a3` is the reply kind. The high bits are zero
for a local checkpoint or one plus the number of spent host calls for a
host checkpoint. This read can be repeated from a read-only image and adds zero
charges. The kernel requires a completed record with the current definition
pin. Other states, unauthorized callers and invalid buffers return -1.
Existing single-byte consumed records remain terminal. A reboot after the
completion flush can recover the saved reply even if the mailbox step
was interrupted.
[Checkpoint walkthrough](qemu-walkthroughs.md#resume-a-local-search-checkpoint)

### Host MCP search checkpoint

Call 56 with bits 3 and 6 admits `find-greeting@7` with a writable, empty
`CLVHCP01` journal and an available bridge. The goal must be ready and
untimed and have one child slot, a zero-byte read limit, the exact
`host.text.search@1` grant and a finite reply budget with at least 20 bytes
reserved. It takes no file grant and bit 0 must be clear. The kernel holds the
agent suspended for the receipt.

The disk header contains the eight-byte magic, the broker's 16-byte session
ID and zero padding. Before dispatch, the kernel flushes a pending record
(tag 5) with that ID, the 34-byte definition pin, the tool ID, reply limit
and reservation. The last three fields are little-endian 64-bit words.
The checkpoint is bound to one broker session for its lifetime.

The dispatch frame is a 36-byte tool request with selector 6, the
operation ID at offsets 12–27 and the little-endian goal handle at offsets
28–35. Its request ID is the child launch handle. The broker records the
operation and its reservation together before contacting the tool. Call 41
queries the result over a 28-byte tool request with selector 7, the saved
operation ID and a fresh request ID. The broker selects the receipt only
from the session bound to this image.

The lookup reply uses kind 5. Status 5 alone reports an unknown outcome.
A settled reply uses the tool's status, then one byte for calls charged
and the optional result text. Before publishing a typed `Matches` or
`HostToolFailed` reply, the kernel flushes the completed record (tag 6)
with the same fields and the receipt appended. Its byte charge is the
length of the typed reply. A live or recovered resume rechecks the pinned
definition and limited tool scope. After recovery the new goal can only
finish the reply. Pending lookup and unknown outcome return -1 in `a0`
with 1 or 2 in `a3`, respectively. Other errors use 0 in `a3`. An unknown
outcome leaves the checkpoint pending and preserves any outstanding host
reservation.
[Host checkpoint walkthrough](qemu-walkthroughs.md#recover-a-host-mcp-search-checkpoint)

## Audit records

Only the shell that owns the goal can read its audit records. Calls 50,
51, 55 and 65 each copy up to four retained entries, oldest first. A short
buffer or an invalid destination range returns -1 before copying any bytes.
Zero capacity succeeds for an empty audit. Reads preserve the entries and
the overwritten count, which saturates at the largest RV64 unsigned value.

### Delegation audit

Call 50 reads the delegation decisions from calls 47, 49, 54 and 60.
The kernel records accepted and denied requests after authenticating the
caller and resolving the agent handle. Each 104-byte entry starts with nine
little-endian RV64 words followed by 32 digest bytes in SHA-256 order.
The words hold the agent handle, requested program selector, requested reply
bytes, file flag (0 or 1), decision code, helper launch handle, caller,
tool state and requested tool ID. On acceptance, code 0 accompanies the
new child's handle. On denial, the code identifies the first failed check
and the handle is zero. A caller of zero identifies the shell. A nonzero
caller is the calling agent's original child launch handle.

Tool state 0 identifies an ordinary program request and has zeroed tool
fields. State 1 records an unresolved ID with a zero digest. State 2 records
the exact registered ID and the digest of its embedded ELF, including when
the live grant denies the request. For an unresolved ID the program word is
all ones and the unknown-selector denial uses code 2. The shell displays
that denial as `unknown-tool`.

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
| 10 | Invalid argument length, caller buffer or tool mode |
| 11 | The goal does not allow this program as a delegated helper |
| 12 | The file grant requires an argument and the launch omits it or supplies different bytes |
| 13 | The goal requires an exact tool revision and this delegation requests a program or a different registered tool |

### Reply audit

Call 51 returns the decisions recorded when a goal member's `message_send`,
`reply_send` or stdout `pipe_write` reaches the budget check. The kernel
validates the destination and user buffer first. Pipe rows report the bytes
that fit in the available space for that write. Each 32-byte entry contains four
little-endian RV64 words in order. They hold the caller's original child
launch handle, byte count submitted to the budget check, remaining allowance
before the send, and flags. For a sent reply, the flags are 0 for a finite
allowance or 1 for an unlimited allowance (remaining word zero). A value of
2 means the reservation was too small. A valid zero-byte send gets an entry.

### Read audit

Call 55 reads decisions from RamFS call 6. The selector in `a3` chooses the
history. Any other selector returns -1 before copying.

| Selector | Retained decisions |
| --- | --- |
| 0 | Reads that reached the budget check |
| 1 | Budget decisions and directory-scope denials, as shown by `goal audit --reads` |

Each history has its own last four entries and overwritten count. Both
report the same lifetime bytes delivered in `a2`, which saturates at the
largest RV64 unsigned value for unlimited goals.

Each 48-byte entry contains six little-endian RV64 words. For a budget
decision they carry the original child launch handle, requested buffer
capacity, bytes available for this read at the file cursor, prior allowance,
flags and the effective allowance scope. Flags are 0 for a finite successful
read, 1 for an unlimited successful read (prior allowance word zero), and 2
for a budget denial. EOF and valid zero-capacity reads have zero cost and
receive entries. Invalid handles and buffers fail before these checks.

For a helper, the prior allowance is the smaller applicable balance. A scope
of zero identifies the goal allowance and one identifies the helper allowance.
The goal takes precedence when the remaining amounts are equal. A successful
copy charges the goal once and, for a helper, spends the same number of bytes
from its helper allowance. Denials preserve both balances and the cursor.

For a directory-scope denial the words are the caller's child launch handle,
requested capacity, zero, zero, flag 4 and zero. This records the request
without disclosing the file's length.

### Rename staging audit

Call 65 lets the owner read the staging decisions from calls 61 and 64.
The kernel records each attempt after authenticating the goal owner or
its live agent. Each 88-byte row starts with three little-endian RV64 words
for the caller, decision and copied basename length. Caller zero identifies
the shell. An agent row carries the original child launch handle. The final
64 bytes contain the submitted name followed by zero padding. An empty name
has length zero. An oversized name or unreadable buffer has length
`u64::MAX` and a zeroed name field. The shell quotes and escapes copied
bytes and displays `<unavailable>` when the field could not be copied.

| Decision code | Result |
| --- | --- |
| 0 | `staged` |
| 1 | `no-rename-grant` — the agent lacks an open descriptor with this goal's staging permission |
| 2 | `invalid-basename` — the name is empty, exceeds 64 bytes, is `.` or `..`, or contains a slash or NUL |
| 3 | `destination-exists` — the name already exists in the source directory |
| 4 | `already-staged` — the active goal has filled its staging slots |
| 5 | `invalid-source` — the owner's descriptor is invalid or the source is unavailable as a linked writable file |
| 6 | `goal-inactive` — the goal has completed, failed or been cancelled |
| 7 | `invalid-buffer` — the nonempty name does not fit inside the caller's user stack |
| 8 | `outside-rename-scope` — the source falls outside the goal's directory tree or its durable file scope |
| 9 | `base-changed` — the additional rename comes from a different RamFS revision |
| 10 | `source-already-staged` — the source file already has a staged rename |
| 11 | `destination-already-staged` — another staged rename targets the same destination path |

Each row reports the first failed check. The kernel copies the bounded
input, checks the goal state and staging slot, then resolves the file
descriptor and validates the source and destination. A failed launch or
shell path open ends before a staging call. Revocation, cancellation, goal
expiry and publication preserve earlier decisions. Closing the goal releases
its history.

### Publication audit

Call 73 reads an indexed publication row. Each view includes commit
decisions made with a matching review token and explicit discards.
The selector in `a4` controls which other discards appear.

| Selector | Additional discards |
| --- | --- |
| 0 | Cancellation and file grant revocation |
| 1 | Cancellation, file grant revocation and goal expiry |
| 2 | — |
| 3 | Cancellation |

Later decisions can follow a revocation. Discards caused by cancellation
or goal expiry form the final rows. Each view reports its last four rows,
oldest first, and its own overwritten count, which saturates at the largest
RV64 unsigned value. A discard, cancellation, revocation or goal expiry
adds a row for each proposal it removes. An attempted commit with a
matching token records one row per staged rename with a shared outcome,
base revision, observed revision and reviewer. Previews, reviews, invalid
or replayed tokens and goal teardown add no rows.

Each 552-byte row starts with five little-endian 64-bit words giving the
outcome, base revision, observed revision, source path length and destination
basename length. The remaining 512 bytes contain the two frozen names back to
back, followed by zero padding. The shell quotes both names and escapes their
bytes. A published batch advances the revision by one.

| Outcome code | Shell output |
| --- | --- |
| 0 | `published` |
| 1 | `conflict` |
| 2 | `postcondition-failed` |
| 3 | `review-expired` |
| 4 | `unavailable` |
| 5 | `discarded` |
| 6 | `discarded-by-cancel` |
| 7 | `discarded-by-revoke` |
| 8 | `discarded-by-expiry` |
| 9 | `storage-failed` |
| 10 | `storage-outcome-unknown` |

Register `a2` contains the supervisor's launch handle for a delegated
approval, all ones for an owner's direct approval, or zero for a discard
or an empty history.
Register `a3` returns the goal handle on success. The identity comes from
the approval the kernel consumed at commit, including conflicts, expired
reviews and unconfirmed storage outcomes. A passing review
replaces the stored reviewer even when it reuses a live token. Another launch
in the same task slot leaves earlier attribution intact. The shell prints the
goal handle and reviewer on commit rows, using `reviewer=owner` for a direct review.

The owner can reuse a single stack buffer to read indices zero through
`count - 1`. To read a consistent set of rows, the shell first checks
call 35 and uses selector 1 if the goal has expired. Selector 0 keeps its
rows and overwritten count stable across expiry. Index zero on an empty
history returns zero and accepts zero capacity. An invalid view, index,
destination range or short buffer fails before copying any bytes.

## Status and reply accounting

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

Call 35 reports the deadline in `a3` independently of the execution state
in `a0`–`a2`. Expiry before the first launch produces `Cancelled` with no
child. A goal whose members had already finished can report `Completed` or
`Failed` together with an expired deadline.

If an agent exits or faults, the kernel records its completion and cancels
all its live helpers in launch order before scheduling another task. Each
helper releases its unused reply reservation. The shell can collect events
and wait results for the agent and for each helper the shell launched.
An agent-issued helper's results belong to that agent. `agent_status`
retains the agent's own exit status or fault cause.

The owner's direct suspend and resume calls (40 and 41) succeed when a live
agent is already in the requested state. A paused child keeps its slot and
any queued reply. Suspension and resumption produce no completion event.

`goal_cancel` stops each live member by its original launch handle in launch
order. The kernel queues one event per cancelled shell-owned child. Repeating
cancellation returns 0 even after another launch reuses a slot. Cancelling a
ready goal produces no event.

Execution and publication have separate results. A completed or failed goal
retains its execution status. If it still has a staged rename, the same
cancellation removes the proposal, invalidates its review token and returns
1 in `a1`. A second cancellation returns 0 in `a1`. If commit ran first,
the published file remains visible and cancellation also returns 0 in `a1`.

Goal and agent status reads are repeatable and survive reuse of the child's
program slot. Closing the goal releases its agent identity. The child's pending
reply, event and wait status remain available under the usual launch rules.
If the shell exits or faults, the kernel cancels every live member by its
original launch handle, even when suspended. It releases the goal and its agent
before scheduling another task. Children launched outside the goal can still
run to completion, including a newer launch in the same program slot.

The kernel reserves each child's reply allowance when it binds the launch.
Calls 34, 38 and 42 reserve all available bytes. Calls 44 and 45 can also
delegate a finite allowance from an unlimited goal. With a finite goal,
`available + reserved + spent` always equals the original limit. Call 43
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

## Traps and exit

User exceptions print `task N fault` with `scause` (trap cause), `sepc`
(instruction address), and `stval` (additional information, when provided).
The kernel removes the faulting task from the schedule and prints
`fault switch` if another task is runnable. Once the last task stops, the
kernel prints `all tasks exited` and idles.

Supervisor faults and unexpected interrupts print `fatal trap` and stop
execution.
