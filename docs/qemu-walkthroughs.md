# QEMU walkthroughs

These examples use the native `clsh>` shell. Start each independent example
with a fresh boot unless it explicitly continues the previous one. Goal
handles and ordinary RamFS changes live for one boot.
[Build and boot the kernel](../README.md#build-and-run-the-rv64-kernel)

- [Boot and shell](#boot-and-shell)
  - [Exchange an Ethernet frame over VirtIO-net](#exchange-an-ethernet-frame-over-virtio-net)
  - [Probe a VirtIO block device](#probe-a-virtio-block-device)
  - [Recover a VirtIO block marker after reboot](#recover-a-virtio-block-marker-after-reboot)
  - [Recover a previous journal record](#recover-a-previous-journal-record)
  - [Probe the QEMU host bridge](#probe-the-qemu-host-bridge)
- [Goals and file grants](#goals-and-file-grants)
  - [Restart a faulted goal monitor](#restart-a-faulted-goal-monitor)
- [Requests, replies and file streams](#requests-replies-and-file-streams)
- [Delegation and read limits](#delegation-and-read-limits)
  - [Watch for a read-scope denial](#watch-for-a-read-scope-denial)
- [Mock planning and registered tools](#mock-planning-and-registered-tools)
  - [Resume a local search checkpoint](#resume-a-local-search-checkpoint)
  - [Run a host-backed mock plan](#run-a-host-backed-mock-plan)
  - [Run a model-backed plan](#run-a-model-backed-plan)
  - [Search a host MCP fixture](#search-a-host-mcp-fixture)
  - [Recover a host MCP search checkpoint](#recover-a-host-mcp-search-checkpoint)
  - [Run Ollama and both MCP searches in one goal](#run-ollama-and-both-mcp-searches-in-one-goal)
  - [Replay a combined goal offline](#replay-a-combined-goal-offline)
- [Reviewed renames](#reviewed-renames)
  - [Review a staged rename through echo](#review-a-staged-rename-through-echo)
  - [Require a supervisor review before commit](#require-a-supervisor-review-before-commit)
  - [Persist a reviewed rename](#persist-a-reviewed-rename)
  - [Persist two renames in one commit](#persist-two-renames-in-one-commit)
  - [Trace a rename commit to its reviewer](#trace-a-rename-commit-to-its-reviewer)
  - [Publish two renames in one commit](#publish-two-renames-in-one-commit)
  - [Observe a reviewed publication](#observe-a-reviewed-publication)
- [Helper accounting and revocation](#helper-accounting-and-revocation)

## Boot and shell

In an ANSI terminal, the shell displays the `clsh>` prompt and help headings
in green. `goal commit` prints `rename committed` in green and
`durable commit outcome unknown` in yellow. Review and goal deadline
expiry also appear in yellow. A commit conflict or a durable storage
failure before submission appears in red. The shell resets the color after
each prompt, heading, or message.

Type `help` for the full command reference. It groups commands by task, shows
their arguments beside short descriptions, and lists options under the command
that accepts them. Type `goal` or `agent` on its own to see just that section.

The build links the shell, the `netd` service and each registered program
as separate RV64 ELFs and embeds them in the kernel. To add a shell program,
put its entry point in `userspace/<name>/main.rs` and append its name to the
program registry. The build, shell and kernel use that registry for names
and stable launch selectors. A versioned file tool maps an exact tool ID
to one of those programs. [Program registry](../crates/clv-abi/src/program.rs) · [File tool registry](../crates/clv-abi/src/tool.rs)
The loader accepts static, little-endian ELF64 executables with one
read/execute segment of at most 64 KiB at `0x00400000`. The kernel build
supplies this limit from `clv-abi`. When linking a program directly with
`userspace/user.ld`, pass `--defsym=USER_IMAGE_MAX_BYTES=65536` to the linker.
The loader allocates one to sixteen pages based on the segment's memory size
(`p_memsz`) and zero-fills any bytes beyond the file data. Boot reports each
program's validated entry address. If loading fails, it prints
`User ELF load failed` with the program name and idles.

The kernel activates paging, prints `Sv39 paging enabled`, and starts `hello`
as task 1 and the shell as task 2. Task 3 runs ordinary `echo` launches,
including agents and helpers. Task 4 runs `grep`. Task 5 runs `echo` for
immediate goal observations, delegated resumes and rename reviews. Task 6
runs `echo` for a background goal monitor. When a NIC initializes, task 7
runs `netd` with its MAC address. Tasks 3, 5 and 6 share the
read/execute pages of one ELF. Each task receives its ID in `a0` and has
its own Sv39 root and read/write, non-executable stack. `netd` has a 16 KiB
stack in the standard build and 24 KiB with `http-probe`. The other tasks
have 4 KiB each.
The stack starts at `0x00402000` for one- and two-page images and immediately
after the image for larger programs. The initial stack pointer is at the
top of the stack. Nonempty syscall buffers must fit within the calling
task's stack. The kernel, UART and attached VirtIO MMIO mappings are
shared in supervisor mode. The boot pool has room for five full-size images,
seven task stacks and the page tables. It holds 114 frames in the standard
build and 116 with `http-probe`. The pool, kernel and 64 KiB supervisor
stack fit within one aligned 2 MiB physical region.

OpenSBI timer interrupts drive round-robin scheduling. A `timer switch` line
shows which task was interrupted and which task will run next. The kernel
starts a fresh quantum when the timer fires or a stopped task is replaced.

### Exchange an Ethernet frame over VirtIO-net

The kernel reads the MAC address from an attached modern VirtIO-net device,
posts a receive buffer and starts the dedicated `netd` task. The timer wakes
the service on receive or transmit readiness or at its requested receive
deadline. A receive copies a ready frame to the service's stack and reposts
the buffer, or returns zero bytes at the deadline. Both results carry
the current monotonic time.
A transmit still pending at its 500 ms deadline causes the driver to request
reset and wake `netd` so its call can fail. DMA buffers remain in supervisor
memory.

`netd` uses smoltcp to answer ARP and IPv4 ICMP echo requests at
`192.0.2.2/24`. It echoes unicast UDP datagrams sent to `192.0.2.2:49152`.
The UDP socket has one receive slot and one transmit slot, each sized for
payloads up to 1,472 bytes. A new valid datagram can replace a reply that
is still waiting on ARP. On `192.0.2.2:49153`, `netd` echoes one TCP stream
at a time with 1 KiB buffers in each direction. The service also answers the
60-byte Ethernet probe `CLVNET01` with `CLVNET02` on experimental EtherType
`0x88b5`.
[Frame I/O syscall](native-syscall-abi.md#network-frame-io) · [smoltcp interface](https://docs.rs/smoltcp/0.14.0/smoltcp/iface/struct.Interface.html#method.poll_ingress_single) · [VirtIO network specification](https://docs.oasis-open.org/virtio/virtio/v1.2/virtio-v1.2.html)

After building the kernel, paste this block at the Linux prompt in `core`.
It connects each QEMU boot to a private Unix datagram socket, checks the
shell and stops the VM after the checks. [QEMU datagram backend options](https://www.qemu.org/docs/master/system/invocation.html)

```sh
python3 - <<'PY'
import os
from select import select
from socket import SOCK_DGRAM, inet_aton, socketpair
from struct import iter_unpack, pack, unpack
from subprocess import PIPE, STDOUT, Popen
import sys
from time import monotonic

guest_mac = bytes.fromhex("52 54 00 12 34 56")
host_mac = bytes.fromhex("02 00 00 00 00 01")
guest_ip = inet_aton("192.0.2.2")
host_ip = inet_aton("192.0.2.1")
udp_echo_port = 49152
tcp_echo_port = 49153
probe_port = 49154
http_port = 8080
tcp_fin, tcp_syn, tcp_rst, tcp_ack = 0x01, 0x02, 0x04, 0x10
prompt = b"\x1b[32mclsh> \x1b[0m"

def ethernet_frame(destination, source, ethertype, payload):
    return (destination + source + pack("!H", ethertype) + payload).ljust(60, b"\0")

def checksum(data):
    if len(data) % 2:
        data += b"\0"
    total = sum(word for (word,) in iter_unpack("!H", data))
    while total >> 16:
        total = (total & 0xffff) + (total >> 16)
    return (~total) & 0xffff

def with_checksum(packet, offset):
    return packet[:offset] + pack("!H", checksum(packet)) + packet[offset + 2:]

def check_arp(host):
    format_fields = pack("!HHBB", 1, 0x0800, 6, 4)
    request = format_fields + pack("!H", 1) + host_mac + host_ip + bytes(6) + guest_ip
    host.send(ethernet_frame(b"\xff" * 6, host_mac, 0x0806, request))
    reply = format_fields + pack("!H", 2) + guest_mac + guest_ip + host_mac + host_ip
    if host.recv(2048) != ethernet_frame(host_mac, guest_mac, 0x0806, reply):
        raise RuntimeError("ARP did not resolve 192.0.2.2 to the guest MAC")

def send_ipv4(host, protocol, identifier, payload):
    header = pack("!BBHHHBBH4s4s",
                  0x45, 0, 20 + len(payload), identifier, 0, 64, protocol, 0, host_ip, guest_ip)
    host.send(ethernet_frame(guest_mac, host_mac, 0x0800, with_checksum(header, 10) + payload))

def receive_ipv4(host, protocol, payload_length=None):
    frame = host.recv(2048)
    if frame[:14] != host_mac + guest_mac + pack("!H", 0x0800):
        raise RuntimeError("Expected a guest Ethernet frame carrying IPv4")
    ip = frame[14:]
    total_length = int.from_bytes(ip[2:4], "big")
    if (not 20 <= total_length <= len(ip) or
            (payload_length is not None and total_length != 20 + payload_length) or
            ip[0] != 0x45 or ip[9] != protocol or ip[12:20] != guest_ip + host_ip or
            checksum(ip[:20]) != 0):
        raise RuntimeError("Invalid IPv4 echo reply header")
    return ip[20:total_length]

def check_echo(host, sequence, payload):
    identifier = 0x1234 + sequence
    request = with_checksum(pack("!BBHHH", 8, 0, 0, identifier, sequence) + payload, 2)
    send_ipv4(host, 1, sequence, request)
    reply = receive_ipv4(host, 1, len(request))
    if (reply[:2] != b"\0\0" or reply[4:] != pack("!HH", identifier, sequence) + payload or
            checksum(reply) != 0):
        raise RuntimeError(f"Invalid ICMP echo reply for sequence {sequence}")

def transport_checksum(protocol, source, destination, segment):
    header = source + destination + pack("!BBH", 0, protocol, len(segment))
    return checksum(header + segment)

def check_udp(host, port, payload):
    length = 8 + len(payload)
    request = pack("!HHHH", port, udp_echo_port, length, 0) + payload
    # UDP encodes a computed zero checksum as all ones.
    request_checksum = transport_checksum(17, host_ip, guest_ip, request) or 0xffff
    send_ipv4(host, 17, port, request[:6] + pack("!H", request_checksum) + payload)
    reply = receive_ipv4(host, 17, length)
    source, destination, reply_length, reply_checksum = unpack("!HHHH", reply[:8])
    if ((source, destination, reply_length) != (udp_echo_port, port, length) or
            not reply_checksum or transport_checksum(17, guest_ip, host_ip, reply) != 0 or
            reply[8:] != payload):
        raise RuntimeError(f"Invalid UDP echo reply for port {port}")

def send_tcp(host, port, sequence, acknowledgement, flags, payload=b"", window=4096,
             *, guest_port=tcp_echo_port):
    segment = pack("!HHIIBBHHH", port, guest_port, sequence, acknowledgement,
                   5 << 4, flags, window, 0, 0) + payload
    value = transport_checksum(6, host_ip, guest_ip, segment)
    send_ipv4(host, 6, sequence & 0xffff, segment[:16] + pack("!H", value) + segment[18:])

def receive_tcp(host, port, *, guest_port=tcp_echo_port):
    segment = receive_ipv4(host, 6)
    if len(segment) < 20 or transport_checksum(6, guest_ip, host_ip, segment) != 0:
        raise RuntimeError("Invalid TCP segment or checksum")
    source, destination, sequence, acknowledgement, offset, flags, _, _, _ = unpack(
        "!HHIIBBHHH", segment[:20])
    header_length = (offset >> 4) * 4
    if (source, destination) != (guest_port, port) or not 20 <= header_length <= len(segment):
        raise RuntimeError("Invalid TCP ports or header length")
    control = flags & (tcp_syn | tcp_ack | tcp_fin | tcp_rst)
    return sequence, acknowledgement, control, segment[header_length:]

def check_tcp(host, guest):
    port = 50010
    client_sequence = 0x12345678
    send_tcp(host, port, client_sequence, 0, tcp_syn)
    server_sequence, acknowledgement, flags, payload = receive_tcp(host, port)
    client_sequence += 1
    if (acknowledgement, flags, payload) != (client_sequence, tcp_syn | tcp_ack, b""):
        raise RuntimeError("Expected TCP SYN-ACK")
    server_sequence = (server_sequence + 1) & 0xffffffff

    # The first data segment also acknowledges the handshake.
    for index, payload in enumerate((b"TCP ", b"echoes ", b"a byte stream.")):
        send_tcp(host, port, client_sequence, server_sequence, tcp_ack, payload)
        client_sequence += len(payload)
        expected = (server_sequence, client_sequence, tcp_ack, payload)
        if receive_tcp(host, port) != expected:
            raise RuntimeError("TCP echo returned different stream bytes or sequence numbers")
        if index == 0:
            # Keep the link silent while the guest retransmits the unacknowledged bytes.
            check_shell(guest)
            if receive_tcp(host, port) != expected:
                raise RuntimeError("Expected retransmission of the unacknowledged TCP bytes")
        server_sequence = (server_sequence + len(payload)) & 0xffffffff

    send_tcp(host, port, client_sequence, server_sequence, tcp_fin | tcp_ack)
    client_sequence += 1
    reply = receive_tcp(host, port)
    if reply == (server_sequence, client_sequence, tcp_ack, b""):
        reply = receive_tcp(host, port)
    if reply != (server_sequence, client_sequence, tcp_fin | tcp_ack, b""):
        raise RuntimeError("Expected an orderly TCP close")
    send_tcp(host, port, client_sequence, (server_sequence + 1) & 0xffffffff, tcp_ack)
    check_shell(guest)

def answer_guest_arp(host):
    format_fields = pack("!HHBB", 1, 0x0800, 6, 4)
    request = host.recv(2048)
    expected = b"\xff" * 6 + guest_mac + pack("!H", 0x0806)
    expected += format_fields + pack("!H", 1) + guest_mac + guest_ip
    if request[:32] != expected or request[38:42] != host_ip:
        raise RuntimeError("Expected the guest to resolve 192.0.2.1 by ARP")
    reply = format_fields + pack("!H", 2) + host_mac + host_ip + guest_mac + guest_ip
    host.send(ethernet_frame(guest_mac, host_mac, 0x0806, reply))

def check_http(host, guest, case):
    answer_guest_arp(host)
    guest_sequence, _, flags, data = receive_tcp(host, http_port, guest_port=probe_port)
    if (flags, data) != (tcp_syn, b""):
        raise RuntimeError("Expected the guest to initiate TCP to port 8080")
    guest_sequence = (guest_sequence + 1) & 0xffffffff
    server_sequence = 0x76543210
    send_tcp(host, http_port, server_sequence, guest_sequence, tcp_syn | tcp_ack,
             guest_port=probe_port)
    server_sequence += 1

    expected = b"GET /probe HTTP/1.1\r\nHost: 192.0.2.1:8080\r\nConnection: close\r\n\r\n"
    request = bytearray()
    while len(request) < len(expected):
        sequence, acknowledged, flags, data = receive_tcp(host, http_port, guest_port=probe_port)
        if (sequence, acknowledged, flags) != (guest_sequence, server_sequence, tcp_ack):
            raise RuntimeError("Unexpected TCP segment while receiving HTTP request")
        request.extend(data)
        if not expected.startswith(request):
            raise RuntimeError(f"Unexpected HTTP request {request!r}")
        guest_sequence = (guest_sequence + len(data)) & 0xffffffff

    if case == "silent":
        send_tcp(host, http_port, server_sequence, guest_sequence, tcp_ack, guest_port=probe_port)
        wait_for_serial(guest, b"netd HTTP probe timed out", timeout=8)
        return

    body = b"Hello from native HTTP\n"
    chunks = (
        b"HTTP/1.1 200 OK\r\nContent-Len",
        f"gth: {len(body)}\r\nConnection: close\r\n\r\n".encode() + body[:5],
        body[5:],
    )
    if case == "truncated":
        chunks = chunks[:-1]
    for index, chunk in enumerate(chunks):
        final = index == len(chunks) - 1
        flags = tcp_ack | (tcp_fin if final else 0)
        send_tcp(host, http_port, server_sequence, guest_sequence, flags, chunk,
                 guest_port=probe_port)
        server_sequence += len(chunk) + int(final)
        if not final:
            reply = receive_tcp(host, http_port, guest_port=probe_port)
            if reply != (guest_sequence, server_sequence, tcp_ack, b""):
                raise RuntimeError("Expected an ACK for the HTTP response segment")

    if case == "truncated":
        wait_for_serial(guest, b"netd HTTP probe truncated response")
        return
    reply = receive_tcp(host, http_port, guest_port=probe_port)
    if reply == (guest_sequence, server_sequence, tcp_ack, b""):
        reply = receive_tcp(host, http_port, guest_port=probe_port)
    if reply != (guest_sequence, server_sequence, tcp_fin | tcp_ack, b""):
        raise RuntimeError("Expected the guest to close the HTTP connection")
    send_tcp(host, http_port, server_sequence, (guest_sequence + 1) & 0xffffffff,
             tcp_ack, guest_port=probe_port)
    wait_for_serial(guest, b"netd HTTP probe received Hello from native HTTP")

def wait_for_serial(guest, expected, timeout=5):
    serial = bytearray()
    deadline = monotonic() + timeout
    while expected not in serial:
        remaining = deadline - monotonic()
        if remaining <= 0 or not select([guest.stdout], [], [], remaining)[0]:
            raise RuntimeError(f"Timed out waiting for {expected!r}")
        chunk = os.read(guest.stdout.fileno(), 4096)
        if not chunk:
            raise RuntimeError(f"QEMU stopped while waiting for {expected!r}")
        sys.stdout.buffer.write(chunk)
        sys.stdout.buffer.flush()
        serial.extend(chunk)
    return serial

def wait_for_shell(guest):
    return wait_for_serial(guest, prompt)

def check_shell(guest):
    guest.stdin.write(b"cat /hello.txt\n")
    guest.stdin.flush()
    if b"Hello from RamFS" not in wait_for_shell(guest):
        raise RuntimeError("The shell could not read /hello.txt")

def boot(exercise_network=False, *, http_case=None):
    target = "target/http-probe" if http_case is not None else "target"
    image = f"{target}/riscv64gc-unknown-none-elf/release/clavenar-kernel"
    host, nic = socketpair(type=SOCK_DGRAM)
    with host, nic:
        fd = nic.fileno()
        guest = Popen([
            "qemu-system-riscv64", "-machine", "virt", "-smp", "1", "-m", "128M",
            "-bios", "default", "-display", "none", "-monitor", "none",
            "-serial", "stdio", "-nic", "none",
            "-kernel", image,
            "-global", "virtio-mmio.force-legacy=off",
            "-netdev", f"dgram,id=net0,local.type=fd,local.str={fd}",
            "-device", "virtio-net-device,netdev=net0,mac=52:54:00:12:34:56",
        ], stdin=PIPE, stdout=PIPE, stderr=STDOUT, pass_fds=(fd,))
        nic.close()
        try:
            boot_log = wait_for_shell(guest)
            if b"virtio net ready mac=52:54:00:12:34:56" not in boot_log:
                raise RuntimeError("VirtIO-net did not report the configured MAC")
            host.settimeout(2)
            check_shell(guest)
            if http_case is not None:
                check_http(host, guest, http_case)
                check_shell(guest)
            elif exercise_network:
                request = ethernet_frame(guest_mac, host_mac, 0x88b5, b"CLVNET01")
                expected = ethernet_frame(host_mac, guest_mac, 0x88b5, b"CLVNET02")
                for _ in range(3):
                    host.send(request)
                    if host.recv(2048) != expected:
                        raise RuntimeError("The Ethernet reply differs from the expected frame")
                    check_shell(guest)
                check_arp(host)
                check_shell(guest)
                payloads = (b"", b"hello from host", bytes(index % 256 for index in range(1472)))
                for sequence, payload in enumerate(payloads, start=1):
                    check_echo(host, sequence, payload)
                    check_shell(guest)
                    check_udp(host, 50000 + sequence, payload)
                    check_shell(guest)
                check_tcp(host, guest)
            print()
        finally:
            guest.terminate()
            guest.wait(timeout=5)

if sys.argv[1:] == ["http"]:
    for case in ("complete", "truncated", "silent"):
        boot(http_case=case)
    print("Host verified the guest HTTP request, split response, truncation and timeout", flush=True)
else:
    boot(exercise_network=True)
    print("Host verified Ethernet probes, ARP, ICMP, UDP and TCP echo after clsh>", flush=True)
    boot(exercise_network=False)
    print("Silent peer reached clsh>", flush=True)
PY
```

The first boot verifies Ethernet probe replies, resolves the guest MAC by ARP,
and checks three ICMP and three UDP echo replies, including full 1,500-byte IPv4
packets. It verifies TCP checksums, handshake, byte order and orderly closure.
The fixture withholds one TCP acknowledgement and checks that the guest
retransmits the same bytes and sequence number while the link is silent. It
reads `/hello.txt` during that wait and between the other exchanges.
The second boot reads the same file with the peer silent.
Run the regular boot command with `-nic none` to check the shell on its
own. [Build and boot the kernel](../README.md#build-and-run-the-rv64-kernel)

### Fetch an HTTP response over native TCP

From the Linux prompt in `core`, build with `http-probe` and run the
HTTP cases in the private QEMU fixture.

```bash
(
  set -o pipefail
  cargo +stable rustc --locked --manifest-path native/kernel/Cargo.toml \
    --features http-probe --target riscv64gc-unknown-none-elf \
    --target-dir target/http-probe --release \
    -- -C "link-arg=-T$PWD/native/linker/riscv64.ld" -C link-arg=--no-relax &&
  awk '
    /^### Exchange an Ethernet frame over VirtIO-net$/ { section = 1 }
    section && /^python3 - .*PY/ { script = 1; next }
    script && /^PY$/ { exit }
    script { print }
  ' docs/qemu-walkthroughs.md | python3 - http
)
```

`netd` initiates TCP to `192.0.2.1:8080` and sends an HTTP/1.1
`GET /probe`. It checks a `200` response with one `Content-Length`,
up to 32 header fields, up to 1 KiB each for headers and body, and a
five-second deadline for the exchange. The fixture splits the response
across TCP segments, checks the exact body through the guest's success
message, and then boots again for a truncated response and a silent HTTP
peer. Each boot checks
`cat /hello.txt` and stops QEMU. [HTTP/1.1 message framing](https://www.rfc-editor.org/rfc/rfc9112.html#section-6.3)

### Probe a VirtIO block device

Build the kernel, then run these commands from `core` in a Linux terminal.
They create a 1 MiB image with a known prefix and attach it read-only.

```sh
disk="$(mktemp -d)/probe.img"
truncate -s 1M "$disk"
printf 'CLVBLK01' | dd of="$disk" conv=notrunc status=none

qemu-system-riscv64 -machine virt -smp 1 -m 128M -bios default -nographic \
  -kernel target/riscv64gc-unknown-none-elf/release/clavenar-kernel \
  -global virtio-mmio.force-legacy=off \
  -drive "file=$disk,format=raw,if=none,id=probe,readonly=on" \
  -device virtio-blk-device,drive=probe
```

Before paging, the kernel reads sector 0 with a 500 ms probe deadline. It
prints the capacity in 512-byte sectors and the first eight bytes in disk
order. The probe resets the device before continuing to `clsh>`.
[VirtIO MMIO and block specification](https://docs.oasis-open.org/virtio/virtio/v1.2/virtio-v1.2.html)

```text
virtio block sectors=0x0000000000000800 sector0=0x434c56424c4b3031
```

At `clsh>`, run `cat /hello.txt` to confirm the shell responds. Quit QEMU
with `Ctrl+a`, then `x`. Use the regular boot command to try the same
kernel with no disk. The block line appears when a modern device is
attached, and a failed probe reports the error before continuing to the
shell.

### Recover a VirtIO block marker after reboot

Build the kernel, then run this block at the Linux prompt in `core`. It
creates a fresh scratch image and attaches it writable with QEMU's
write-back cache. [QEMU block cache options](https://www.qemu.org/docs/master/system/invocation.html)

```sh
disk="$(mktemp -t clavenar-demo.XXXXXX)"
truncate -s 1M "$disk"
printf 'CLVDEMO1' | dd of="$disk" conv=notrunc status=none

qemu-system-riscv64 -machine virt -smp 1 -m 128M -bios default -nographic \
  -kernel target/riscv64gc-unknown-none-elf/release/clavenar-kernel \
  -global virtio-mmio.force-legacy=off \
  -drive "file=$disk,format=raw,if=none,id=demo,cache=writeback,readonly=off" \
  -device virtio-blk-device,drive=demo
```

The demo recognizes `CLVDEMO1` followed by zeros across all of sector 0.
On a writable device with negotiated flush support, it writes `CLVSAVE1`
and zero padding to a blank sector 1. Look for `virtio block demo flushed`
before paging. The kernel prints it after the flush completes.
[VirtIO block specification](https://docs.oasis-open.org/virtio/virtio/v1.2/virtio-v1.2.html)

Quit QEMU with `Ctrl+a`, then `x`. In the same Linux terminal, boot with
the existing `$disk` attached read-only.

```sh
qemu-system-riscv64 -machine virt -smp 1 -m 128M -bios default -nographic \
  -kernel target/riscv64gc-unknown-none-elf/release/clavenar-kernel \
  -global virtio-mmio.force-legacy=off \
  -drive "file=$disk,format=raw,if=none,id=demo,readonly=on" \
  -device virtio-blk-device,drive=demo
```

Look for `virtio block demo recovered` before `clsh>`. That line confirms
the full marker sector matched. In the first boot, a read-only attachment
with a blank sector 1 prints `virtio block demo read-only`, and an
unrecognized, nonblank sector 1 prints `virtio block demo sector 1 occupied`.

### Recover a previous journal record

After building the kernel, run this block at the Linux prompt in `core`.
It creates a scratch image whose sector 0 contains `CLVJNL01` followed by
zeros. The journal uses sectors 1 and 2.

```sh
journal_disk="$(mktemp -t clavenar-journal.XXXXXX)"
truncate -s 1M "$journal_disk"
printf 'CLVJNL01' | dd of="$journal_disk" conv=notrunc status=none

boot_journal() {
  qemu-system-riscv64 -machine virt -smp 1 -m 128M -bios default -nographic \
    -kernel target/riscv64gc-unknown-none-elf/release/clavenar-kernel \
    -global virtio-mmio.force-legacy=off \
    -drive "file=$journal_disk,format=raw,if=none,id=journal,cache=writeback,readonly=$1" \
    -device virtio-blk-device,drive=journal
}

boot_journal off
```

Before paging, expect `virtio block journal flushed` for generation 1 in
sector 1. Exit QEMU with `Ctrl+a`, then `x`, and run `boot_journal off`
again in the same Linux terminal. The next boot recovers generation 1,
writes generation 2 to sector 2 and reports it flushed.

Exit and run `boot_journal on` from the same terminal. Expect
`virtio block journal recovered` for generation 2 in sector 2 with
`payload_prefix=0x434c56534c4f5432` (`CLVSLOT2`).

Exit QEMU again, change one payload byte in sector 2, then boot read-only.

```sh
printf '\000' | dd of="$journal_disk" bs=1 seek=1040 conv=notrunc status=none
boot_journal on
```

Recovery now reports generation 1 in sector 1 with
`payload_prefix=0x434c56534c4f5431` (`CLVSLOT1`). Each boot selects the
highest valid generation. A writable boot with negotiated flush support
updates the other slot and reports success after the flush. A blank journal
starts at generation 1. Duplicate generations or a nonblank journal with
no valid record produce `virtio block journal invalid slots`.

Each record occupies one 512-byte sector. Integers are little-endian.

| Offset | Field |
| --- | --- |
| 0 | 32-bit format version, currently 1 |
| 4 | 32-bit payload length, up to 492 bytes |
| 8 | 64-bit nonzero generation |
| 16 | Payload followed by zero padding |
| 508 | CRC-32 over bytes 0 through 507 |

[CRC-32 algorithm](https://www.rfc-editor.org/rfc/rfc1952.txt)

### Probe the QEMU host bridge

Run this from an interactive terminal in the Core repository after building
the kernel. The host broker creates a private Unix socket and launches
QEMU with a single modern VirtIO console port. The UART carries `clsh>`.
The boot probe exchanges four-byte `PING` and `PONG` messages after Sv39
activation, with a 500 ms deadline. A successful probe leaves the port
available for the host-backed plan. [VirtIO MMIO and console specification](https://docs.oasis-open.org/virtio/virtio/v1.3/virtio-v1.3.html)

```sh
bridge_journal="$(mktemp -d)/host-bridge.sqlite3"
python3 platforms/qemu-riscv64-virt/host_bridge.py \
  --journal "$bridge_journal" --budget-units 2
```

Look for `host bridge ready` between `Sv39 paging enabled` and `clsh>`.
At the prompt, run the host-backed mock plan example. Press `Ctrl+a`, then
`x`, to exit QEMU. Use `--mode probe-only` to exercise the boot probe by itself.
[Host-backed mock plan](#run-a-host-backed-mock-plan)

Rerun with `--mode silent-probe` to see `host bridge timed out` before the
shell, or `--mode invalid-probe` to see `host bridge invalid reply`.

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

### Resume a suspended agent through echo

At a fresh `clsh>` prompt, let `echo` resume an agent that can propose a rename.

```text
write /draft.txt keep
goal create --preserve-file /draft.txt
goal grant-file --stage-rename /draft.txt
agent run --suspended hello --stage-rename final.txt
agent status
agent resume --via echo
wait hello
goal diff
cat /draft.txt
cat /final.txt
goal commit
cat /final.txt
goal close
```

The first status prints `Suspended child=<handle>`. The resume prints
`resumed by supervisor=<handle>` alongside the goal, agent and original
child handles. The grant covers one suspension. After `hello` exits with
status 0, `goal diff` shows
`rename /draft.txt -> final.txt` and `ready to commit`. The first two
reads print `keep` and `cannot open path`. Once the owner commits, reading
`/final.txt` prints `keep`.

### Observe and resume a suspended echo agent

At a fresh `clsh>` prompt, give the agent just enough reply bytes to send
`echo ready`. The shell uses a second instance of the same executable for
the observation and again for the resume.

```text
goal create --reply-bytes 10
agent run --suspended echo
goal observe echo
agent resume --via echo
wait echo
recv echo
goal budget
goal close
```

The observer reports `Running` for the original child and
`available=0 reserved=10 spent=0`. The resume receipt identifies a
supervisor launch and the same agent and child handles printed at launch.
`wait echo` reports exit 0, `recv echo` prints `echo ready`, and the final
budget shows `available=0 reserved=0 spent=10`. The shell collects each
report by its own handle. `event` labels the two kinds of completion
`echo report exited` and `echo exited`.

### Monitor a goal in the background

At a fresh `clsh>` prompt, arm a monitor while the filesystem agent is
suspended. The prompt returns immediately. The supervisor can resume the
agent while the monitor waits for the goal to finish.

```text
write /draft.txt keep
goal create --preserve-file /draft.txt --reply-bytes 0
goal grant-file --stage-rename /draft.txt
agent run --suspended hello --stage-rename final.txt
goal observe --on-finish echo
goal observe --collect
agent resume --via echo
wait hello
goal observe --collect
goal diff
goal commit
cat /final.txt
goal close
```

The first collection prints `pending`. The second prints `Completed` with the
child handle from `agent run`, exit 0 and `available=0 reserved=0 spent=0`.
The rename stays private until the owner reviews and commits it. The final
read prints `keep`. Collecting again prints `no goal monitor to collect`.

To see cancellation, repeat on a fresh boot through the first collection,
then enter

```text
goal cancel
goal observe --collect
wait hello
goal diff
cat /draft.txt
goal close
```

The monitor reports `Cancelled` for that child, the wait prints
`hello cancelled`, the diff is empty and the original file contains `keep`.
A deadline that expires while the agent is suspended also wakes the monitor
and the report includes `deadline=expired`.

### Restart a faulted goal monitor

At a fresh `clsh>` prompt, arm a finish monitor with one restart available
when you collect its report.

```text
goal create --reply-bytes 10
agent run --suspended echo
goal observe --on-finish echo --restart-once
goal observe --collect
agent resume
wait echo
goal observe --collect
goal budget
goal observe --collect
goal close
```

The first collection prints `pending`. After the agent exits, the monitor
reports `Completed` with `available=0 reserved=0 spent=10`. Collecting
again prints `no goal monitor to collect`.

In a temporary build that faults the first finish monitor before call 68,
collection prints the fault and both launch handles, then collects the
replacement's report in the same command. The goal still spends 10 reply
bytes. If both attempts fault before reporting, collection prints the
second fault and the monitor is finished. A fault after call 68 leaves the
queued report available for one collection. A replacement requires the
original goal to remain open and its deadline to be valid.

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
fresh `echo` and collects its reply and exit status. A running ordinary
`echo` can continue while the observer runs. Immediate observations,
delegated resumes and rename reviews share task 5. A background monitor
can wait in task 6 while any of them runs.

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

### Send data through stdin and stdout

`ask --stdio echo` gives `echo` separate stdin and stdout pipes. The shell
writes input and drains the echoed chunks before sending more. Once all
input has been sent, it closes stdin and reads stdout through EOF.

```text
ask --stdio echo 0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789abcdefghijklmnopqrstuvwxyz
wait echo
```

The input is longer than a pipe's 32-byte capacity. The shell prints all of
it in order, and `wait echo` reports exit status 0. Run `ask --stdio echo`
with no text to check that closing empty stdin also lets the child exit.

### Search a granted file with grep

`grep -F <pattern> <path>` runs the native search program on one read-only
file. It selects whole lines containing a case-sensitive literal. Single or
double quotes can enclose a pattern with spaces. An empty quoted pattern
selects every line. The shell passes the opened file and the pattern together.
A delegated search receives the same arguments using the agent's file grant.
The interactive shell accepts up to 256 bytes for the entire command line.
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

### Watch for a read-scope denial

At a fresh `clsh>` prompt, arm the `echo` monitor before starting the goal's
children. The two helpers and the agent will each try to read the granted
file outside the goal's directory. Set the alert threshold to three denials.

```text
mkdir /work
mkdir /private
write /private/input.txt private
goal create --children 3 --allow-helper grep --read-under /work
goal grant-file --delegate-to grep /private/input.txt
goal observe --on-read-denial echo --after-denials 3
goal observe --collect
agent run --suspended hello
agent delegate --with-file --reply-bytes 18 grep -F private
wait grep
recv grep
goal observe --collect
agent delegate --with-file --reply-bytes 18 grep -F private
wait grep
recv grep
goal observe --collect
agent resume
wait hello
recv hello
goal audit --reads
goal observe --collect
goal observe --collect
goal close
```

Collection prints `pending` immediately before the first denial and after
each of the two helper denials. All three reads fail and the audit shows
`read_bytes=0`. Collection after the third denial prints one
`read-denial alert` with the goal handle, the first `grep` launch handle,
`requested=128`, and `denied=outside-read-scope suppressed=2`. Match the
launch and request size to the first audit entry. The next collection
prints `no goal monitor to collect`.

The threshold accepts 1 through 16 and defaults to 1. To disarm the watch,
repeat from a fresh boot and enter `goal observe --disarm` after the
second helper denial, before resuming `hello`. The third denial appears in
the audit and collection prints `no goal monitor to collect`. Disarm frees
the slot for another watch. Goal cancellation, expiry and closure also
revoke a watch that has yet to send an alert. An alert already queued
remains collectable after closure. Each watch counts further scope denials
up to collection.

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
agent run hello --mock find-greeting@7
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

Revision 7 prints `cannot start agent`. The goal remains `Ready` with
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

### Resume a local search checkpoint

After building the kernel, run this block at the Linux prompt in `core`.
It creates a dedicated image for one checkpoint.

```sh
checkpoint_disk="$(mktemp -t clavenar-checkpoint.XXXXXX)"
truncate -s 1M "$checkpoint_disk"
printf 'CLVCHK01' | dd of="$checkpoint_disk" conv=notrunc status=none

boot_checkpoint() {
  qemu-system-riscv64 -machine virt -smp 1 -m 128M -bios default -nographic \
    -kernel target/riscv64gc-unknown-none-elf/release/clavenar-kernel \
    -global virtio-mmio.force-legacy=off \
    -drive "file=$checkpoint_disk,format=raw,if=none,id=checkpoint,cache=writeback,readonly=${1:-off}" \
    -device virtio-blk-device,drive=checkpoint
}

boot_checkpoint
```

Look for `agent checkpoint journal ready`. At `clsh>`, start the local search.

```text
goal create --children 2 --allow-tool text.search@1 --read-bytes 18 --reply-bytes 36
goal grant-file --delegate-to grep --argument Hello /hello.txt
agent run --checkpoint hello --mock find-greeting@2
```

Wait for `agent checkpoint saved (paused)`, then run the checks below.

```text
agent status
goal audit
goal audit --reads
goal budget
```

Expect a suspended agent, one `grep` delegation, `read_bytes=18` and
`available=0 reserved=18 spent=18`. Quit QEMU with `Ctrl+a`, then `x`, and run
`boot_checkpoint` again in the same Linux terminal. The boot line says
`agent checkpoint recovered (paused)`. At `clsh>`, resume with the saved image.

```text
agent resume
wait hello
recv --checkpoint hello
recv hello
goal audit
goal audit --reads
goal budget
goal close
```

Both reads print `Hello from RamFS`. The checkpoint read also reports
`read_bytes=18 reply_bytes=36`. The recovered goal has zero new
delegations and zero new read audit entries. It still reports
`read_bytes=18`, and its reply budget shows `available=0 reserved=0 spent=36`.
The original search was already charged for its reply, and this resume
charged for forwarding the result.

Quit and run `boot_checkpoint on` a third time to attach the same image
read-only. Look for `agent checkpoint completed`. At `clsh>`, run these
commands.

```text
recv --checkpoint hello
recv --checkpoint hello
agent resume
```

Both reads print the saved result and the same charges. `agent resume`
reports `no current agent`. Prepare a fresh checkpoint image to run the
search again.

Sector 0 contains `CLVCHK01` followed by zeros. Slots 1 and 2 use the
512-byte journal format. The generation 1 payload starts with byte 1 and
the 34-byte definition pin. Five little-endian 64-bit words follow in
order for the read limit, read bytes spent, reply limit, reply bytes spent
and remaining reply reservation. The last 18 bytes hold the saved
`Matches` reply. The generation 2 completion payload uses the same
layout with byte 3, the final reply charge and a zero reply reservation.
A single byte 4 records cancellation. The legacy single-byte 2 marker
still reports `agent checkpoint consumed`. An invalid nonblank slot
blocks recovery because it may have held a terminal record.
[Journal record format](#recover-a-previous-journal-record)

The kernel flushes the completed record before queueing the live reply.
After a crash in that interval, boot with the same image and use
`recv --checkpoint hello` to read the completed reply directly. In the
boot that saved a paused checkpoint, `agent resume` can finish directly
and `goal cancel` can record cancellation and discard the reply.

### Invoke a registered file tool over stdio

`find-greeting@3` invokes the same `text.search@1` tool with two pipes.
The agent sends the approved literal and closes stdin. `grep` checks the
complete input against the launch literal before reading the file. The
agent drains stdout, collects the helper's exit status and forwards the
result. [Revision 3 definition](../userspace/hello/definitions/find-greeting-v3.toml)

```text
goal create --children 2 --allow-tool text.search@1 --read-bytes 18 --reply-bytes 36
goal grant-file --delegate-to grep --argument Hello /hello.txt
agent run hello --mock find-greeting@3
wait hello
recv hello
goal audit
goal audit --reads
goal audit --replies
goal budget
goal close
```

The shell prints `Hello from RamFS`. The read audit reports `read_bytes=18`.
The reply audit attributes 18 bytes to the helper's stdout and 18 to the
agent's forwarded reply. The final balance is `available=0 reserved=0
spent=36`.

To exercise a denied stdout write, first run
`write /long.txt Hello from a longer line`. Repeat with `--read-bytes 64`
and grant `/long.txt`. The result exceeds the pinned 18-byte helper
reservation. The helper's reply audit reports
`denied=insufficient-reply-bytes`, the shell prints
`cannot read requested file` and the final balance reports `spent=0`.

### Invoke a registered file tool over MCP

`find-greeting@4` starts `grep` as a local MCP stdio server for the same
registered tool, `text.search@1`. The guest `hello` client sends
`initialize`, `notifications/initialized`, `tools/list` and `tools/call`
using protocol revision `2025-11-25`. The server exposes one tool named
`text.search`. It accepts a `pattern` string, checks the decoded bytes
against the approved literal and searches the granted descriptor once.
Each UTF-8 JSON-RPC frame is capped at 576 bytes including the newline.
[Revision 4 definition](../userspace/hello/definitions/find-greeting-v4.toml) · [MCP stdio transport](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports)

```text
goal create --children 2 --allow-tool text.search@1 --read-bytes 18 --reply-bytes 1042
goal grant-file --delegate-to grep --argument Hello /hello.txt
agent run hello --mock find-greeting@4
wait hello
recv hello
agent status
goal audit
goal audit --reads
goal audit --replies
goal budget
goal close
```

The shell prints `Hello from RamFS`. The delegation audit records
`tool=text.search@1`, and the read audit reports `read_bytes=18`.
The three encoded stdout responses consume 444 bytes of the helper's
1,024-byte reservation. The agent spends 18 bytes forwarding the
decoded result, leaving `available=580 reserved=0 spent=462`.

Repeat with `--read-bytes 17` to see a tool error. The audit reports
`denied=insufficient-read-bytes` and `read_bytes=0`. The shell prints
`cannot read requested file`. The encoded error response still counts
toward the reply budget.

### Run a host-backed mock plan

Start the host broker with the two-unit allowance above. In revision 5,
`hello` requests an action from the host before invoking the registered
`text.search@1` tool over MCP. The request carries a launch ID and plan
selector. The approved action selects the literal `Hello` from the pinned
definition, and the goal's grant supplies the file.
[Start the broker](#probe-the-qemu-host-bridge) · [Revision 5 definition](../userspace/hello/definitions/find-greeting-v5.toml)

```text
goal create --children 2 --allow-tool text.search@1 --read-bytes 18 --reply-bytes 1042
goal grant-file --delegate-to grep --argument Hello /hello.txt
agent run hello --mock find-greeting@5
wait hello
recv hello
agent status
goal audit
goal audit --reads
goal budget
goal close
```

`recv hello` prints `Hello from RamFS`. The delegation audit shows one
accepted invocation of `text.search@1`, and the read audit shows
`read_bytes=18`. The local MCP session and forwarded result consume
462 guest reply bytes. The host broker reserves two mock units before
dispatch and settles one before replying.

Repeat the commands in the **same QEMU boot**. With one host unit
available, the broker declines the next request before dispatch.
`recv hello` prints `host plan budget exhausted`, the delegation history
is empty, and `goal audit --reads` reports `read_bytes=0`.

Quit QEMU with `Ctrl+a`, then `x`. In the same host terminal, reopen the
SQLite journal to inspect the recorded maximum and actual usage.

```sh
python3 - "$bridge_journal" <<'PY'
from contextlib import closing
from pathlib import Path
import sqlite3
import sys

path = Path(sys.argv[1]).resolve().as_uri()
with closing(sqlite3.connect(path + "?mode=ro", uri=True)) as journal:
    print("session request state max_units used_units reply")
    for row in journal.execute(
        "SELECT session_id, request_id, state, max_units, used_units, reply "
        "FROM requests ORDER BY session_id, request_id"
    ):
        print(*row)
PY
```

The two rows share a host session ID and end in `settled 2 1 1` and
`denied 2 0 0`. In the denied row, `max_units` records the required
allowance. Replaying a request with the same session and launch ID
returns the stored reply. Each QEMU launch has its own host session
and allowance, even when it reuses a journal.

Restart the broker with `--mode invalid-plan` or `--mode silent-plan`
and run one goal in that boot. An unapproved action prints
`invalid host plan reply`. A silent planner prints `host plan timed out`
after two seconds. In both cases, the delegation history is empty,
`read_bytes=0`, and the failure reply consumes one reply byte.

With `silent-plan`, run `goal cancel` immediately after `agent run` to
cancel while the request is pending. Then `wait hello` prints
`hello cancelled` and the delegation and read histories are empty. The
journal keeps the operation `in_flight` with its two host units reserved.
A replay of an `in_flight` request leaves that reservation in place.

### Run a model-backed plan

Start local-only Ollama on the host and download the model. Then run the
broker from `core` in a host terminal. The broker contacts Ollama on
loopback and sends the fixed greeting task and a two-action JSON schema.
The pinned guest definition supplies the tool and pattern. The goal grant
supplies the file.
[Local model setup](../README.md#use-a-local-ollama-model) · [Ollama structured outputs](https://docs.ollama.com/capabilities/structured-outputs) · [Revision 6 definition](../userspace/hello/definitions/find-greeting-v6.toml)

```sh
bridge_journal="$(mktemp -d)/host-bridge.sqlite3"
python3 platforms/qemu-riscv64-virt/host_bridge.py \
  --journal "$bridge_journal" --mode ollama \
  --ollama-model qwen2.5:0.5b --budget-tokens 2080
```

Enter these commands at `clsh>`.

```text
goal create --children 2 --allow-tool text.search@1 --read-bytes 18 --reply-bytes 1042
goal grant-file --delegate-to grep --argument Hello /hello.txt
agent run hello --plan find-greeting@6
wait hello
recv hello
agent status
goal audit
goal audit --reads
goal budget
goal close
```

When the model chooses `search`, `recv hello` prints `Hello from RamFS`
and the audit shows `read_bytes=18`. A `skip` decision prints
`host plan skipped the search` with zero file reads. The guest waits at most
30 seconds or until the goal deadline, whichever comes first.

The broker reserves 2,080 tokens before dispatch, using a 2,048-token context
and a 32-token generation limit. It settles the prompt and generation counts
reported by Ollama. Run a second goal in the same VM to see
`host plan budget exhausted` after the first has used some of its allowance.
An invalid decision or exhausted host budget leaves `read_bytes=0`. When
usage is unknown, the journal keeps the reservation `in_flight`.
[Model limits](https://docs.ollama.com/modelfile#valid-parameters-and-values) · [Chat response fields](https://docs.ollama.com/api/chat)

Quit QEMU with `Ctrl+a`, then `x`. From the same host terminal, inspect the
reserved and settled token counts in the journal.

```sh
python3 - "$bridge_journal" <<'PY'
from contextlib import closing
from pathlib import Path
import sqlite3
import sys

path = Path(sys.argv[1]).resolve().as_uri()
with closing(sqlite3.connect(path + "?mode=ro", uri=True)) as journal:
    print("model request state max_tokens used_tokens prompt_tokens generation_tokens")
    for row in journal.execute("""
        SELECT model, request_id, state, max_units, used_units,
               prompt_tokens, generation_tokens
        FROM requests
        JOIN model_sessions USING (session_id)
        LEFT JOIN token_usage USING (session_id, request_id)
        ORDER BY session_id, request_id
    """):
        print(*row)
PY
```

### Search a host MCP fixture

In a host terminal, start a separate loopback server using the MCP Python
SDK. The fixture server searches its own two lines. The broker uses
stateless JSON responses over Streamable HTTP and sends the fixed literal
`Hello` to `text.search`.
[MCP Python SDK](https://pypi.org/project/mcp/1.26.0/) · [Streamable HTTP](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports#streamable-http)

```sh
mcp_fixture_deps="$(mktemp -d)"
python3 -m pip install --target "$mcp_fixture_deps" 'mcp==1.26.0'
PYTHONPATH="$mcp_fixture_deps" python3 - <<'PY'
from mcp.server.fastmcp import FastMCP

server = FastMCP("clavenar-fixture", host="127.0.0.1", port=8765,
                 stateless_http=True, json_response=True)
fixture = ("Hello from MCP", "Goodbye from MCP")

@server.tool(name="text.search", structured_output=False)
def search(pattern: str) -> str:
    if pattern != "Hello":
        raise ValueError("fixture only accepts the Hello literal")
    print("tools/call text.search", flush=True)
    return next(line for line in fixture if pattern in line)

server.run(transport="streamable-http")
PY
```

After building the kernel, run the broker from `core` in a second host
terminal. This VM receives a one-call allowance.

```sh
bridge_journal="$(mktemp -d)/host-bridge.sqlite3"
python3 platforms/qemu-riscv64-virt/host_bridge.py \
  --mode mcp --mcp-port 8765 --budget-calls 1 --journal "$bridge_journal"
```

Enter these commands at `clsh>`.

```text
goal create --children 1 --allow-tool host.text.search@1 --read-bytes 0 --reply-bytes 20
agent run hello --plan find-greeting@7
wait hello
recv hello
goal audit --reads
goal budget
goal close
```

`recv hello` prints `Hello from MCP`. The server prints one
`tools/call text.search` line. The guest audit shows `read_bytes=0`, and
the guest spends 14 reply bytes. Repeat the commands in the same VM to
see `host tool budget exhausted` with zero additional calls. Omit
`--allow-tool host.text.search@1` to see `host tool grant required`.
[Revision 7 definition](../userspace/hello/definitions/find-greeting-v7.toml)

The broker commits a reservation before contacting the MCP server. A
failure before `tools/call` settles zero calls. A correlated tool response
settles one call. A timeout or lost result after dispatch leaves the call
`in_flight` and keeps its reservation. The same VM session and launch ID
reuse the recorded outcome.

Quit QEMU with `Ctrl+a`, then `x`. In the broker's host terminal, inspect
the journal after the successful and exhausted runs.

```sh
python3 - "$bridge_journal" <<'PY'
from contextlib import closing
from pathlib import Path
import sqlite3
import sys

path = Path(sys.argv[1]).resolve().as_uri()
with closing(sqlite3.connect(path + "?mode=ro", uri=True)) as journal:
    print("session goal launch state max_calls used_calls result")
    for row in journal.execute("""
        SELECT session_id, goal_id, request_id, state, max_units, used_units, result
        FROM mcp_calls JOIN requests USING (session_id, request_id)
        ORDER BY session_id, request_id
    """):
        print(*row)
PY
```

Expect one `settled` row with `1 1 b'Hello from MCP'` and one `denied` row
with `1 0 None`. Match the goal and launch columns to the handles shown
in the guest. Stop the fixture server with `Ctrl+c` in its host terminal.

### Recover a host MCP search checkpoint

Start the [MCP fixture](#search-a-host-mcp-fixture) in a host terminal.
After building the kernel, run the following at the Linux prompt in
`core`. The broker creates the image at the new path and binds one
`find-greeting@7` search to it.

```sh
checkpoint_dir="$(mktemp -d)"
python3 platforms/qemu-riscv64-virt/host_bridge.py \
  --mode mcp --mcp-port 8765 --budget-calls 1 \
  --checkpoint-disk "$checkpoint_dir/checkpoint.img" \
  --journal "$checkpoint_dir/bridge.sqlite3"
```

At `clsh>`, issue the search.

```text
goal create --children 1 --allow-tool host.text.search@1 --read-bytes 0 --reply-bytes 20
agent run --checkpoint hello --plan find-greeting@7
agent status
goal budget
recv --checkpoint hello
```

The agent is suspended at a pending checkpoint with 20 reply bytes reserved
and zero spent. The fixture logs one `tools/call text.search`. Wait for
`host checkpoint <id> receipt recorded` in the terminal running QEMU, then
quit QEMU with `Ctrl+a`, then `x`. You can stop the fixture now. From the same
Linux terminal, reboot using the same image and journal.

```sh
python3 platforms/qemu-riscv64-virt/host_bridge.py \
  --mode recover --checkpoint-disk "$checkpoint_dir/checkpoint.img" \
  --journal "$checkpoint_dir/bridge.sqlite3"
```

Look for `agent host checkpoint recovered (pending)`. At `clsh>` enter
the following.

```text
agent resume
wait hello
recv hello
recv --checkpoint hello
recv --checkpoint hello
goal budget
```

`wait hello` reports exit 0. Each receive prints `Hello from MCP`, and
both checkpoint reads report `read_bytes=0 reply_bytes=14 host_calls=1`.
The goal's reply balance is `available=6 reserved=0 spent=14`. The recovery
broker looks up the original receipt through a read-only journal connection.
Reboot again with the recovery command to read the completed checkpoint
directly with `recv --checkpoint hello`.

To exercise an unknown outcome, restart the fixture with `import time` and
add `time.sleep(3)` immediately after the tool prints `tools/call text.search`.
Repeat with a fresh checkpoint directory. The broker times out waiting for
the tool and prints `host checkpoint <id> outcome unknown`. After reboot
in recovery mode, `agent resume` prints `host checkpoint outcome unknown`.
Set `bridge_journal="$checkpoint_dir/bridge.sqlite3"` and run the journal
query above. It shows a single `in_flight` row with one reserved call.
Repeated resumes preserve the pending checkpoint and the fixture log contains
one call across both boots.

### Run Ollama and both MCP searches in one goal

Start local-only Ollama with `qwen2.5:0.5b` and the MCP fixture in host
terminals. The example uses Ollama on port `11435` and the MCP fixture on
port `8765`. Pass `--ollama-port 11434` if you use Ollama's default port.
[Ollama setup](../README.md#use-a-local-ollama-model) · [MCP fixture](#search-a-host-mcp-fixture)

With the kernel built, run the broker from `core` in another host terminal.

```sh
bridge_journal="$(mktemp -d)/host-bridge.sqlite3"
python3 platforms/qemu-riscv64-virt/host_bridge.py \
  --mode combined --ollama-model qwen2.5:0.5b --ollama-port 11435 \
  --mcp-port 8765 --budget-tokens 4160 --budget-calls 1 --journal "$bridge_journal"
```

Enter these commands at `clsh>`.

```text
goal create --children 2 --allow-tool text.search@1 --allow-tool host.text.search@1 --read-bytes 18 --reply-bytes 1088
goal grant-file --delegate-to grep --argument Hello /hello.txt
agent run hello --plan find-greeting@8
wait hello
recv hello
goal audit
goal audit --reads
goal budget
goal close
```

When Ollama chooses `search`, `recv hello` prints `Hello from RamFS` and
`Hello from MCP` on separate lines. The guest audit attributes the local
`text.search@1` invocation and 18 bytes read to the helper. The guest
spends 476 reply bytes, including the local MCP traffic. The host fixture
prints one `tools/call text.search` line. A `skip` decision ends the plan
before either search runs.
[Revision 8 definition](../userspace/hello/definitions/find-greeting-v8.toml)

Repeat the guest commands in the same VM. There is enough model allowance for
another inference, and the next `search` decision can still run the local
tool. `recv hello` reports `host tool budget exhausted`. The fixture call
count stays at one. Remove either `--allow-tool` option to see
`plan tool grants required` before the model is contacted.

Quit QEMU with `Ctrl+a`, then `x`. In the broker terminal, inspect the journal.

```sh
python3 - "$bridge_journal" <<'PY'
from contextlib import closing
from pathlib import Path
import sqlite3
import sys

path = Path(sys.argv[1]).resolve().as_uri()
with closing(sqlite3.connect(path + "?mode=ro", uri=True)) as journal:
    print("session goal launch request resource state reserved used result")
    for row in journal.execute("""
        SELECT r.session_id, o.goal_id, o.launch_id, r.request_id,
               CASE WHEN c.request_id IS NULL THEN 'tokens' ELSE 'calls' END,
               r.state, r.max_units, r.used_units, c.result
        FROM requests AS r
        JOIN request_owners AS o USING (session_id, request_id)
        LEFT JOIN mcp_calls AS c USING (session_id, request_id)
        ORDER BY r.session_id, r.request_id
    """):
        print(*row)
PY
```

The successful goal has two rows with the same session, goal and launch
and distinct request IDs. The model reserves 2,080 tokens and settles its
reported usage. The tool reserves and settles one call. The next goal's tool
row is `denied` with zero calls used. A tool request whose outcome is
unknown remains `in_flight` and keeps its call reservation.

### Replay a combined goal offline

Run the [combined walkthrough](#run-ollama-and-both-mcp-searches-in-one-goal)
and quit QEMU. From the journal query, take the session and goal IDs for
the recorded outcome you want to replay. A successful search has
`tokens settled` and `calls settled` rows. The host tool budget refusal
from the second goal has `tokens settled` and `calls denied` rows. If the
model skipped the searches, that goal has one `tokens settled` row.

In the same host Bash terminal, replace the two placeholders below with
the IDs. `bridge_journal` still points to the recorded database.

```sh
replay_session=RECORDED_SESSION_ID
replay_goal=RECORDED_GOAL_ID
receipt_journal="$(mktemp -d)/replay-receipts.sqlite3"
python3 platforms/qemu-riscv64-virt/host_bridge.py \
  --mode replay --replay-journal "$bridge_journal" \
  --replay-session "$replay_session" --replay-goal "$replay_goal" \
  --journal "$receipt_journal"
```

The broker prints a fresh `replay session` ID and the recorded session,
goal and launch before booting the VM. It opens the source journal
read-only and serves its saved host replies. You can stop both providers
before replay.
The default receipt journal is `target/host-bridge-replay.sqlite3`.

At the new `clsh>` prompt, give the new goal both tool grants and its
local file grant.

```text
goal create --children 2 --allow-tool text.search@1 --allow-tool host.text.search@1 --read-bytes 18 --reply-bytes 1088
goal grant-file --delegate-to grep --argument Hello /hello.txt
agent run hello --plan find-greeting@8
wait hello
recv hello
goal audit --reads
goal close
```

The output depends on the recorded outcome.

| Recorded outcome | `recv hello` | Local read audit | Receipt stage and reply |
| --- | --- | --- | --- |
| Model search and successful host MCP call | `Hello from RamFS` and `Hello from MCP` | `read_bytes=18` | `model 1`, `tool 1` |
| Model skip | `host plan skipped the search` | `read_bytes=0` | `model 3` |
| Model search and host MCP budget refusal | `host tool budget exhausted` | `read_bytes=18` | `model 1`, `tool 0` |

After a `search` decision, the local MCP tool searches the fresh guest's
file. Replay binds the saved replies in order to one new goal and launch.
Repeating the guest commands in the same VM reports `host model unavailable`.
Restart replay for another run.

Quit QEMU with `Ctrl+a`, then `x`. In the host terminal, set
`receipt_session` to the new `replay session` ID from the broker output.

```sh
receipt_session=REPLAY_SESSION_ID
python3 - "$receipt_journal" "$receipt_session" <<'PY'
from contextlib import closing
from pathlib import Path
import sqlite3
import sys

path = Path(sys.argv[1]).resolve().as_uri()
with closing(sqlite3.connect(path + "?mode=ro", uri=True)) as journal:
    print("source_journal source_session source_goal source_launch source_request "
          "goal launch request stage reply result")
    for row in journal.execute("""
        SELECT s.source_journal, s.source_session_id, s.source_goal_id, s.source_launch_id,
               r.source_request_id, r.goal_id, r.launch_id, r.request_id, r.stage, r.reply, r.result
        FROM replay_sessions AS s
        LEFT JOIN replay_receipts AS r ON r.session_id = s.id
        WHERE s.id = ?
        ORDER BY r.request_id
    """, (sys.argv[2],)):
        print(*row)
PY
```

Each receipt links the fresh goal, launch and request to its source and
records the reply the broker selected before sending it to the guest. For
a successful host call, `result` contains `b'Hello from MCP'`. For a
partial replay, use a recorded `search` and omit the `goal grant-file`
command in the new VM. The guest stops at the local search and the query
shows only the model receipt.

A recorded `skip` must be the goal's sole request. A recorded `search`
requires a second row for the same launch with a settled MCP result or a
budget refusal with zero usage. An in-flight request, missing required row,
or inconsistent owner prevents the broker from starting QEMU.

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

The goal owner can propose a rename while a goal is ready or running.
Goals have one staging slot by default.
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

### Review a staged rename through echo

At a fresh `clsh>` prompt, let `hello` stage a rename and ask a separate
`echo` process to review it.

```text
write /draft.txt keep
goal create --preserve-file /draft.txt
goal grant-file --stage-rename /draft.txt
agent run hello --stage-rename final.txt
wait hello
goal diff --via echo
goal commit
cat /final.txt
goal close
```

`hello` exits with status 0. The diff shows `rename /draft.txt -> final.txt`,
identifies the reviewer, goal and base revision, and reports `ready to commit`
with a guest deadline. You can still read `/draft.txt` at this point. Only
after `goal commit` does `/final.txt` contain `keep`.

To see a blocked review, repeat on a fresh boot and insert
`write /draft.txt changed` immediately after `goal create`, before the file
grant. The agent still exits with status 0, but the reviewer reports
`postcondition failed`. The shell refuses the commit. Before closing the
goal, `cat /draft.txt` prints `changed` and `cat /final.txt` fails to open.

### Require a supervisor review before commit

At a fresh `clsh>` prompt, set `--require-rename-review echo` when creating
the goal. This makes a passing review by the separate `echo` supervisor a
condition of publication for this goal.

```text
write /draft.txt keep
goal create --preserve-file /draft.txt --require-rename-review echo
goal grant-file --stage-rename /draft.txt
agent run hello --stage-rename final.txt
wait hello
goal diff
goal commit
cat /draft.txt
goal diff --via echo
goal commit
cat /final.txt
goal close
```

The first diff shows `rename /draft.txt -> final.txt` and
`supervisor review required`. The first commit is refused, and reading
`/draft.txt` prints `keep`. The delegated review identifies the supervisor
and reports `ready to commit`. The second commit publishes the rename, and
`/final.txt` contains `keep`. Reading the diff again between approval and
commit preserves the approval and its original deadline.

To check a stale approval, repeat on a fresh boot and insert
`write /draft.txt revised` after `goal diff --via echo`. The second commit
reports `commit conflict`. Before closing the goal, `cat /draft.txt`
prints `revised` and `cat /final.txt` reports `cannot open path`.

### Persist a reviewed rename

From the Linux prompt in `core`, build the kernel and create a dedicated
raw image. Sector 0 contains `CLVRNM01` followed by zeros. The two slots
use the same record format as the journal demo.
[Journal record format](#recover-a-previous-journal-record)

Each payload contains a little-endian 16-bit basename length, the
basename bytes and up to 256 bytes of file data.

```sh
durable_disk="$(mktemp -t clavenar-rename.XXXXXX)"
truncate -s 1M "$durable_disk"
printf 'CLVRNM01' | dd of="$durable_disk" conv=notrunc status=none

boot_durable() {
  qemu-system-riscv64 -machine virt -smp 1 -m 128M -bios default -nographic \
    -kernel target/riscv64gc-unknown-none-elf/release/clavenar-kernel \
    -global virtio-mmio.force-legacy=off \
    -drive "file=${2:-$durable_disk},format=raw,if=none,id=durable,cache=writeback,readonly=$1,rerror=report,werror=report" \
    -device virtio-blk-device,drive=durable
}

boot_durable off
```

Expect `durable rename journal ready` after Sv39 paging. Paste this block
at `clsh>`.

```text
write /draft.txt keep
goal create --durable --preserve-file /draft.txt --reply-bytes 0
goal grant-file --stage-rename /draft.txt
agent run hello --stage-rename final.txt
wait hello
goal diff --via echo
goal commit
cat /final.txt
goal close
```

`--durable` requires approval from `echo`. After that approval,
`goal commit` prints `rename committed` once the new record has been
flushed and the file is visible as `/final.txt`. The next line prints
`durable commit generation=… checksum=…`.

Exit QEMU with `Ctrl+a`, then `x`. In the same Linux terminal, run
`boot_durable on` to attach the saved image read-only. Look for
`durable rename recovered` before `clsh>` and compare its generation
and checksum with the commit output. Run `cat /final.txt` to see `keep`,
and `cat /draft.txt` to see `cannot open path`.

The journal holds one root-level file. Later durable renames can update
that same file, alternating sectors. Ordinary edits and unlinks in RamFS
are volatile, and boot recovers the highest valid generation.

To inject a write failure, quit QEMU and run this block from the same
Linux terminal.

```sh
fault_config="$(mktemp -t clavenar-rename-fault.XXXXXX)"
printf '%s\n' '[inject-error]' 'event = "write_aio"' 'errno = "5"' > "$fault_config"
boot_durable off "blkdebug:$fault_config:$durable_disk"
```

At `clsh>`, try to rename the recovered file again.

```text
goal create --durable --preserve-file /final.txt --reply-bytes 0
goal grant-file --stage-rename /final.txt
agent run hello --stage-rename replacement.txt
wait hello
goal diff --via echo
goal commit
cat /final.txt
cat /replacement.txt
goal audit --publication
```

Expect `durable commit outcome unknown` and the attempted generation
and checksum, followed by `keep` for `/final.txt` and `cannot open path`
for `/replacement.txt`. The audit records `storage-outcome-unknown` and
the diff stays staged. With this injected write failure, the next boot
reports the previous generation and checksum and recovers `/final.txt`.

### Persist two renames in one commit

From the Linux prompt in `core`, build the kernel and prepare a raw
pair journal. Sector 0 holds `CLVRNM02` followed by zeros. The slots cover
sectors 1–2 and 3–4. Each slot holds both files under one generation and
one CRC-32.

```sh
pair_disk="$(mktemp -t clavenar-rename-pair.XXXXXX)"
truncate -s 1M "$pair_disk"
printf 'CLVRNM02' | dd of="$pair_disk" conv=notrunc status=none

boot_pair() {
  qemu-system-riscv64 -machine virt -smp 1 -m 128M -bios default -nographic \
    -kernel target/riscv64gc-unknown-none-elf/release/clavenar-kernel \
    -global virtio-mmio.force-legacy=off \
    -drive "file=${2:-$pair_disk},format=raw,if=none,id=pair,cache=writeback,readonly=$1,rerror=report,werror=report" \
    -device virtio-blk-device,drive=pair
}

boot_pair off
```

At `clsh>`, stage both root-level renames and review them together.

```text
write /draft-a.txt alpha
write /draft-b.txt beta
goal create --durable --rename-limit 2 --preserve-file /draft-a.txt --preserve-file /draft-b.txt
goal grant-file --stage-rename /draft-a.txt
goal grant-file --stage-rename /draft-b.txt
agent run hello --stage-rename final-a.txt --stage-rename final-b.txt
wait hello
goal diff --via echo
goal commit
cat /final-a.txt
cat /final-b.txt
goal close
```

The review lists both renames. Commit writes both sectors and flushes
before it publishes both names. Record the generation and checksum printed
after `rename committed`. Later durable commits must include the same two
file identities.

Quit QEMU with `Ctrl+a`, then `x`, and run `boot_pair on` in the same
Linux terminal. The `durable rename recovered` line before `clsh>` reports
the same generation and checksum. Run `cat /final-a.txt` and
`cat /final-b.txt` again to see `alpha` and `beta`.

To fail the second sector write of the next commit, quit QEMU and run
this block in the same Linux terminal. Sector 4 is the second sector of
the slot that follows the first successful commit.
[QEMU block I/O fault injection](https://www.qemu.org/docs/master/devel/testing/blkdebug.html)

```sh
pair_fault="$(mktemp -t clavenar-pair-fault.XXXXXX)"
printf '%s\n' '[inject-error]' 'event = "write_aio"' 'sector = "4"' 'errno = "5"' > "$pair_fault"
boot_pair off "blkdebug:$pair_fault:$pair_disk"
```

At `clsh>`, stage and review a new name for each file.

```text
goal create --durable --rename-limit 2 --preserve-file /final-a.txt --preserve-file /final-b.txt
goal grant-file --stage-rename /final-a.txt
goal grant-file --stage-rename /final-b.txt
agent run hello --stage-rename next-a.txt --stage-rename next-b.txt
wait hello
goal diff --via echo
goal commit
ls /
goal audit --publication
```

Expect `durable commit outcome unknown` and the attempted generation
and checksum. The listing shows both `final-*.txt` names and the audit
shows two `storage-outcome-unknown` rows. Quit and run `boot_pair on`
again. Both `final-*.txt` files and the previous generation and checksum
come from the intact slot.

To inject a flush failure after both writes, quit from that read-only boot
and run this block in the same Linux terminal.

```sh
pair_flush_fault="$(mktemp -t clavenar-pair-flush-fault.XXXXXX)"
printf '%s\n' '[inject-error]' 'event = "flush_to_disk"' 'errno = "5"' > "$pair_flush_fault"
boot_pair off "blkdebug:$pair_flush_fault:$pair_disk"
```

Repeat the commands that stage and review `next-a.txt` and `next-b.txt`.
Commit prints `durable commit outcome unknown` with the attempted
generation and checksum again. The running guest still lists both
`final-*.txt` files. After quitting, run `boot_pair on` and compare the
boot line with that commit output. In this QEMU example both written
sectors remain readable, so the generation and checksum match the
attempt. Read `/next-a.txt` and `/next-b.txt` to see `alpha` and `beta`.

Integers in the 1,024-byte slot are little-endian.

| Offset | Field |
| --- | --- |
| 0 | 32-bit format version, 2 |
| 4 | 32-bit payload length |
| 8 | 64-bit nonzero generation |
| 16 | Two file entries followed by zero padding |
| 1012 | 64-bit copy of the generation |
| 1020 | CRC-32 over bytes 0 through 1019 |

Each file entry contains a 16-bit basename length, a 16-bit content
length, the basename bytes (1–64 bytes) and the file bytes (0–256 bytes).
The basenames must be distinct. Recovery validates the entire slot before
loading either file.

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
rows, oldest first. These include commits, explicit discards and renames
discarded by cancellation, file grant revocation or goal expiry. Each row gives
the absolute source path and destination basename captured at staging. `base`
is the RamFS revision at staging. `revision` is the public revision when the
kernel made the decision. When a commit consumes an approval, the decision
also includes the goal handle and the reviewer.
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
source="/draft.txt" new_name="final.txt" base=2 revision=4 outcome=conflict goal=0x0000000000000001 reviewer=owner
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

The new goal shows one row with `base=4 revision=5 outcome=published` and
`reviewer=owner`. The file `final.txt` contains `revised`. To see
`outcome=review-expired`, leave QEMU running for at least 61 seconds between
`goal diff` and `goal commit`.

### Trace a rename commit to its reviewer

At a fresh `clsh>` prompt, require supervisor review and compare the review
receipt with the publication audit.

```text
write /draft.txt keep
goal create --preserve-file /draft.txt --require-rename-review echo
goal grant-file --stage-rename /draft.txt
agent run hello --stage-rename final.txt
wait hello
goal diff --via echo
goal commit
goal audit --publication
cat /final.txt
goal close
```

The `outcome=published` row contains the same goal and reviewer handles as
the receipt printed by `goal diff --via echo`. Reading `/final.txt` prints
`keep`.

For a conflict, start again on a fresh boot and insert
`write /draft.txt revised` just before `goal commit`. The audit shows
`outcome=conflict` with that review's handle. Before closing the goal,
`cat /draft.txt` prints `revised` and `cat /final.txt` reports `cannot open path`.
To check token expiry, repeat from a fresh boot and wait at least 61 seconds
between the review and commit. The audit shows `outcome=review-expired`
with the original reviewer.

### Publish two renames in one commit

At a fresh `clsh>` prompt, use `--rename-limit 2` to give the goal two
staging slots and repeat `--preserve-file` to capture both inputs at
creation. Grant each file in the order of its proposed name. `hello` stages
both changes against the same RamFS base.

```text
mkdir /work
write /work/draft-a.txt alpha
write /work/draft-b.txt beta
goal create --rename-limit 2 --rename-under /work --require-rename-review echo --preserve-file /work/draft-a.txt --preserve-file /work/draft-b.txt
goal grant-file --stage-rename /work/draft-a.txt
goal grant-file --stage-rename /work/draft-b.txt
agent run hello --stage-rename final-a.txt --stage-rename final-b.txt
wait hello
goal audit --renames
goal diff --via echo
ls /work
goal commit
cat /work/final-a.txt
cat /work/final-b.txt
goal audit --publication
goal close
```

Both staging rows show the same `caller=agent caller_child=` handle.
The diff shows both renames and a single reviewer receipt. Before commit
`ls /work` lists the two draft names. Commit publishes the two final names
together. The reads print `alpha` and `beta`. Both audit rows show
`outcome=published` with the same base revision, new revision and reviewer.

To check a failed postcondition, run the sequence again on a fresh boot with
one of these changes.

- Launch `agent run hello --stage-rename final-a.txt` to stage only the
  first rename.
- Run `write /work/draft-b.txt revised` after `goal create` and before either
  rename is staged.

In both cases `goal diff --via echo` reports `postcondition failed` and
`goal commit` requires a passing review. Use `ls /work` and `cat` on the two
draft paths to verify their names and contents at review time.

For a conflict, start on a fresh boot and run `write /work/draft-b.txt revised`
just before `goal commit`. The commit reports `commit conflict` and both
audit rows report `outcome=conflict`. Before closing the goal, read the original
paths. `draft-a.txt` contains `alpha` and `draft-b.txt` contains `revised`.
Run `ls /work` to confirm the directory still lists the two draft names.

To revoke both grants, run `goal revoke-file` after the supervisor review
and before `goal commit`. The diff becomes empty, both draft names remain
visible, and the publication audit shows two `discarded-by-revoke` rows.

Both sources must be distinct. Each destination must be free in its parent
directory and distinct from any other staged destination there. Discard,
cancellation and goal expiry remove the entire pending batch. Revoking a
file grant also removes the batch if it contains a rename staged under
that grant.

### Observe a reviewed publication

At a fresh `clsh>` prompt, give `hello` a two-file rename goal confined to
`/home/work`. Arm a read-only `echo` monitor while the worker is suspended.
The shell delegates the resume and the review to separate launches in the
supervisor slot. The monitor has its own launch and address space.

```text
mkdir /home
mkdir /home/work
mkdir /private
write /home/work/draft-a.txt alpha
write /home/work/draft-b.txt beta
write /private/untouched.txt private
goal create --rename-limit 2 --rename-under /home/work --require-rename-review echo --reply-bytes 0 --preserve-file /home/work/draft-a.txt --preserve-file /home/work/draft-b.txt
goal grant-file --stage-rename /home/work/draft-a.txt
goal grant-file --stage-rename /home/work/draft-b.txt
agent run --suspended hello --stage-rename final-a.txt --stage-rename final-b.txt
goal observe --on-publication echo
goal observe --collect
agent resume --via echo
wait hello
goal observe --collect
goal commit
goal diff --via echo
goal observe --collect
ls /home/work
```

All three collections print `pending`. The early commit requires a passing
supervisor review. The review prints its own handle and `ready to commit`,
and the directory still lists both draft names.

Publish the reviewed batch as the shell owner.

```text
goal commit
goal observe --collect
ls /home/work
cat /home/work/final-a.txt
cat /home/work/final-b.txt
cat /private/untouched.txt
goal audit --publication
goal close
```

The monitor reports the goal handle, `outcome=published`, `renames=2` and
the reviewer handle from the passing review. Its reply budget sample shows
`available=0 reserved=0 spent=0`. Both final names appear together, with
`alpha` and `beta` in their original files. The audit attributes both
renames to the same publication and reviewer. The file under `/private`
still contains `private`.

For cancellation, repeat the first block from a fresh boot, then cancel
the reviewed transaction.

```text
goal cancel
goal observe --collect
ls /home/work
cat /home/work/draft-a.txt
cat /home/work/draft-b.txt
cat /private/untouched.txt
goal audit --publication
goal close
```

The worker has already finished, so `goal cancel` reports
`goal already finished` and `staged rename discarded`. The monitor reports
`outcome=discarded-by-cancel` for both renames and identifies the reviewer
whose approval was attached to the batch. The original names and contents
remain visible, and the audit records two discarded renames.

## Helper accounting and revocation

### Share a read allowance across two helper launches

Two different helper programs can run at the same time. Each has one slot
for helper launches, so two launches of `grep` run in sequence.
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
