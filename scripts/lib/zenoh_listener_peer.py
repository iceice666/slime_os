"""A stock eclipse-zenoh 1.0.0 listener on the host, and the record it writes about what it received.

Two halves in one file. The importing half (`ensure_venv`, `Listener`) runs under the system
Python: it builds a virtual environment under `build/zenoh-external/` from the one wheel pinned
below with `pip --require-hashes --no-deps`, then starts the second half in that environment.
The script half (`python zenoh_listener_peer.py <port> <receipt>`) is the stock peer: mode peer,
multicast scouting and gossip off, one TCP listener on the host loopback, one subscriber on the
demo key. It writes a record of `contracts/zenoh-external-receipt/v1` when the guest's session
ends, and never reads the wire the packet capture holds.

QEMU user networking forwards the guest's `10.0.2.2` to the host's loopback, so the listener
binds `127.0.0.1` and nothing wider. The wheel is a registry download and this is an explicit
local experiment, never part of CI.
"""

from __future__ import annotations

import subprocess
import sys
import threading
import time
from pathlib import Path

from harness import ROOT

WHEEL = "eclipse_zenoh-1.0.0-cp38-abi3-manylinux_2_17_x86_64.manylinux2014_x86_64.whl"
WHEEL_SHA256 = "1763912951ecd2a3706f668faf0541245387096b37a4fefb73af7848dadd46ff"
VERSION = "1.0.0"
PORT = 17447
SCRATCH = ROOT / "build" / "zenoh-external"
VENV = SCRATCH / "venv"
REQUIREMENTS = SCRATCH / "requirements.txt"
KEY = "0/slime_demo/counter/slime_demo_msgs::msg::dds_::Counter_/RIHS01_a82fd5ffcb96d0a197a5ad3680d1c4e6ba43a962928ecd592fb565eb8129595b"
SAMPLES = 4
READY = "[zenoh-listener] ready"
# The guest's session is judged by the capture; the listener only has to stay up until the
# guest's four samples have arrived and the link has closed, or this bound passes.
DEADLINE_SECONDS = 150
READY_SECONDS = 30
SETTLE_SECONDS = 3


class ListenerError(RuntimeError):
    """The pinned peer could not be built or did not run."""


def venv_python() -> Path:
    return VENV / "bin" / "python"


def ensure_venv() -> Path:
    """The pinned peer's interpreter, built fresh from the hash-pinned wheel on every call.

    An existing environment is never reused: its metadata says nothing about the files installed,
    and only the hash-constrained install proves they are the pinned wheel's.
    """
    interpreter = venv_python()
    SCRATCH.mkdir(parents=True, exist_ok=True)
    REQUIREMENTS.write_text(f"eclipse-zenoh=={VERSION} --hash=sha256:{WHEEL_SHA256}\n", encoding="utf-8")
    steps = (
        [sys.executable, "-m", "venv", "--clear", str(VENV)],
        [str(interpreter), "-m", "pip", "install", "--quiet", "--require-hashes", "--no-deps", "--only-binary=:all:", "-r", str(REQUIREMENTS)],
    )
    for command in steps:
        done = subprocess.run(command, capture_output=True, text=True, check=False)
        if done.returncode != 0:
            raise ListenerError(f"{' '.join(command[:4])} failed: {done.stderr.strip()[-400:]}")
    return interpreter


