"""Run the native networking walkthroughs on a private QEMU link."""

import argparse
from concurrent.futures import ThreadPoolExecutor
from datetime import datetime, timedelta, timezone
from fcntl import LOCK_EX, flock
from functools import partial
from http.client import HTTPConnection, parse_headers
from io import BytesIO
import json
import os
from pathlib import Path
import re
from select import select
from shutil import copyfile
from socket import SOCK_DGRAM, SOCK_STREAM, inet_aton, inet_ntoa, socketpair
import ssl
from struct import iter_unpack, pack, unpack
from subprocess import PIPE, STDOUT, Popen, run
import sys
from tempfile import TemporaryDirectory
from time import monotonic, sleep

core_root = Path(__file__).resolve().parents[2]
target_triple = "riscv64gc-unknown-none-elf"

guest_mac = bytes.fromhex("52 54 00 12 34 56")
host_mac = bytes.fromhex("02 00 00 00 00 01")
guest_ip = inet_aton("192.0.2.2")
host_ip = inet_aton("192.0.2.1")
https_ip = inet_aton("192.0.2.3")
udp_echo_port = 49152
tcp_echo_port = 49153
fallback_probe_port = 49154
dns_port = 53
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


def send_ipv4(host, protocol, identifier, payload, *, peer_ip=host_ip):
    header = pack("!BBHHHBBH4s4s",
                  0x45, 0, 20 + len(payload), identifier, 0, 64, protocol, 0, peer_ip, guest_ip)
    host.send(ethernet_frame(guest_mac, host_mac, 0x0800, with_checksum(header, 10) + payload))


def receive_ipv4(host, protocol, payload_length=None, *, peer_ip=host_ip):
    frame = host.recv(2048)
    if frame[:14] != host_mac + guest_mac + pack("!H", 0x0800):
        raise RuntimeError("Expected a guest Ethernet frame carrying IPv4")
    ip = frame[14:]
    total_length = int.from_bytes(ip[2:4], "big")
    if (not 20 <= total_length <= len(ip) or
            (payload_length is not None and total_length != 20 + payload_length) or
            ip[0] != 0x45 or ip[9] != protocol or ip[12:20] != guest_ip + peer_ip or
            checksum(ip[:20]) != 0):
        raise RuntimeError("Invalid guest IPv4 header")
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


def send_udp(host, source, destination, payload):
    length = 8 + len(payload)
    datagram = pack("!HHHH", source, destination, length, 0) + payload
    # UDP encodes a computed zero checksum as all ones.
    value = transport_checksum(17, host_ip, guest_ip, datagram) or 0xffff
    send_ipv4(host, 17, source, datagram[:6] + pack("!H", value) + payload)


def check_udp(host, port, payload):
    send_udp(host, port, udp_echo_port, payload)
    length = 8 + len(payload)
    reply = receive_ipv4(host, 17, length)
    source, destination, reply_length, reply_checksum = unpack("!HHHH", reply[:8])
    if ((source, destination, reply_length) != (udp_echo_port, port, length) or
            not reply_checksum or transport_checksum(17, guest_ip, host_ip, reply) != 0 or
            reply[8:] != payload):
        raise RuntimeError(f"Invalid UDP echo reply for port {port}")


def send_tcp(host, port, sequence, acknowledgement, flags, payload=b"", window=4096,
             *, guest_port=tcp_echo_port, peer_ip=host_ip):
    segment = pack("!HHIIBBHHH", port, guest_port, sequence, acknowledgement,
                   5 << 4, flags, window, 0, 0) + payload
    value = transport_checksum(6, peer_ip, guest_ip, segment)
    send_ipv4(host, 6, sequence & 0xffff, segment[:16] + pack("!H", value) + segment[18:],
              peer_ip=peer_ip)


def read_tcp_segment(host, port, *, peer_ip=host_ip):
    segment = receive_ipv4(host, 6, peer_ip=peer_ip)
    if len(segment) < 20 or transport_checksum(6, guest_ip, peer_ip, segment) != 0:
        raise RuntimeError("Invalid TCP segment or checksum")
    source, destination, sequence, acknowledgement, offset, flags, _, _, _ = unpack(
        "!HHIIBBHHH", segment[:20])
    header_length = (offset >> 4) * 4
    if destination != port or not 20 <= header_length <= len(segment):
        raise RuntimeError("Invalid TCP ports or header length")
    control = flags & (tcp_syn | tcp_ack | tcp_fin | tcp_rst)
    return source, (sequence, acknowledgement, control, segment[header_length:])


def receive_tcp(host, port, *, guest_port=tcp_echo_port, peer_ip=host_ip):
    source, segment = read_tcp_segment(host, port, peer_ip=peer_ip)
    if source != guest_port:
        raise RuntimeError("Unexpected TCP source port")
    return segment


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


def answer_guest_arp(host, peer_ip=host_ip):
    format_fields = pack("!HHBB", 1, 0x0800, 6, 4)
    request = host.recv(2048)
    expected = b"\xff" * 6 + guest_mac + pack("!H", 0x0806)
    expected += format_fields + pack("!H", 1) + guest_mac + guest_ip
    if request[:32] != expected or request[38:42] != peer_ip:
        raise RuntimeError(f"Expected the guest to resolve {inet_ntoa(peer_ip)} by ARP")
    reply = format_fields + pack("!H", 2) + host_mac + peer_ip + guest_mac + guest_ip
    host.send(ethernet_frame(guest_mac, host_mac, 0x0806, reply))


