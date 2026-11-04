#!/usr/bin/env python3

"""Judge the Foxglove bridge: `tools/foxglove-bridge/bridge.py` between a Zenoh replayer and a client.

The expectation lives in `scripts/lib/foxglove_bridge_exam.py`, and the client in `scripts/lib/foxglove_ws.py`.
This script runs the bridge as a command and imports none of it:

    python3 tools/foxglove-bridge/bridge.py --store FILE --zenoh-port P --websocket-port Q [--pins FILE]
        listen for Zenoh on tcp/127.0.0.1:P and serve Foxglove on ws://127.0.0.1:Q until SIGTERM

Steps:

1. The exam's judges hold their controls, and the bridge's committed pin file names exactly the exam's wheels.
2. The bridge, started with a pin file whose eclipse-zenoh hash for this host is changed, refuses as a
   dependency and never listens.
3. The frame store is encoded by the exam from the bag `SLIME_LIDAR_BAG` names, pinned by size and SHA-256.
   The bridge starts on it. A client connects and subscribes the channel the bridge advertises.
4. A stock eclipse-zenoh 1.0.0 peer, from the external Zenoh checks' pinned wheel, puts one sample on
   another key under domain 0 and every frame on the store's key.
5. The session the client saw and the bridge's own lines are judged against the store.

Runs entirely on the host and needs no QEMU. The bag and a registry are needed, so the recipe is explicit and
in no aggregate. `--controls-only` runs step 1's controls.
"""

from __future__ import annotations

import sys as _sys
from pathlib import Path as _Path

_sys.path.insert(0, str(_Path(__file__).resolve().parents[1] / "lib"))

import argparse
import os
import platform
import signal
import socket
import subprocess
import tempfile
import threading
import time
from pathlib import Path
from typing import NoReturn

import devloop_observations
import foxglove_bridge_exam as exam
import foxglove_ws as ws
import lidar_publish_exam
import zenoh_listener_peer
from harness import ROOT

REPLAYER = ROOT / "scripts" / "lib" / "zenoh_replayer.py"
REFUSAL_SECONDS = 600
STOP_SECONDS = 30


def fail(message: str) -> NoReturn:
    raise SystemExit(f"foxglove bridge check: {message}")


def free_port() -> int:
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        return probe.getsockname()[1]


def listening(port: int) -> bool:
    try:
        with socket.create_connection(("127.0.0.1", port), timeout=0.2):
            return True
    except OSError:
        return False


class Bridge:
    """The bridge as a child process, its stdout read on a thread so a chatty child never blocks."""

    def __init__(self, store: Path, zenoh_port: int, websocket_port: int, pins: Path | None = None) -> None:
        command = [_sys.executable, str(exam.BRIDGE), "--store", str(store), "--zenoh-port", str(zenoh_port), "--websocket-port", str(websocket_port)]
        if pins is not None:
            command += ["--pins", str(pins)]
        self.process = subprocess.Popen(command, cwd=ROOT, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
                                        env={**os.environ, "PYTHONDONTWRITEBYTECODE": "1"})
        self.stdout: list[str] = []
        self.stderr: list[str] = []
        self.ready = threading.Event()
        self.threads = [threading.Thread(target=self._read, args=(stream, sink), daemon=True)
                        for stream, sink in ((self.process.stdout, self.stdout), (self.process.stderr, self.stderr))]
        for thread in self.threads:
            thread.start()

    def _read(self, stream, sink: list[str]) -> None:
        for line in stream:
            sink.append(line)
            if exam.READY.fullmatch(line.strip()):
                self.ready.set()

    def stop(self) -> int:
        if self.process.poll() is None:
            self.process.send_signal(signal.SIGTERM)
        try:
            self.process.wait(timeout=STOP_SECONDS)
        except subprocess.TimeoutExpired:
            self.process.kill()
            self.process.wait()
            fail("the bridge did not stop within 30 s of SIGTERM")
        for thread in self.threads:
            thread.join(5)
        return self.process.returncode


def controls() -> int:
    try:
        accepted, refused = exam.controls()
    except exam.ExamError as error:
        fail(str(error))
    print(f"[foxglove-bridge] controls accepted={accepted} refused={refused}")
    return refused