class Listener:
    """The stock peer as a child process; the context returns once it is subscribed."""

    def __init__(self, receipt: Path, port: int = PORT) -> None:
        self.receipt, self.port = receipt, port
        self.process: subprocess.Popen[str] | None = None

    def __enter__(self) -> Listener:
        interpreter = ensure_venv()
        self.receipt.unlink(missing_ok=True)
        self.process = subprocess.Popen(
            [str(interpreter), str(Path(__file__).resolve()), str(self.port), str(self.receipt)],
            stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True,
        )
        assert self.process.stdout is not None
        # readline() blocks until a newline, so a child that stays alive and silent would hold the
        # deadline hostage. A reader thread reports the first line and the wait on it is bounded.
        ready = threading.Event()
        stream = self.process.stdout
        self.output: list[str] = []

        def watch() -> None:
            # The reader runs until the child closes its output: stopping at READY would leave a
            # pipe nobody drains, and a chatty child would block on it before it wrote its record.
            for line in stream:
                self.output.append(line)
                if line.strip() == READY:
                    ready.set()

        self.reader = threading.Thread(target=watch, daemon=True)
        self.reader.start()
        if ready.wait(READY_SECONDS):
            return self
        self.__exit__(None, None, None)
        raise ListenerError(f"the stock listener did not report ready within {READY_SECONDS} s")

    def finish(self, timeout: float) -> str:
        """Wait for the peer to write its record; returns what it printed."""
        assert self.process is not None and self.process.stdout is not None
        try:
            self.process.wait(timeout=timeout)
        except subprocess.TimeoutExpired as error:
            raise ListenerError("the stock listener did not finish") from error
        self.reader.join(10)
        output = "".join(self.output)
        if self.process.returncode != 0:
            raise ListenerError(f"the stock listener exited {self.process.returncode}: {output[-300:]}")
        return output

    def __exit__(self, *_exc: object) -> None:
        if self.process is not None and self.process.poll() is None:
            self.process.terminate()
            try:
                self.process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait()


def receipt_text(listen: str, remote_peers: int, samples: list[tuple[str, bytes, bytes]]) -> str:
    rows = "".join(f'    {{ key = "{key}"; payloadHex = "{payload.hex()}"; attachmentHex = "{attachment.hex()}"; }};\n' for key, payload, attachment in samples)
    return f'{{\n  formatVersion = 1;\n  peerVersion = "{VERSION}";\n  listen = "{listen}";\n  maxRemotePeers = {remote_peers};\n  samples = [\n{rows}  ];\n}}\n'


def serve(port: int, receipt: Path) -> int:
    import json

    import zenoh

    listen = f"tcp/127.0.0.1:{port}"
    config = zenoh.Config()
    config.insert_json5("mode", json.dumps("peer"))
    config.insert_json5("listen/endpoints", json.dumps([listen]))
    config.insert_json5("scouting/multicast/enabled", "false")
    config.insert_json5("scouting/gossip/enabled", "false")
    session = zenoh.open(config)
    received: list[tuple[str, bytes, bytes]] = []
    # A sample is delivered by a remote peer, so the peers the session holds when one arrives are
    # the peers that sent it. A peer that connects, publishes and leaves between two polls is
    # still counted, because the delivery itself records it.
    most = 0
    seen = False

    def on_sample(sample: zenoh.Sample) -> None:
        nonlocal most, seen
        attachment = sample.attachment
        received.append((str(sample.key_expr), bytes(sample.payload), b"" if attachment is None else bytes(attachment)))
        most, seen = max(most, len(session.info.peers_zid()), 1), True

    subscriber = session.declare_subscriber(KEY, on_sample)
    print(READY, flush=True)
    quiet_since = None
    deadline = time.monotonic() + DEADLINE_SECONDS
    while time.monotonic() < deadline:
        peers = len(session.info.peers_zid())
        most, seen = max(most, peers), seen or peers > 0
        if seen and peers == 0 and len(received) >= SAMPLES:
            quiet_since = quiet_since or time.monotonic()
            if time.monotonic() - quiet_since >= SETTLE_SECONDS:
                break
        else:
            quiet_since = None
        time.sleep(0.2)
    receipt.write_text(receipt_text(listen, most, received), encoding="utf-8")
    print(f"[zenoh-listener] received={len(received)} remote-peers={most}", flush=True)
    # The handle is held to here so the subscriber stays declared for the whole collection.
    subscriber.undeclare()
    session.close()
    return 0


if __name__ == "__main__":
    raise SystemExit(serve(int(sys.argv[1]), Path(sys.argv[2])))