def check_http(host, guest, case, response_delay):
    answer_guest_arp(host)
    probe_port, (initial_sequence, _, flags, data) = read_tcp_segment(host, http_port)
    if (flags, data) != (tcp_syn, b""):
        raise RuntimeError("Expected the guest to initiate TCP to port 8080")
    connection = (initial_sequence, probe_port)
    guest_sequence = (initial_sequence + 1) & 0xffffffff
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

    if case == "silent" or response_delay:
        send_tcp(host, http_port, server_sequence, guest_sequence, tcp_ack, guest_port=probe_port)
    if case == "silent":
        log = wait_for_serial(guest, b"netd HTTP probe timed out", timeout=8)
        return connection, log
    log = b""
    if response_delay:
        log = check_shell(guest)
        sleep(response_delay)

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
        log += wait_for_serial(guest, b"netd HTTP probe truncated response")
        return connection, log
    reply = receive_tcp(host, http_port, guest_port=probe_port)
    if reply == (guest_sequence, server_sequence, tcp_ack, b""):
        reply = receive_tcp(host, http_port, guest_port=probe_port)
    if reply != (guest_sequence, server_sequence, tcp_fin | tcp_ack, b""):
        raise RuntimeError("Expected the guest to close the HTTP connection")
    send_tcp(host, http_port, server_sequence, (guest_sequence + 1) & 0xffffffff,
             tcp_ack, guest_port=probe_port)
    log += wait_for_serial(guest, b"netd HTTP probe received Hello from native HTTP")
    return connection, log


def read_serial(guest):
    chunk = os.read(guest.stdout.fileno(), 4096)
    if not chunk:
        raise RuntimeError("QEMU stopped while reading the serial console")
    sys.stdout.buffer.write(chunk)
    sys.stdout.buffer.flush()
    return chunk


def wait_for_serial(guest, expected, timeout=5):
    serial = bytearray()
    deadline = monotonic() + timeout
    while expected not in serial:
        remaining = deadline - monotonic()
        if remaining <= 0 or not select([guest.stdout], [], [], remaining)[0]:
            raise RuntimeError(f"Timed out waiting for {expected!r}")
        serial.extend(read_serial(guest))
    return serial


def wait_for_shell(guest):
    return wait_for_serial(guest, prompt)


def guest_command(guest, command):
    guest.stdin.write(command.encode() + b"\n")
    guest.stdin.flush()
    return wait_for_shell(guest)


def check_shell(guest):
    log = guest_command(guest, "cat /hello.txt")
    if b"Hello from RamFS" not in log:
        raise RuntimeError("The shell could not read /hello.txt")
    return log


def build_kernel(target, *, feature=None, ca=None):
    command = ["cargo", "rustc", "--locked", "--manifest-path", "native/kernel/Cargo.toml",
               "--target", target_triple, "--target-dir", target, "--release"]
    if feature:
        command.extend(["--features", feature])
    command.extend(["--", "-C", f"link-arg=-T{core_root / 'native/linker/riscv64.ld'}",
                    "-C", "link-arg=--no-relax"])
    env = os.environ | {"CLV_TLS_CA_DER": str(ca)} if ca else None
    run(command, cwd=core_root, env=env, check=True)
    return core_root / target / target_triple / "release/clavenar-kernel"


def boot(image, *, exercise_network=False, http_case=None, response_delay=0, extra_args=(),
         extra_fds=(), verify=None):
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
            *extra_args,
        ], stdin=PIPE, stdout=PIPE, stderr=STDOUT, pass_fds=(fd, *extra_fds))
        nic.close()
        try:
            boot_log = wait_for_shell(guest)
            if b"virtio net ready mac=52:54:00:12:34:56" not in boot_log:
                raise RuntimeError("VirtIO-net did not report the configured MAC")
            host.settimeout(2)
            boot_log += check_shell(guest)
            connection = None
            if http_case is not None:
                connection, http_log = check_http(host, guest, http_case, response_delay)
                boot_log += http_log + check_shell(guest)
            elif verify is not None:
                verify(host, guest, boot_log)
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
            return boot_log, connection
        finally:
            guest.terminate()
            guest.wait(timeout=5)


def boot_with_entropy(image, chunks, quota):
    peer, source = socketpair(type=SOCK_STREAM)
    with peer, source, ThreadPoolExecutor(max_workers=1) as workers:
        peer.settimeout(2)

        def supply():
            for chunk in chunks:
                request = bytearray()
                while len(request) < 2:
                    data = peer.recv(2 - len(request))
                    if not data:
                        raise RuntimeError("QEMU closed the entropy backend")
                    request.extend(data)
                if request != bytes([2, len(chunk)]):
                    raise RuntimeError(f"Unexpected entropy request {request.hex()}")
                peer.sendall(chunk)

        pending = workers.submit(supply)
        fd = source.fileno()
        args = (
            "-chardev", f"socket,id=rng-source,fd={fd}",
            "-object", "rng-egd,id=rng0,chardev=rng-source",
            "-device", f"virtio-rng-device,rng=rng0,max-bytes={quota},period=40",
        )
        result = boot(image, http_case="complete", extra_args=args, extra_fds=(fd,))
        pending.result()
        return result


