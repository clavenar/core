# Native syscall ABI

The RV64 prototype implements calls 0–88. The caller places the call number in
`a7`, passes up to six arguments in `a0`–`a5`, and executes `ecall`.
The per-call descriptions give the result registers and error values.
[Shared call numbers](../crates/clv-abi/src/lib.rs) · [User-space wrappers](../userspace/libclv/lib.rs)

- [RamFS](#ramfs)
- [Process lifecycle and launch messages](#process-lifecycle-and-launch-messages)
- [Typed requests and replies](#typed-requests-and-replies)
- [Pipes and completion events](#pipes-and-completion-events)
- [Goal and agent calls](#goal-and-agent-calls)
  - [Goal deadlines](#goal-deadlines)
  - [Read-only goal observers](#read-only-goal-observers)
  - [Background goal monitors](#background-goal-monitors)
  - [Delegating one agent resume](#delegating-one-agent-resume)
- [Rename review and publication](#rename-review-and-publication)
- [Goal membership and delegated authority](#goal-membership-and-delegated-authority)
- [Audit records](#audit-records)
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
| 8 `GoalObservation` | 64 bytes | Kernel snapshot requested by the observer through call 85. Call 26 rejects this tag. |
| 9 `AgentResumed` | 32 bytes | Kernel receipt for a supervisor's successful call 87. Call 26 rejects this tag. |

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

## Pipes and completion events

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
| 33 `goal_create` | None | Fresh goal handle, or -1 if the slot is occupied or handles are exhausted |
| 34 `goal_run` | `a0` goal handle, `a1` program selector | Fresh child launch handle, or -1 on failure |
| 35 `goal_status` | `a0` goal handle | State tag, member in `a1`, detail in `a2`, and goal deadline in `a3` (0 for unlimited, absolute guest nanoseconds while armed, all ones for expired) |
| 36 `goal_close` | `a0` goal handle | 0 on release, 1 while running |
| 37 `goal_cancel` | `a0` goal handle | `a0` is 0 for a new or repeated cancellation, 1 for already completed or failed, or -1 on error. On success, `a1` is 1 if this call discarded a staged rename, 0 otherwise. |
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
| 60 `goal_revoke_file` | `a0` goal handle | `a0` is 0 for an owned goal, including a repeat call, or -1 for an invalid handle or caller. On success, `a1` is 1 if this call discarded a rename staged through the grant, 0 otherwise. |
| 61 `goal_read_audit_with_limit` | Same arguments as call 57 | Same count, overwritten count and lifetime read bytes as call 57, with the effective limit identified in each entry |
| 62 `goal_grant_file_to_with_arg` | `a0` goal handle, `a1` shell file handle, `a2` helper selector, `a3` literal address, `a4` literal length (0–64 bytes) | 0 when the file and its argument restriction are attached, -1 on rejection |
| 63 `agent_status_with_definition` | `a0` agent handle, `a1` shell stack destination, `a2` capacity in bytes | Same status registers as call 39, plus a 34-byte definition record in the buffer |
| 64 `agent_invoke_file_tool` | `a0` agent handle or zero for self, `a1` versioned tool ID, `a2` reply reservation, `a3` literal address, `a4` literal length (0–64) | Fresh helper handle on admission, or -1 on rejection |
| 65 `goal_audit_with_tool` | Same arguments as call 52 | Retained delegation count and overwritten count as in call 59, with 104 bytes per entry |
| 66 `goal_stage_rename` | `a0` goal handle, `a1` owner's file handle, `a2` new basename address, `a3` length (1–64 bytes) | 0 when staged, or -1 on rejection |
| 67 `goal_diff` | `a0` goal handle, `a1` stack destination, `a2` capacity in bytes | Total bytes copied in `a0`, source path length in `a1`. An empty diff returns zero in both registers. Failure returns -1 in `a0`. |
| 68 `goal_review` | Same arguments as call 67 | Same diff and lengths as call 67, with a token or zero in `a2` and review status in `a3` |
| 69 `goal_commit` | `a0` goal handle, `a1` review token | 0 on publication, 1 on conflict, 2 on a failed file postcondition, 3 for an expired matching token, or -1 for an invalid caller, goal, token, or unavailable revision |
| 70 `goal_create_preserving_file` | `a0` owner's file handle, `a1` stack address of the six call 48 argument words in order (little-endian RV64), `a2` length in bytes (48) | Fresh goal handle, or -1 on rejection |
| 71 `goal_grant_file_for_rename` | `a0` goal handle, `a1` owner's file handle | 0 when the ready goal receives the grant, or -1 on rejection |
| 72 `agent_stage_rename` | `a0` granted file handle, `a1` new basename address, `a2` length (1–64 bytes) | 0 when the calling agent stages the rename, or -1 on rejection |
| 73 `goal_rename_audit` | `a0` goal handle, `a1` owner's stack destination, `a2` capacity in bytes | Retained staging decision count (0–4), with overwritten count in `a1`, or -1 on failure |
| 74 `goal_discard` | `a0` goal handle | 0 when a pending rename was discarded, 1 when the diff was already empty, or -1 for an invalid caller or goal |
| 75 `goal_review_with_deadline` | Same arguments as call 68 | Same diff, lengths and token. With a nonzero token, `a3` holds the absolute guest deadline in nanoseconds. With a zero token, `a3` holds the call 68 review status. |
| 76 `goal_publication_audit` | `a0` goal handle, `a1` owner's stack destination, `a2` capacity in bytes, `a3` zero-based row index (oldest first) | Retained decision count (0–4) in `a0` and overwritten count in `a1`, or -1 on failure |
| 77 `goal_create_with_rename_scope` | `a0` owner's directory handle, `a1` stack address of the six call 48 words, `a2` length in bytes (48), `a3` preserved-file handle or `NO_RAMFS_HANDLE` | Fresh goal handle, or -1 on rejection |
| 78 `goal_create_with_read_scope` | `a0` owner's read-directory handle, `a1` and `a2` option block as in call 77, `a3` preserved-file handle, `a4` rename-directory handle (each optional handle takes `NO_RAMFS_HANDLE` when absent) | Fresh goal handle, or -1 on rejection |
| 79 `goal_read_decisions` | Same arguments as call 57 | Retained read decision count (0–4), overwritten count in `a1` and lifetime bytes delivered in `a2`, or -1 on failure |
| 80 `goal_publication_audit_with_cancellation` | Same arguments as call 76 | Same row format and result registers as call 76. Includes a pending rename discarded by call 37. |
| 81 `goal_publication_decisions` | Same arguments as call 76 | Same row format and result registers as call 76. Includes renames discarded by calls 37 and 60. |
| 82 `goal_create_with_timeout` | `a0` optional read-directory handle, `a1` address and `a2` length of the 48-byte call 48 option block, `a3` optional preserved-file handle, `a4` optional rename-directory handle, `a5` positive timeout in milliseconds | Fresh goal handle, or -1 on rejection |
| 83 `goal_publication_decisions_with_expiry` | Same arguments as call 76 | Same row format and result registers as call 81. Also includes a rename discarded on goal expiry. |
| 84 `goal_launch_observer` | `a0` open goal handle, `a1` program selector | Fresh shell-owned child handle, or -1 on rejection |
| 85 `goal_observe` | `a0` observer's stack destination, `a1` capacity in bytes | 64 after copying the snapshot and queuing the same record for the parent, or -1 on failure |
| 86 `agent_launch_supervisor` | `a0` suspended agent handle, `a1` program selector | Fresh shell-owned supervisor launch handle, or -1 on rejection |
| 87 `agent_resume_granted` | None | 0 after resuming the bound child and queuing its receipt, or -1 on failure |
| 88 `goal_launch_finish_monitor` | `a0` open goal handle | Fresh shell-owned monitor launch handle, or -1 on rejection |

### Goal deadlines

Call 82 reads the six call 48 option words in little-endian order from the
owner's stack. The timeout starts at goal creation on the guest monotonic
clock. Time spent ready, running, suspended or awaiting publication counts
toward the same deadline. Omit an optional handle with
`NO_RAMFS_HANDLE` (all ones). A supplied handle follows the same ownership,
scope and preservation rules as call 78. Zero milliseconds, an invalid
option or descriptor, or a duration that cannot be represented as an
absolute nanosecond value below `u64::MAX` returns -1 without consuming a
goal handle or retaining a file. Earlier creation calls create untimed goals.

The kernel checks the clock on user trap entry. At or after the deadline,
it processes expiry before dispatching a syscall or resuming a task on a
timer interrupt. A syscall admitted before the deadline runs to completion.
Expiry stops all live goal members, closes their descriptors, releases unused
reply reservations and drops the goal's file grant, preserved input and
staged rename. The owner can still read status and audit records, collect
child results and close the goal. A child that already exited or faulted
keeps its result. A previously published rename stays visible. Call 35
continues to report the deadline until close, including after cancellation
or successful publication.

### Read-only goal observers

The goal owner uses call 84 to start an ordinary child in a separate address
space. The kernel binds a one-use inspection grant to the new launch and the
open goal. Calls 84 and 88 queue the zero-byte `ObserveGoal` request (kind 8).
Reading this assignment through call 25 leaves the inspection grant live.

An observer starts with an empty RamFS handle table. It runs outside the
goal's membership and budgets. The grant works in every state of an open
goal, including after expiry. Calls 84 and 86 share one dedicated slot when
the selected program is `echo`. The kernel reports task 5 as the sender of
its replies. Ordinary `echo` launches, agents and helpers use task 3. Each
launch has its own handle, request and reply mailboxes, and RamFS handle
table. The two slots have separate stacks and Sv39 roots and share the
read/execute image pages. Other program selectors use their ordinary slot.
A busy report slot rejects another `echo` report launch. Reusing either
slot replaces only that slot's previous results.

For a call 84 launch, call 85 takes one atomic sample from the bound goal.
The record contains eight little-endian RV64 words. The first four encode
the state tag, representative child, exit status or fault detail and deadline
as in call 35. The next four encode the budget flags, available reply bytes,
reserved reply bytes and spent reply bytes as in call 44.
The kernel copies the record to the observer and queues identical bytes as a
typed reply under its launch handle. Call 27 reads that reply. The report
uses the observer's mailbox and leaves the goal's reply balance and audit
unchanged.

A successful sample consumes the grant. A short buffer, invalid destination
range or occupied reply mailbox returns -1 before delivery and allows a
retry. Closing the goal, owner exit or observer exit invalidates an unused
call 84 grant. A later goal or launch in the same slot gets a fresh identity.
Goal control, audit, review and publication calls retain their existing
caller checks.

### Background goal monitors

The goal owner uses call 88 to start one background `echo` in task 6. The
monitor has its own stack, Sv39 root and mailboxes, an empty RamFS handle
table and one read-only observation grant. The ordinary `echo` and
synchronous report slots remain available. A second launch returns -1
while the monitor is live or its parent still has a reply or status to collect.

For a ready or running goal the monitor sleeps at its entry point. Call 16
on its handle returns -1 immediately and call 27 returns an empty mailbox.
Once the goal completes, fails or is cancelled, the kernel freezes the
64-byte observation after settling member reservations, then makes the
monitor runnable. Deadline cancellation includes the expired deadline. For
an already terminal goal, the kernel captures the snapshot during call 88.

The monitor uses call 85 to receive and send that frozen record once. The
owner can wait for its exit and receive the typed `GoalObservation` reply
from task 6 by its launch handle. The report survives closing the finished
goal and records the deadline as it stood at capture. Closing a ready goal
before capture cancels the waiting monitor. Its wait result is cancellation
and its reply mailbox is empty. Owner exit stops any live monitor.

### Delegating one agent resume

The owner uses call 86 to give a separate process one resume of a live,
suspended agent. It binds the grant to the agent's original child launch
and queues an empty `ResumeAgent` request (kind 9). An `echo` supervisor
uses the same separate slot as an `echo` observer, so it can resume an
agent that is itself running `echo`. The supervisor starts with an empty
RamFS handle table and runs outside the goal's child limit and reply budget.

Call 87 takes its target from the calling launch's grant. On success, it
makes the child runnable at its saved frame and queues an `AgentResumed`
reply in the supervisor's mailbox for the owner. Call 27 returns the
supervisor launch, goal, agent and resumed child handles as four consecutive
little-endian RV64 words. An occupied mailbox makes call 87 fail before the
resume. The supervisor can retry after its parent collects the pending reply.

A resume by the owner or any supervisor ends every grant for that
suspension. A later suspension of the same agent requires a fresh grant.
Repeating call 40 on an already suspended agent preserves the current
suspension. Goal cancellation, expiry, closure or supervisor exit also
invalidates unused authority. Reviewing and committing a rename still
require the owner.

## Rename review and publication

Calls 66 and 67 are restricted to the goal owner. Call 66 requires a linked
writable RamFS file and an empty staging slot. It rejects an occupied name,
`.`, `..`, and names containing a slash or NUL. Rejection preserves any
pending change. Call 67 copies the original absolute source path followed
immediately by the destination basename, with no separator or terminator.
A stack buffer of `RENAME_DIFF_MAX_BYTES` suffices for any diff. An empty
diff also accepts zero capacity. A short buffer or an invalid nonempty
destination range fails before copying any bytes.

Calls 68 and 75 copy the frozen diff to the owning shell. Once the goal has
completed successfully and the full diff has been delivered, RamFS checks
the original revision, source identity at its original name, and vacant
destination. RamFS checks the source's parent against the goal's rename scope.
When the goal preserves an input file, RamFS also compares the source's
identity, length and contents with that input. A conflict takes precedence
over a failed postcondition.

Call 68 returns the following status codes. Call 75 uses the same codes for a
preview or blocked review and returns the deadline for a passing review.

| Status in `a3` | Meaning | Token in `a2` |
| --- | --- | --- |
| 0 | Preview for an empty diff or a ready, running, failed or cancelled goal. | 0 |
| 1 | Ready to commit. The checks passed. | Nonzero, single-use |
| 2 | Conflict. The base revision, original source or destination has changed. | 0 |
| 3 | Postcondition failed. The source falls outside the rename scope or differs from the preserved input. | 0 |
| 4 | Review unavailable. The kernel cannot prepare publication, represent the deadline or allocate a token. | 0 |

The first passing review starts a 60-second token lifetime on the kernel's monotonic
clock. QEMU `virt` provides a 10 MHz RISC-V time counter, so each tick is
100 nanoseconds. Both review calls reuse a live token. For a timed goal,
call 75 reports the earlier of the token and goal deadlines. If only the
token has expired, a passing review issues a fresh token. The goal deadline
stays fixed. A rejected review clears the token and leaves the frozen
diff available. A failed copy returns -1 before checking the change or
altering its token. Call 67 reads the diff without changing review state.
Discard, cancellation, goal expiry, successful close and owner exit invalidate the token.
Revoking a file grant invalidates the token for a rename staged through that grant.

Call 69 checks the caller, goal state and token. It consumes a matching token
and returns 3 if only the token has expired. The rename remains staged for
review or discard. Once the goal expires, call 69 returns -1 for the former
token and the diff is empty. With a live token, it repeats the RamFS checks
in the syscall that publishes the rename. A failed RamFS check preserves the
public files and staged diff. Success updates the file's name, advances the
revision and clears the diff.
Public file and directory creation or removal, truncating opens, nonempty
writes to linked files and publication advance the revision. A mutation
requiring a new revision fails if the counter is exhausted.

Call 70 uses call 48's option flags, budgets and selector validation. It
requires a descriptor for a linked writable RamFS file. Creation retains
the file and snapshots all its bytes (0–256), independently of the descriptor
cursor. A rejected creation consumes no goal handle or pin. The owner can
close the descriptor immediately. Successful publication, cancellation, goal expiry,
successful close or owner exit releases the retained input.

Call 77 uses the same 48-byte option block as call 70. It binds the directory
behind the owner's descriptor and accepts an optional preserved input with the
same rules as call 70. `NO_RAMFS_HANDLE` is the all-ones RV64 value. Handle
zero is valid. The owner can close both descriptors after creation. Calls 66
and 72 check scope membership by walking the source's directory ancestors
before looking up the destination. File grants continue to govern read access.

Call 78 requires an open directory for the read scope. The owner can close
all supplied descriptors after creation. The directory stays pinned until the
goal closes or its owner exits. On call 6, RamFS walks the open file's current
parent ancestry before checking either byte budget or copying to user memory.
A scope denial returns -1 and preserves the cursor and both balances.

Call 71 requires a linked writable RamFS file and an empty file-grant slot.
The direct agent receives read access and permission to stage a rename.
Ordinary direct children receive read access only. Delegating a file to a
helper requires a delegable grant. Call 72 verifies that the caller is the
goal's live agent and that its open descriptor carries this goal's rename
permission. It uses the same slot, basename checks and publication rules as
call 66. Only the goal owner can review and commit the proposal.

Call 74 accepts the owner's open goal handle in any goal state. It releases
the reference held by the pending rename and invalidates its review token.
The owner can discard a proposal even when its review returned no token.
The public file and RamFS revision stay intact. The goal retains its lifecycle
state, file grant, preserved input, budgets and audit history. A ready or
running goal can stage a replacement under the same rules.

## Goal membership and delegated authority

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
the available balance. Bits 2, 3 and 4 select the request format.

| Format flags | Request kind | Payload |
| --- | --- | --- |
| All clear | `DelegateWithFile` (tag 4) | Two little-endian RV64 words for the helper selector and its reply allowance, followed by 0–64 literal argument bytes |
| Bit 2 only | `PlanIntent` (tag 5) | One intent byte. Intent 1 for `hello` pins `find-greeting@1`. Other legacy intents retain their original delivery behavior. |
| Bit 3 only | `PlanDefinition` (tag 6) | Two unsigned bytes for the intent and revision. The pairs `[1, 1]` and `[1, 2]` select the corresponding `find-greeting` revisions for `hello`. |
| Bit 4 only | `StageRename` (tag 7) | 1–64 bytes for the proposed basename. `hello` presents the name and its startup file descriptor to call 72. |

Unknown flag bits, multiple format bits set, an unreadable buffer or the wrong
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
derived from that grant, including copies already held by helpers. It also
discards any unpublished rename staged through that grant, invalidates
its review token and appends the discard to the history read by call 81.
The discard flag in `a1` describes that same revocation. An owner-staged
rename of the same file keeps its review token, and the goal retains its
preserved input.
Only the owning shell can call it. Each read and revocation runs
to completion on the boot hart. Bytes delivered earlier stay charged. A later
read using a revoked descriptor fails before copying or charging bytes. Other
opens of the same file keep their own authority and cursor.

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

## Audit records

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

Call 79 supplies the history for `goal audit --reads`. It retains the last
four read decisions, including scope denials. Budget decisions use the call
61 encoding. For a scope denial the six words are the caller's child launch
handle, requested capacity, zero, zero, flag 4, and zero. This records the
request without disclosing the file's length. Calls 57 and 61 independently
retain the last four attempts that reached the budget check.

Call 73 lets the owner read the staging decisions from calls 66 and 72.
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
| 4 | `already-staged` — the active goal already holds a pending rename |
| 5 | `invalid-source` — the owner's descriptor is invalid or the source is unavailable as a linked writable file |
| 6 | `goal-inactive` — the goal has completed, failed or been cancelled |
| 7 | `invalid-buffer` — the nonempty name does not fit inside the caller's user stack |
| 8 | `outside-rename-scope` — the file's parent falls outside the goal's directory tree |

Each row reports the first failed check. The kernel copies the bounded
input, checks the goal state and staging slot, then resolves the file
descriptor and validates the source and destination. A failed launch or
shell path open ends before a staging call. Revocation, cancellation, goal
expiry and publication preserve earlier decisions. Closing the goal releases
its history.

Call 76 reads commit decisions made by call 69 with a matching review token
and explicit discards made by call 74. Call 80 also includes a rename
discarded by cancellation (call 37). Call 81 includes all decisions from
call 80 plus any rename discarded by file grant revocation (call 60). Call 83
also includes a rename discarded at the goal deadline. Later decisions can
follow a revocation. A discard caused by cancellation or goal expiry is the
final row. Each view reports its own last four decisions, oldest first,
and its own overwritten count, which saturates at the largest RV64 unsigned
value. A discard, cancellation, revocation or goal expiry adds a row only
when it removes a proposal. Previews, reviews, invalid or replayed tokens
and goal teardown add no rows.

Each 552-byte row starts with five little-endian 64-bit words giving the
outcome, base revision, observed revision, source path length and destination
basename length. The remaining 512 bytes contain the two frozen names back to
back, followed by zero padding. The shell quotes both names and escapes their
bytes. A published rename advances the revision by one.

| Outcome code | Shell output |
| --- | --- |
| 0 | `published` |
| 1 | `conflict` |
| 2 | `postcondition-failed` |
| 3 | `review-expired` |
| 4 | `unavailable` |
| 5 | `discarded` |
| 6 | `discarded-by-cancel` (calls 80, 81 and 83) |
| 7 | `discarded-by-revoke` (calls 81 and 83) |
| 8 | `discarded-by-expiry` (call 83) |

The owner can reuse a single stack buffer to read indices zero through
`count - 1`. Goal expiry can append a row to call 83 between reads. To read a
consistent set of rows, the shell first checks call 35. If the goal has
expired it reads call 83. Otherwise it uses call 81, whose rows and
overwritten count remain stable across expiry. Index zero on an empty
history returns zero and accepts zero capacity.
An invalid index, a short buffer for a retained row, or an invalid destination
range fails before copying any bytes.

Calls 52, 53, 57, 59, 61, 65, 73 and 79 each copy up to four retained entries, oldest
first. A short buffer or an invalid destination range returns -1 before copying
any bytes. Zero capacity succeeds for an empty audit. Reads preserve the entries
and the overwritten count, which saturates at the largest RV64 unsigned value.

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

## Traps and exit

User exceptions print `task N fault` with `scause` (trap cause), `sepc`
(instruction address), and `stval` (additional information, when provided).
The kernel removes the faulting task from the schedule and prints
`fault switch` if another task is runnable. Once the last task stops, the
kernel prints `all tasks exited` and idles.

Supervisor faults and unexpected interrupts print `fatal trap` and stop
execution.
