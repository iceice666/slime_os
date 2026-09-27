"""Controlled HTTP and DNS peers using ordinary host TCP/UDP sockets.

HTTP/DNS are externally specified formats. This helper neither handles Ethernet
frames nor resolves hostnames or fetches HTTP on the guest's behalf.
"""

from __future__ import annotations

import socket
import selectors
import os
import sys
import struct
import threading
import time
from dataclasses import dataclass, field

HTTP_PORT = 18080
DNS_PORT = 1053
BODY = bytes((index * 37 + 11) % 256 for index in range(12289))


@dataclass(frozen=True)
class Case:
    name: str
    response: bytes
    body: bytes
    status: int = 200
    error: str = "none"
    dns: str = "a"
    host: str = "example.test"
    path: str = "/qualification?bounded=1"
    expect_request: bool = True
    reset: bool = False
    stall: bool = False
    launch_url: str | None = None
    attached: bool = True
    port: int = HTTP_PORT
    destination: str = "10.0.2.2"

    @property
    def url(self) -> str:
        return self.launch_url if self.launch_url is not None else f"http://{self.host}:{self.port}{self.path}"


def cases() -> tuple[Case, ...]:
    length = b"HTTP/1.1 200 OK\r\nContent-Length: " + str(len(BODY)).encode() + b"\r\nConnection: close\r\n\r\n" + BODY
    chunks = b"".join(f"{len(part):x};bounded=yes\r\n".encode() + part + b"\r\n" for part in (BODY[:31], BODY[31:4097], BODY[4097:]))
    return (
        Case("numeric-content-length", length, BODY, host="10.0.2.2", path="/"),
        Case("dns-content-length", length, BODY),
        Case("default-port-path", length, BODY, path="/", launch_url="http://example.test", port=80, destination="10.0.2.100"),
        Case("chunked", b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n" + chunks + b"0\r\nX-Result: bounded\r\n\r\n", BODY),
        Case("close-delimited", b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n" + BODY, BODY),
        Case("no-body", b"HTTP/1.1 204 No Content\r\n\r\n", b"", status=204),
        Case("informational-non2xx", b"HTTP/1.1 100 Continue\r\n\r\nHTTP/1.1 404 Not Found\r\nContent-Length: 3\r\n\r\nno!", b"no!", status=404),
        Case("conflicting-lengths", b"HTTP/1.1 200 OK\r\nContent-Length: 1\r\nContent-Length: 2\r\n\r\nx", b"", status=0, error="malformed"),
        Case("early-eof", b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nabc", b"abc", error="early-eof"),
        Case("unsupported-coding", b"HTTP/1.1 200 OK\r\nContent-Encoding: gzip\r\nContent-Length: 0\r\n\r\n", b"", status=0, error="unsupported"),
        Case("upgrade", b"HTTP/1.1 101 Switching Protocols\r\nConnection: upgrade\r\nUpgrade: websocket\r\n\r\n", b"", status=0, error="unsupported"),
        Case("malformed-chunk", b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\nz\r\n", b"", error="malformed"),
        Case("ambiguous-framing", b"HTTP/1.1 200 OK\r\nContent-Length: 1\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n", b"", status=0, error="malformed"),
        Case("oversized-header", b"HTTP/1.1 200 OK\r\nX-Large: " + b"x" * 8192 + b"\r\n\r\n", b"", status=0, error="limit"),
        Case("oversized-chunk", b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n100001\r\n", b"", error="limit"),
        Case("reset", b"", b"", status=0, error="transport", reset=True),
        Case("deadline", b"", b"", status=0, error="transport", stall=True),
        Case("dns-cname", length, BODY, dns="cname"),
        Case("dns-nxdomain", b"", b"", status=0, error="transport", dns="nxdomain", expect_request=False),
        Case("dns-truncated", b"", b"", status=0, error="transport", dns="truncated", expect_request=False),
        Case("dns-compression-loop", b"", b"", status=0, error="transport", dns="loop", expect_request=False),
        Case("dns-forbidden", b"", b"", status=0, error="transport", dns="forbidden", expect_request=False),
        Case("dns-mismatched-id", b"", b"", status=0, error="transport", dns="mismatched-id", expect_request=False),
        Case("dns-wrong-source", b"", b"", status=0, error="transport", dns="wrong-source", expect_request=False),
        Case("dns-timeout", b"", b"", status=0, error="transport", dns="timeout", expect_request=False),
        Case("dns-wrong-question", b"", b"", status=0, error="transport", dns="wrong-question", expect_request=False),
        Case("dns-excessive-records", b"", b"", status=0, error="transport", dns="excessive-records", expect_request=False),
        Case("dns-excessive-aliases", b"", b"", status=0, error="transport", dns="excessive-aliases", expect_request=False),
        Case("dns-zero-ttl", b"", b"", status=0, error="transport", dns="zero-ttl", expect_request=False),
        Case("url-scheme", b"", b"", status=0, error="url", expect_request=False, launch_url="https://example.test/", attached=False),
        Case("url-injection", b"", b"", status=0, error="url", expect_request=False, launch_url="http://example.test:18080/\r\nHost: invalid.test", attached=False),
        Case("url-userinfo", b"", b"", status=0, error="url", expect_request=False, launch_url="http://user@example.test:18080/", attached=False),
    )


def dns_question(packet: bytes) -> tuple[str, bytes]:
    if len(packet) < 17 or packet[2:12] != b"\x01\x00\x00\x01\x00\x00\x00\x00\x00\x00":
        raise ValueError("guest DNS query is not one standard recursive question")
    labels: list[str] = []
    offset = 12
    while offset < len(packet) and packet[offset]:
        length = packet[offset]
        if length > 63 or offset + 1 + length >= len(packet):
            raise ValueError("invalid guest DNS question label")
        labels.append(packet[offset + 1:offset + 1 + length].decode("ascii"))
        offset += length + 1
    if packet[offset:] != b"\0\0\x01\0\x01":
        raise ValueError("guest DNS query is not exactly A/IN")
    return ".".join(labels).lower(), packet[12:]


def dns_response(query: bytes, question: bytes, mode: str, destination: str = "10.0.2.2") -> bytes:
    flags = 0x8183 if mode == "nxdomain" else 0x8380 if mode == "truncated" else 0x8180
    count = 0 if mode in ("nxdomain", "truncated") else 2 if mode == "cname" else 1
    identity = int.from_bytes(query[:2], "big") ^ (1 if mode == "mismatched-id" else 0)
    header = struct.pack("!HHHHHH", identity, flags, 1, 100 if mode == "excessive-records" else count, 0, 0)
    if mode == "wrong-question":
        question = question.replace(b"example", b"invalid")
    answer = b""
    if mode == "excessive-aliases":
        owner = b"\xc0\x0c"
        for index in range(5):
            alias = b"\x02n" + bytes([ord("0") + index]) + b"\x04test\0"
            answer += owner + struct.pack("!HHIH", 5, 1, 30, len(alias)) + alias
            owner = alias
        answer += owner + struct.pack("!HHIH", 1, 1, 30, 4) + socket.inet_aton(destination)
        return struct.pack("!HHHHHH", identity, flags, 1, 6, 0, 0) + question + answer
    if count:
        address = socket.inet_aton("127.0.0.1" if mode == "forbidden" else destination)
        owner = b"\xc0\x0c"
        if mode == "cname":
            alias = b"\x04edge\x07example\x04test\0"
            answer = owner + struct.pack("!HHIH", 5, 1, 30, len(alias)) + alias
            owner = alias
        if mode == "loop":
            offset = 12 + len(question)
            owner = (0xC000 | offset).to_bytes(2, "big")
        answer += owner + struct.pack("!HHIH", 1, 1, 0 if mode == "zero-ttl" else 30, 4) + address
    return header + question + answer


@dataclass
class Peer:
    case: Case
    http_port: int = HTTP_PORT
    dns_port: int = DNS_PORT
    requests: list[bytes] = field(default_factory=list)
    queries: list[tuple[str, int, int]] = field(default_factory=list)
    errors: list[Exception] = field(default_factory=list)
    stop: threading.Event = field(default_factory=threading.Event)
    _sockets: list[socket.socket] = field(default_factory=list)
    _threads: list[threading.Thread] = field(default_factory=list)

    def __enter__(self) -> Peer:
        try:
            tcp = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
            self._sockets.append(tcp)
            tcp.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
            tcp.bind(("127.0.0.1", self.http_port))
            tcp.listen(4)
            self.http_port = tcp.getsockname()[1]
            udp = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
            self._sockets.append(udp)
            udp.bind(("127.0.0.1", self.dns_port))
            self.dns_port = udp.getsockname()[1]
            for sock, handler in ((tcp, self._http), (udp, self._dns)):
                sock.settimeout(0.1)
                thread = threading.Thread(target=self._observe, args=(handler, sock), daemon=True)
                self._threads.append(thread)
                thread.start()
        except BaseException:
            self.__exit__(None, None, None)
            raise
        return self

    def __exit__(self, *_: object) -> None:
        self.stop.set()
        for thread in self._threads:
            thread.join(timeout=3)
        for sock in self._sockets:
            sock.close()
        if any(thread.is_alive() for thread in self._threads):
            raise RuntimeError("HTTP/DNS observer did not stop")

    def _observe(self, handler, sock: socket.socket) -> None:
        try:
            handler(sock)
        except Exception as error:
            self.errors.append(error)

    def _http(self, listener: socket.socket) -> None:
        while not self.stop.is_set():
            try:
                conn, _ = listener.accept()
            except TimeoutError:
                continue
            with conn:
                conn.settimeout(0.1)
                request = bytearray()
                while not self.stop.is_set() and b"\r\n\r\n" not in request:
                    try:
                        chunk = conn.recv(4096)
                    except TimeoutError:
                        continue
                    if not chunk:
                        break
                    request.extend(chunk)
                    if len(request) > 8192:
                        raise ValueError("HTTP request exceeds controlled bound")
                self.requests.append(bytes(request))
                if self.case.reset:
                    conn.setsockopt(socket.SOL_SOCKET, socket.SO_LINGER, struct.pack("ii", 1, 0))
                    continue
                if self.case.stall:
                    self.stop.wait(60)
                    continue
                try:
                    for offset in range(0, len(self.case.response), 173):
                        conn.sendall(self.case.response[offset:offset + 173])
                        time.sleep(0.001)
                except (BrokenPipeError, ConnectionResetError):
                    # Rejection of malformed headers may precede the final write.
                    if self.case.error == "none":
                        raise

    def _dns(self, sock: socket.socket) -> None:
        while not self.stop.is_set():
            try:
                packet, source = sock.recvfrom(4096)
            except TimeoutError:
                continue
            name, question = dns_question(packet)
            if name != "example.test":
                raise ValueError(f"guest queried undeclared controlled name {name!r}")
            self.queries.append((name, int.from_bytes(packet[:2], "big"), source[1]))
            if self.case.dns == "wrong-source":
                with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as wrong_source:
                    wrong_source.bind(("127.0.0.1", 0))
                    wrong_source.sendto(dns_response(packet, question, "a", self.case.destination), source)
            elif self.case.dns != "timeout":
                sock.sendto(dns_response(packet, question, self.case.dns, self.case.destination), source)

    def qualify(self) -> None:
        if self.errors:
            raise ValueError(f"controlled peer failed: {self.errors[0]}")
        if self.case.expect_request:
            if len(self.requests) != 1:
                raise ValueError(f"expected exactly one HTTP request, observed {len(self.requests)}")
            request = self.requests[0]
            rows = request.split(b"\r\n")
            if rows[0] != f"GET {self.case.path} HTTP/1.1".encode():
                raise ValueError("HTTP method/path/version differs from the launched URL")
            headers = rows[1:-2]
            authority = self.case.host.lower() + (f":{self.case.port}" if self.case.port != 80 else "")
            expected = {f"host: {authority}".encode(), b"connection: close", b"accept-encoding: identity"}
            normalized = [row.lower() for row in headers]
            if set(normalized) != expected or len(normalized) != len(expected) or rows[-2:] != [b"", b""]:
                raise ValueError("HTTP headers or request boundary differ from the bounded request")
        elif self.requests:
            raise ValueError("DNS refusal still reached the HTTP server")
        if not self.case.attached:
            if self.queries:
                raise ValueError("invalid URL unexpectedly invoked DNS")
        elif self.case.host == "10.0.2.2":
            if self.queries:
                raise ValueError("numeric address unexpectedly invoked DNS")
        elif not self.queries:
            raise ValueError("hostname request produced no real DNS query")


def relay() -> None:
    """QEMU guestfwd byte relay: preserve host EOF without interpreting HTTP."""
    with socket.create_connection(("127.0.0.1", HTTP_PORT), timeout=5) as conn:
        conn.settimeout(5)
        with selectors.DefaultSelector() as ready:
            ready.register(sys.stdin.fileno(), selectors.EVENT_READ)
            ready.register(conn, selectors.EVENT_READ)
            while True:
                events = ready.select(timeout=60)
                if not events:
                    raise TimeoutError("controlled byte relay idle bound")
                for event, _ in events:
                    if event.fileobj == conn:
                        data = conn.recv(4096)
                        if not data:
                            return
                        sys.stdout.buffer.write(data)
                        sys.stdout.buffer.flush()
                    else:
                        data = os.read(sys.stdin.fileno(), 4096)
                        if not data:
                            conn.shutdown(socket.SHUT_WR)
                            ready.unregister(sys.stdin.fileno())
                        else:
                            conn.sendall(data)


if __name__ == "__main__":
    if sys.argv[1:] != ["--relay"]:
        raise SystemExit("http_peer is a checker helper; only --relay is supported")
    relay()