def check_rng(image):
    ready = b"netd crypto entropy ready"
    unavailable = b"netd crypto entropy unavailable"
    device = ("-device", "virtio-rng-device,rng=rng0")
    system_rng = ("-object", "rng-random,id=rng0,filename=/dev/urandom")
    sequences = []
    for _ in range(2):
        log, (sequence, port) = boot(image, http_case="complete", extra_args=system_rng + device)
        if b"virtio rng ready" not in log or ready not in log or port < 57344:
            raise RuntimeError("The entropy device did not supply both boot seeds")
        sequences.append(sequence)
    if sequences[0] == sequences[1]:
        raise RuntimeError("The two seeded boots had the same TCP SYN sequence")

    log, absent = boot(image, http_case="complete")
    if b"virtio rng" in log or unavailable not in log or absent[1] != fallback_probe_port:
        raise RuntimeError("Expected the fallback when the entropy device is absent")
    log, stalled = boot_with_entropy(image, (), 40)
    if b"virtio rng timed out" not in log or unavailable not in log or stalled != absent:
        raise RuntimeError("Expected the fallback when the entropy device stalls")

    transport = bytes.fromhex("92 31 4f 0a 12 51 d9 b3")
    crypto = bytes(range(32))
    chunks = (transport, *(crypto[index:index + 8] for index in range(0, 32, 8)))
    full_log, full = boot_with_entropy(image, (transport, crypto), 40)
    split_log, split = boot_with_entropy(image, chunks, 8)
    # The first ChaCha20 word for this test key, with counter and nonce zero.
    expected_port = 0xe000 | (0x7d2bfd39 & 0x1fff)
    if ready not in full_log or ready not in split_log or full[1] != expected_port or full != split:
        raise RuntimeError("Short entropy reads did not reproduce the expected seeded connection")

    log, partial = boot_with_entropy(image, chunks[:2], 8)
    if (b"virtio rng ready" not in log or unavailable not in log or
            partial != (full[0], fallback_probe_port)):
        raise RuntimeError("An incomplete cryptographic seed affected the connection fallback")
    print("Host verified TCP seeding and fallback with VirtIO RNG", flush=True)
    print("Host verified complete, split and incomplete cryptographic seed reads", flush=True)


def check_rtc(image):
    nanos_per_second = 1_000_000_000
    for base in ("2026-01-01T00:00:00", "2031-07-15T12:34:56"):
        log, _ = boot(image, http_case="complete", response_delay=0.5,
                      extra_args=("-rtc", f"base={base},clock=vm"))
        stamps = dict(re.findall(rb"netd HTTP probe unix_ns_(start|done)=(0x[0-9a-f]{16})", log))
        if stamps.keys() != {b"start", b"done"}:
            raise RuntimeError("The guest did not report both Unix timestamps")
        start, done = (int(stamps[key], 16) for key in (b"start", b"done"))
        reference = int(datetime.fromisoformat(base).replace(tzinfo=timezone.utc).timestamp())
        if not 0 <= start - reference * nanos_per_second < 2 * nanos_per_second:
            raise RuntimeError(f"Guest Unix time {start} did not match RTC base {base}")
        if not 450_000_000 <= done - start < 5 * nanos_per_second:
            raise RuntimeError(f"Unexpected guest elapsed time {done - start} ns")
        print(f"Host verified RTC base {base} UTC and clock progression", flush=True)


def read_dns_query(host):
    datagram = receive_ipv4(host, 17)
    if len(datagram) < 20:
        raise RuntimeError("Expected a UDP datagram with a DNS header")
    source, destination, length, value = unpack("!HHHH", datagram[:8])
    if (not source or destination != dns_port or length != len(datagram) or
            not value or transport_checksum(17, guest_ip, host_ip, datagram) != 0):
        raise RuntimeError("Invalid DNS query ports, length or checksum")

    transaction, flags, questions, answers, authority, additional = unpack("!6H", datagram[8:20])
    question = datagram[20:]
    if ((flags, questions, answers, authority, additional) != (0x0100, 1, 0, 0, 0) or
            question != b"\x05probe\x08clavenar\x04test\x00" + pack("!HH", 1, 1)):
        raise RuntimeError("Expected a single DNS A question for probe.clavenar.test")
    return source, transaction, question


def send_dns_answer(host, query, address):
    port, transaction, question = query
    if address is None:
        header = pack("!6H", transaction, 0x8183, 1, 0, 0, 0)
        record = b""
    else:
        header = pack("!6H", transaction, 0x8180, 1, 1, 0, 0)
        # The answer name points back to the question at offset 12.
        record = b"\xc0\x0c" + pack("!HHIH", 1, 1, 60, 4) + address
    send_udp(host, dns_port, port, header + question + record)