def step_refusal(directory: Path, store: Path) -> None:
    marker = exam.host_marker(_sys.platform, platform.machine())
    pins = directory / "altered-pins.txt"
    pins.write_text(exam.altered_pins(exam.PINS.read_text(encoding="utf-8"), marker), encoding="utf-8")
    zenoh_port, websocket_port = free_port(), free_port()
    bridge = Bridge(store, zenoh_port, websocket_port, pins)
    listened: list[str] = []
    deadline = time.monotonic() + REFUSAL_SECONDS
    while bridge.process.poll() is None and time.monotonic() < deadline:
        listened += [name for name, port in (("zenoh", zenoh_port), ("websocket", websocket_port)) if name not in listened and listening(port)]
        time.sleep(0.2)
    returncode = bridge.process.poll()
    if returncode is None:
        bridge.stop()
    for thread in bridge.threads:
        thread.join(5)
    try:
        exam.judge_refusal(returncode, "".join(bridge.stderr), listened)
    except exam.ExamError as error:
        fail(f"refusal: {error}")
    print("[foxglove-bridge] a changed pin is refused as a dependency and nothing listens")


def step_run(store_path: Path, want: exam.Store) -> int:
    zenoh_port, websocket_port = free_port(), free_port()
    bridge = Bridge(store_path, zenoh_port, websocket_port)
    if not bridge.ready.wait(exam.READY_SECONDS):
        bridge.stop()
        fail(f"the bridge printed no ready line within {exam.READY_SECONDS} s: {''.join(bridge.stderr)[-400:]}")
    try:
        client = ws.Client("127.0.0.1", websocket_port, timeout=10)
        before: list[ws.Message] = []
        deadline = time.monotonic() + 5
        while (message := client.receive(deadline)) is not None:
            before.append(message)
        advertised = [m.json() for m in before if m.text is not None and m.json().get("op") == "advertise"]
        if len(advertised) != 1 or len(advertised[0].get("channels", [])) != 1:
            exam.judge_session(before, [], want)
        channel = advertised[0]["channels"][0]["id"]
        client.send_json({"op": "subscribe", "subscriptions": [{"id": exam.SUBSCRIPTION, "channelId": channel}]})
        interpreter = zenoh_listener_peer.ensure_venv()
        replay = subprocess.run(
            [str(interpreter), "-E", str(REPLAYER), str(zenoh_port), str(store_path), lidar_publish_exam.KEY,
             exam.OTHER_KEY, str(exam.REPLAY_PERIOD_MS)],
            capture_output=True, text=True, timeout=len(want.frames) * exam.REPLAY_PERIOD_MS / 1000 + 300,
            env={k: v for k, v in os.environ.items() if not k.startswith(("PYTHON", "PIP_"))},
        )
        if replay.returncode != 0:
            fail(f"the replayer exited {replay.returncode}: {(replay.stdout + replay.stderr)[-400:]}")
        after: list[ws.Message] = []
        deadline = time.monotonic() + exam.SETTLE_SECONDS
        while (message := client.receive(deadline)) is not None:
            after.append(message)
            deadline = time.monotonic() + exam.SETTLE_SECONDS
        client.close()
        exam.judge_session(before, after, want)
    except (exam.ExamError, ws.ProtocolError, OSError) as error:
        bridge.stop()
        fail(f"session: {error}; bridge said {''.join(bridge.stdout)[-300:]!r}")
    returncode = bridge.stop()
    try:
        exam.judge_lines("".join(bridge.stdout), returncode, zenoh_port, websocket_port, len(want.frames))
    except exam.ExamError as error:
        fail(f"bridge lines: {error}")
    print(f"[foxglove-bridge] messages={len(after)} bytes={sum(len(p) for _, p in want.frames)} channel={channel} dropped=1")
    return len(after)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.split("\n", 1)[0])
    parser.add_argument("--controls-only", action="store_true", help="run the judges against their controls only")
    arguments = parser.parse_args()
    if Path.cwd().resolve() != ROOT:
        fail(f"run from repository root: {ROOT}")
    refused = controls()
    if arguments.controls_only:
        return
    for path in (exam.BRIDGE, exam.PINS):
        if not path.is_file():
            fail(f"{path.relative_to(ROOT)} does not exist; the bridge has not landed")
    try:
        exam.judge_pins(exam.PINS.read_text(encoding="utf-8"))
        data = lidar_publish_exam.store_from_bag(os.environ.get("SLIME_LIDAR_BAG"))
        want = exam.store_of(data)
    except (exam.ExamError, lidar_publish_exam.ExamError) as error:
        fail(str(error))
    with tempfile.TemporaryDirectory(prefix="slime-foxglove-bridge-") as name:
        directory = Path(name)
        store = directory / "lidar.store"
        store.write_bytes(data)
        step_refusal(directory, store)
        messages = step_run(store, want)
    devloop_observations.record(casesObserved=messages, bytesObserved=sum(len(p) for _, p in want.frames), negativeControlsRefused=refused)
    print("foxglove bridge check: a stock Zenoh replayer's every frame reached an independent Foxglove client through the bridge, byte for byte and in order, and the bridge refused a changed pin and a sample on another key")


if __name__ == "__main__":
    main()