def start_https_goal(guest, log, *, operation="https-probe", grant=True, timeout_ms=None,
                     allowance=None):
    if allowance is None:
        allowance = {"https-probe": 24, "mcp-init": 16, "mcp-list": 12, "mcp-call": 14}[operation]
    command = f"goal create --reply-bytes {allowance}"
    other_grant = {"mcp-init": "https-probe", "mcp-list": "mcp-init", "mcp-call": "mcp-list"}
    granted_operation = operation if grant else other_grant.get(operation)
    if granted_operation:
        command += f" --allow-{granted_operation}"
    if timeout_ms is not None:
        command += f" --timeout-ms {timeout_ms}"
    created = guest_command(guest, command)
    launched = guest_command(guest, f"agent run hello --{operation}")
    if b"goal created" not in created or b"child=0x" not in launched:
        raise RuntimeError("Could not launch the HTTPS goal")
    log.extend(created + launched)
    return allowance


def finish_https_goal(guest, expected, *, spent=1, allowance=24, exit_status=1):
    status = guest_command(guest, "wait hello")
    reply = guest_command(guest, "recv hello")
    budget = guest_command(guest, "goal budget")
    closed = guest_command(guest, "goal close")
    outcome = (b"hello cancelled" if exit_status is None else
               f"hello exited 0x{exit_status:016x}".encode())
    if (outcome not in status or expected not in reply.splitlines() or
            f"available={allowance - spent} reserved=0 spent={spent}".encode() not in budget or
            b"goal closed" not in closed):
        raise RuntimeError("Unexpected goal result, reply budget or close")


def check_dns_failure(host, guest, log, *, case):
    expected, reply = {
        "nxdomain": (b"netd DNS lookup failed", b"HTTPS probe DNS lookup failed"),
        "unusable": (b"netd DNS address rejected", b"HTTPS probe destination denied"),
        "outside-grant": (b"netd DNS address rejected", b"HTTPS probe destination denied"),
        "silent": (b"netd DNS lookup timed out", b"HTTPS probe timed out"),
    }[case]
    start_https_goal(guest, log)
    answer_guest_arp(host)
    query = read_dns_query(host)
    if case != "silent":
        address = {"nxdomain": None, "unusable": guest_ip,
                   "outside-grant": inet_aton("192.0.2.4")}[case]
        send_dns_answer(host, query, address)

    deadline = monotonic() + 7
    while expected not in log:
        ready = select([host, guest.stdout], [], [], max(0, deadline - monotonic()))[0]
        if not ready or monotonic() >= deadline:
            raise RuntimeError(f"Timed out waiting for {expected!r}")
        if host in ready and (case != "silent" or read_dns_query(host) != query):
            raise RuntimeError("Unexpected traffic during a failed DNS lookup")
        if guest.stdout in ready:
            log.extend(read_serial(guest))
    if select([host], [], [], 0.1)[0]:
        raise RuntimeError("Expected the guest to stop sending traffic after DNS failure")
    finish_https_goal(guest, reply)


def tls_context(credentials):
    context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    context.minimum_version = context.maximum_version = ssl.TLSVersion.TLSv1_3
    context.num_tickets = 0
    context.load_cert_chain(*credentials)

    def check_sni(_server, requested, _context):
        if requested != "probe.clavenar.test":
            return ssl.ALERT_DESCRIPTION_UNRECOGNIZED_NAME

    context.sni_callback = check_sni
    return context


def resolve_tls_peer(host, *, cached=False):
    if not cached:
        answer_guest_arp(host)
    send_dns_answer(host, read_dns_query(host), https_ip)
    if not cached:
        answer_guest_arp(host, https_ip)


def cancel_https_goal(guest, log):
    cancelled = guest_command(guest, "goal cancel")
    if b"goal cancelled" not in cancelled:
        raise RuntimeError("Could not cancel the HTTPS goal")
    log.extend(cancelled)


class TlsPeer:
    def __init__(self, host, context):
        self.host = host
        self.incoming, self.outgoing = ssl.MemoryBIO(), ssl.MemoryBIO()
        self.server = context.wrap_bio(self.incoming, self.outgoing, server_side=True)
        self.port, (sequence, _, flags, data) = read_tcp_segment(host, 8443, peer_ip=https_ip)
        if self.port < 57344 or flags != tcp_syn or data:
            raise RuntimeError("Expected a TLS SYN from a seeded source port")
        self.guest_next = (sequence + 1) & 0xffffffff
        self.server_next = 0x23456789
        self.send = partial(send_tcp, host, 8443, guest_port=self.port, peer_ip=https_ip)
        self.receive = partial(receive_tcp, host, 8443, guest_port=self.port, peer_ip=https_ip)
        self.send(self.server_next, self.guest_next, tcp_syn | tcp_ack)
        self.server_next += 1
        self.pending = b""
        self.request = bytearray()
        self.authenticated = self.close_notify = self.responded = False

    def exchange(self, guest, log, respond, *, terminal=None, cancel_on_fin=False):
        deadline = monotonic() + 7
        while terminal is None or terminal not in log:
            ready = select([self.host, guest.stdout], [], [], max(0, deadline - monotonic()))[0]
            if not ready or monotonic() >= deadline:
                raise RuntimeError(f"Timed out during TLS exchange waiting for {terminal!r}")
            if self.host in ready:
                sequence, acknowledged, flags, data = self.receive()
                if (flags & tcp_syn or (terminal is None and flags & tcp_rst) or
                        acknowledged > self.server_next):
                    raise RuntimeError("Unexpected TCP control or acknowledgement during TLS")
                finished = bool(flags & tcp_fin)
                if data or finished:
                    if sequence != self.guest_next:
                        raise RuntimeError("Unexpected TCP stream sequence during TLS")
                    self.guest_next = (self.guest_next + len(data) + finished) & 0xffffffff
                    if not (finished and cancel_on_fin):
                        self.send(self.server_next, self.guest_next, tcp_ack | (flags & tcp_fin))
                        self.server_next += int(finished)
                if data:
                    self.consume(data, respond)
                if finished:
                    if cancel_on_fin:
                        cancel_https_goal(guest, log)
                    else:
                        ack = (self.guest_next, self.server_next, tcp_ack, b"")
                        if self.receive() != ack:
                            raise RuntimeError("Expected the guest's final TCP ACK")
                    return
                if self.pending and acknowledged == self.server_next:
                    chunk, self.pending = self.pending[:512], self.pending[512:]
                    self.send(self.server_next, self.guest_next, tcp_ack, chunk)
                    self.server_next += len(chunk)
            if guest.stdout in ready:
                log.extend(read_serial(guest))

    def consume(self, data, respond):
        self.incoming.write(data)
        try:
            if not self.authenticated:
                self.server.do_handshake()
                self.authenticated = True
            while True:
                plaintext = self.server.read(4096)
                if not plaintext:
                    self.close_notify = True
                    break
                if self.responded:
                    raise RuntimeError("Unexpected HTTP bytes after the request")
                self.request.extend(plaintext)
                self.responded = respond(self.server, bytes(self.request))
        except ssl.SSLWantReadError:
            pass
        self.pending += self.outgoing.read()

    def require_shutdown(self):
        if self.server.version() != "TLSv1.3" or not self.close_notify or not self.responded:
            raise RuntimeError("Expected a completed TLS exchange and close_notify before TCP FIN")

    def send_late_reply(self, reply):
        self.server.write(reply)
        self.send(self.server_next, self.guest_next, tcp_ack, self.outgoing.read())
        deadline = monotonic() + 1
        while select([self.host], [], [], 0.1)[0]:
            if monotonic() >= deadline or not self.receive()[2] & tcp_rst:
                raise RuntimeError("Unexpected traffic after the cancelled connection's late response")


def check_tls_peer(host, guest, log, *, credentials, case, cached=False):
    expected = {
        "complete": b"netd HTTPS probe received Hello from native HTTPS",
        "malformed": b"netd HTTPS probe invalid response",
        "truncated": b"netd HTTPS probe truncated response",
        "stalled": b"netd HTTPS probe timed out",
        "wrong-body": b"netd HTTPS probe unexpected body",
        "rejected": b"netd TLS probe certificate rejected",
        "cancelled": b"netd HTTPS probe cancelled",
    }[case]
    expected_request = (
        b"GET /probe HTTP/1.1\r\n"
        b"Host: probe.clavenar.test:8443\r\n"
        b"Connection: close\r\n\r\n"
    )
    body = b"Hello from native HTTPS\n"
    chunks = (
        b"HTTP/1.1 200 OK\r\nContent-Len",
        f"gth: {len(body)}\r\nConnection: close\r\n\r\n".encode() + body[:5],
        body[5:],
    )
    if case in ("truncated", "stalled"):
        chunks = chunks[:-1]
    elif case == "malformed":
        chunks = (b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nContent-Length: 0\r\n\r\n",)
    elif case == "wrong-body":
        chunks = chunks[:-1] + (body[5:].upper(),)

    def respond(server, request):
        if not expected_request.startswith(request):
            raise RuntimeError(f"Unexpected HTTPS request {request!r}")
        if request != expected_request:
            return False
        if case == "cancelled":
            cancel_https_goal(guest, log)
        else:
            for chunk in chunks:
                server.write(chunk)
            if case == "truncated":
                server.unwrap()
        return True

    start_https_goal(guest, log)
    resolve_tls_peer(host, cached=cached)
    peer = TlsPeer(host, tls_context(credentials))
    peer.exchange(guest, log, respond, terminal=None if case == "complete" else expected)
    if expected not in log:
        log.extend(wait_for_serial(guest, expected))

    authenticated = case != "rejected"
    if peer.authenticated != authenticated or (authenticated and peer.server.version() != "TLSv1.3"):
        raise RuntimeError("Unexpected TLS handshake result")
    if peer.request != (expected_request if authenticated else b""):
        raise RuntimeError(f"Unexpected decrypted request bytes {peer.request!r}")
    if case == "complete":
        peer.require_shutdown()

    if case == "cancelled":
        finish_https_goal(guest, b"no message", spent=0, exit_status=None)
        # Deliver the old connection's result after its goal closes.
        peer.send_late_reply(b"".join(chunks))
        check_tls_peer(host, guest, bytearray(), credentials=credentials, case="complete", cached=True)
        return

    reply = {
        "complete": b"Hello from native HTTPS",
        "stalled": b"HTTPS probe timed out",
        "rejected": b"HTTPS probe TLS failed",
    }.get(case, b"HTTPS probe invalid response")
    complete = case == "complete"
    finish_https_goal(guest, reply, spent=24 if complete else 1, exit_status=0 if complete else 1)


def check_tls_prerequisite(host, guest, log, *, reason):
    start_https_goal(guest, log)
    expected = f"netd TLS probe {reason} unavailable".encode()
    if expected not in log:
        wait_for_serial(guest, expected)
    if select([host], [], [], 0.1)[0]:
        raise RuntimeError(f"Unexpected guest traffic without {reason}")
    finish_https_goal(guest, b"HTTPS probe unavailable")


def check_https_authority(host, guest, log, *, grant):
    start_https_goal(guest, log, grant=grant, timeout_ms=600 if grant else None)
    if grant:
        answer_guest_arp(host)
        read_dns_query(host)
        start = monotonic()
        finish_https_goal(guest, b"no message", spent=0, exit_status=None)
        if monotonic() - start > 2:
            raise RuntimeError("The probe did not stop at the earlier goal deadline")
    else:
        finish_https_goal(guest, b"HTTPS probe grant required")
    if select([host], [], [], 0.1)[0]:
        raise RuntimeError("An ungranted or expired goal sent network traffic")


def relay_mcp(server, request, *, method, case, hold_reply=None):
    head, separator, body = request.partition(b"\r\n\r\n")
    if len(head) > 1024 or len(body) > 1024:
        raise RuntimeError("The HTTP request exceeded the fixture bounds")
    if not separator:
        return False
    line, _, headers = head.partition(b"\r\n")
    headers = parse_headers(BytesIO(headers + b"\r\n\r\n"))
    version = "2025-11-25"
    protocol_header = None if method == "initialize" else version
    if (line != b"POST /mcp HTTP/1.1" or headers.get("Host") != "probe.clavenar.test:8443" or
            headers.get("Connection") != "close" or headers.get("Content-Type") != "application/json" or
            headers.get("Accept") != "application/json, text/event-stream" or
            headers.get("MCP-Protocol-Version") != protocol_header or "MCP-Session-Id" in headers):
        raise RuntimeError(f"Unexpected MCP request headers {head!r}")
    length, = headers.get_all("Content-Length", [])
    if len(body) < int(length):
        return False
    if len(body) != int(length):
        raise RuntimeError("Unexpected MCP request length")

    message = {"jsonrpc": "2.0", "method": method}
    if method == "initialize":
        message.update(id=1, params={
            "protocolVersion": version, "capabilities": {},
            "clientInfo": {"name": "clavenar-netd", "version": "1"},
        })
    elif method == "tools/list":
        message.update(id=2, params={})
    elif method == "tools/call":
        message.update(id=3, params={"name": "text.search", "arguments": {"pattern": "Hello"}})
    if json.loads(body) != message:
        raise RuntimeError(f"Unexpected MCP message {body!r}")

    forwarded = {name: headers[name] for name in ("Accept", "Content-Type", "MCP-Protocol-Version")
                 if name in headers}
    notification = method == "notifications/initialized"
    connection = HTTPConnection("127.0.0.1", 8765, timeout=2)
    try:
        connection.request("POST", "/mcp", body, forwarded)
        response = connection.getresponse()
        reply = response.read(1025)
        status_line = f"HTTP/1.1 {response.status} {response.reason}\r\n"
        content_type = response.getheader("Content-Type")
        if (response.status != (202 if notification else 200) or len(reply) > 1024 or
                (notification and reply) or (not notification and content_type != "application/json")):
            raise RuntimeError(f"Unexpected MCP SDK response {status_line.strip()} {reply!r}")
    finally:
        connection.close()

    if method == "initialize" and case in ("wrong-id", "wrong-version"):
        result = json.loads(reply)
        if case == "wrong-id":
            result["id"] = 2
        else:
            result["result"]["protocolVersion"] = "2025-06-18"
        reply = json.dumps(result).encode()
    elif notification and case == "nonempty-accepted":
        reply = b"{}"
    elif method == "tools/list" and case in ("list-wrong-id", "wrong-schema", "paginated"):
        result = json.loads(reply)
        if case == "list-wrong-id":
            result["id"] = 1
        elif case == "wrong-schema":
            schema = result["result"]["tools"][0]["inputSchema"]
            schema["properties"]["pattern"]["type"] = "integer"
        else:
            result["result"]["nextCursor"] = "another-page"
        reply = json.dumps(result).encode()
    elif method == "tools/call" and case in (
        "call-wrong-id", "tool-error", "max-result", "oversized", "control-text", "late-response",
    ):
        result = json.loads(reply)
        if case == "call-wrong-id":
            result["id"] = 2
        elif case == "tool-error":
            result["result"]["isError"] = True
        else:
            result["result"]["content"][0]["text"] = {
                "max-result": "x" * 63 + "\n",
                "oversized": "x" * 65,
                "control-text": "Hello\x1b[0m",
                "late-response": "Reply from the cancelled call",
            }[case]
        reply = json.dumps(result).encode()

    if content_type is not None:
        status_line += f"Content-Type: {content_type}\r\n"
    head = f"{status_line}Content-Length: {len(reply)}\r\nConnection: close\r\n\r\n".encode()
    if hold_reply is not None:
        hold_reply(head + reply)
    else:
        server.write(head)
        if reply:
            server.write(reply)
    print(f"Host MCP server accepted {method}", flush=True)
    return True


def check_mcp_peer(host, guest, log, *, credentials, case, operation="mcp-init", cached=False):
    allowance = start_https_goal(
        guest, log, operation=operation, grant=case != "missing-grant",
        allowance={"short-budget": 13, "max-result": 64}.get(case),
    )
    label = operation.replace("mcp-", "MCP ").encode()
    held_reply = None
    exit_status = 1
    if case == "missing-grant":
        reply, spent = label + b" grant required", 1
    else:
        resolve_tls_peer(host, cached=cached)
        context = tls_context(credentials)
        methods = ("initialize", "notifications/initialized", "tools/list", "tools/call")
        count = {"mcp-init": 2, "mcp-list": 3, "mcp-call": 4}[operation]
        last_method = {
            "cancelled": methods[count - 2],
            "wrong-id": "initialize",
            "wrong-version": "initialize",
            "nonempty-accepted": "notifications/initialized",
            "wrong-schema": "tools/list",
        }.get(case, methods[count - 1])

        def hold_reply(data):
            nonlocal held_reply
            held_reply = data
            cancel_https_goal(guest, log)

        ports = set()
        for method in methods[:count]:
            peer = TlsPeer(host, context)
            if peer.port in ports:
                raise RuntimeError("Expected a fresh TCP source port for each MCP request")
            ports.add(peer.port)
            last = method == last_method
            terminal = {
                "nonempty-accepted": b"netd HTTPS probe invalid response",
                "late-response": b"netd HTTPS probe cancelled",
            }.get(case) if last else None
            respond = partial(relay_mcp, method=method, case=case,
                              hold_reply=hold_reply if last and case == "late-response" else None)
            peer.exchange(guest, log, respond, terminal=terminal,
                          cancel_on_fin=last and case == "cancelled")
            if terminal is not None:
                if not peer.responded:
                    raise RuntimeError(f"The MCP server did not receive {method}")
            else:
                peer.require_shutdown()
            if last:
                break

        if case in ("cancelled", "late-response"):
            reply, spent = b"no message", 0
            exit_status = None
        elif case == "short-budget":
            reply, spent = b"no message", 0
        elif case in ("complete", "max-result"):
            reply = {
                "mcp-init": b"MCP initialized",
                "mcp-list": b"text.search",
                "mcp-call": b"Hello from MCP",
            }[operation] if case == "complete" else b"x" * 63
            spent, exit_status = allowance, 0
        else:
            reply, spent = label + b" invalid response", 1
    finish_https_goal(guest, reply, spent=spent, allowance=allowance, exit_status=exit_status)
    if held_reply is not None:
        peer.send_late_reply(held_reply)
        check_mcp_peer(host, guest, bytearray(), credentials=credentials, case="complete",
                       operation=operation, cached=True)
    elif select([host], [], [], 0.1)[0]:
        raise RuntimeError("Unexpected guest traffic after the MCP result")


def check_tls(*, mcp_operation=None):
    def openssl(*args):
        run(["openssl", *args], check=True)

    target = core_root / "target/tls-probe"
    with TemporaryDirectory(prefix="clavenar-tls-") as directory:
        files = Path(directory)
        keygen = ("-newkey", "ec", "-pkeyopt", "ec_paramgen_curve:P-256", "-noenc")
        for authority in (("fixture",) if mcp_operation else ("fixture", "other")):
            openssl("req", "-x509", *keygen, "-days", "4", "-subj", f"/CN={authority} CA",
                    "-addext", "basicConstraints=critical,CA:TRUE",
                    "-addext", "keyUsage=critical,keyCertSign,cRLSign",
                    "-keyout", files / f"{authority}.key", "-out", files / f"{authority}.pem")
        ca = files / "fixture.der"
        openssl("x509", "-in", files / "fixture.pem", "-outform", "DER", "-out", ca)

        def leaf(label, name, authority, serial):
            key, csr, pem = (files / f"{label}.{suffix}" for suffix in ("key", "csr", "pem"))
            openssl("req", "-new", *keygen, "-subj", "/CN=probe.clavenar.test", "-keyout", key, "-out", csr)
            extensions = files / "leaf.conf"
            extensions.write_text(f"subjectAltName=DNS:{name}\n"
                                  "basicConstraints=critical,CA:FALSE\n"
                                  "keyUsage=critical,digitalSignature\nextendedKeyUsage=serverAuth\n")
            openssl("x509", "-req", "-in", csr, "-CA", files / f"{authority}.pem",
                    "-CAkey", files / f"{authority}.key", "-set_serial", str(serial), "-days", "2",
                    "-sha256", "-extfile", extensions, "-out", pem)
            return pem, key

        valid = leaf("valid", "probe.clavenar.test", "fixture", 1)
        # Each run must keep the image that trusts its own temporary CA.
        target.mkdir(parents=True, exist_ok=True)
        image = files / "clavenar-kernel"
        with (target / ".fixture.lock").open("a") as lock:
            flock(lock, LOCK_EX)
            copyfile(build_kernel(target, feature="tls-probe", ca=ca), image)

        now = datetime.now(timezone.utc) + timedelta(seconds=1)
        system_rng = ("-object", "rng-random,id=rng0,filename=/dev/urandom",
                      "-device", "virtio-rng-device,rng=rng0")

        def rtc(time):
            return "-rtc", f"base={time:%Y-%m-%dT%H:%M:%S},clock=vm"

        if mcp_operation:
            cases = {
                "mcp-init": ("complete", "missing-grant", "wrong-id", "wrong-version",
                             "cancelled", "nonempty-accepted"),
                "mcp-list": ("complete", "missing-grant", "wrong-version", "list-wrong-id",
                             "wrong-schema", "paginated", "cancelled"),
                "mcp-call": ("complete", "missing-grant", "wrong-schema", "call-wrong-id",
                             "tool-error", "max-result", "oversized", "control-text",
                             "short-budget", "cancelled", "late-response"),
            }
            label = {"mcp-init": "initialization", "mcp-list": "discovery",
                     "mcp-call": "invocation"}[mcp_operation]
            for case in cases[mcp_operation]:
                boot(image, extra_args=system_rng + rtc(now),
                     verify=partial(check_mcp_peer, credentials=valid, case=case, operation=mcp_operation))
                print(f"Host verified MCP {label} case {case}", flush=True)
            return

        wrong_name = leaf("wrong-name", "other.clavenar.test", "fixture", 2)
        other_ca = leaf("other-ca", "probe.clavenar.test", "other", 1)
        for label, credentials, time in (
            ("expired certificate", valid, now + timedelta(days=3)),
            ("wrong certificate name", wrong_name, now),
            ("untrusted CA", other_ca, now),
        ):
            boot(image, extra_args=system_rng + rtc(time),
                 verify=partial(check_tls_peer, credentials=credentials, case="rejected"))
            print(f"Host verified TLS rejected the {label} before HTTP", flush=True)

        for case in ("complete", "malformed", "truncated", "stalled", "wrong-body"):
            boot(image, extra_args=system_rng + rtc(now),
                 verify=partial(check_tls_peer, credentials=valid, case=case))
            print(f"Host verified HTTPS response case {case}", flush=True)
        print("Host verified HTTPS connects to the DNS answer at 192.0.2.3", flush=True)

        for case in ("nxdomain", "unusable", "outside-grant", "silent"):
            boot(image, extra_args=system_rng + rtc(now),
                 verify=partial(check_dns_failure, case=case))
            print(f"Host verified DNS failure case {case}", flush=True)

        for grant in (False, True):
            boot(image, extra_args=system_rng + rtc(now),
                 verify=partial(check_https_authority, grant=grant))
        print("Host verified HTTPS grant denial and the earlier goal deadline", flush=True)

        boot(image, extra_args=system_rng + rtc(now),
             verify=partial(check_tls_peer, credentials=valid, case="cancelled"))
        print("Host verified cancellation, stale reply rejection and a fresh HTTPS goal", flush=True)

        for reason, args in (
            ("entropy", rtc(now)),
            ("clock", system_rng + ("-rtc", "base=1969-12-31T23:59:30,clock=vm")),
        ):
            boot(image, extra_args=args, verify=partial(check_tls_prerequisite, reason=reason))
            print(f"Host verified TLS refuses to connect without {reason}", flush=True)


def main():
    parser = argparse.ArgumentParser(
        description=__doc__,
        epilog="For the MCP modes, start mcp_fixture.py on 127.0.0.1:8765 first.",
    )
    parser.add_argument("mode", nargs="?", default="ethernet",
                        choices=("ethernet", "http", "rng", "rtc", "tls", "mcp", "mcp-list", "mcp-call"),
                        help="walkthrough to run (default: ethernet)")
    mode = parser.parse_args().mode
    if mode == "ethernet":
        image = build_kernel("target")
        boot(image, exercise_network=True)
        print("Host verified Ethernet probes, ARP, ICMP, UDP and TCP echo after clsh>", flush=True)
        boot(image)
        print("Silent peer reached clsh>", flush=True)
    elif mode in ("tls", "mcp", "mcp-list", "mcp-call"):
        operation = {"tls": None, "mcp": "mcp-init", "mcp-list": "mcp-list", "mcp-call": "mcp-call"}[mode]
        check_tls(mcp_operation=operation)
    else:
        image = build_kernel("target/http-probe", feature="http-probe")
        if mode == "rtc":
            check_rtc(image)
        elif mode == "rng":
            check_rng(image)
        else:
            for case in ("complete", "truncated", "silent"):
                boot(image, http_case=case)
            print("Host verified the guest HTTP request, split response, truncation and timeout", flush=True)


if __name__ == "__main__":
    main()
