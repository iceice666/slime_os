#!/usr/bin/env python3

"""Frame-store gate: `contracts/frame-store/v1`, its Rust reader and its MCAP converter.

The expectation lives in `scripts/lib/frame_store_exam.py` and `scripts/lib/mcap_format.py`. This
script runs the implementation against it:

1. The judges' controls: each must refuse its corrupted evidence before any verdict is trusted.
2. The committed fixture `components/lib/src/frame_store/fixture.bin` is small and valid.
3. The bag named by SLIME_LIDAR_BAG has the pinned size and SHA-256, and the exam's own MCAP
   reading finds what the item states.
4. The converter runs as
       SLIME_LIDAR_BAG=<bag> python3 tools/frame-store/convert-mcap.py \\
           --sha256 <hex> --channel <topic> --output <store>
   Two conversions of the pinned bag's /velodyne_points channel must be identical and equal the
   store the exam encodes from its MCAP reading. A small two-channel bag built from the pinned
   bag's messages must convert the same way, and the converter must refuse a bag whose digest is
   not the one named, a compressed chunk and a missing channel, by class, writing no store.
5. The Rust reader runs as `cargo test -p slime-components --no-default-features --
   frame_store::exam` with SLIME_FRAME_STORE_CORPUS naming a manifest of `<case> <path>` lines
   and SLIME_FRAME_STORE_TRANSCRIPT naming the file it writes. For each case, in manifest order, it
   writes either `[frame-store] <case> refuse <class>`, or one `accept` line, one `frame` line per
   frame with the SHA-256 of the payload slice it exposes, and an `end` line. The transcript must
   equal the exam decoder's reading of every store: the fixture, the converted store and the corpus.

`--controls-only` runs step 1 and needs neither the bag nor the implementation.
"""

from __future__ import annotations

import sys as _sys
from pathlib import Path as _Path

_sys.path.insert(0, str(_Path(__file__).resolve().parents[1] / "lib"))

import argparse
import hashlib
import os
import shutil
import subprocess
from typing import NoReturn

import frame_store_exam as exam
import mcap_format
from harness import ROOT

SCRATCH = ROOT / "build" / "frame-store"
CONVERTER_TIMEOUT_SECONDS = 600


def fail(message: str) -> NoReturn:
    print(f"frame-store: FAIL: {message}", file=_sys.stderr)
    raise SystemExit(1)


def run_controls() -> int:
    results = exam.controls()
    failed = [name for name, passed in results if not passed]
    if failed:
        fail(f"{len(failed)} of {len(results)} controls did not hold: {', '.join(failed)}")
    print(f"frame-store: {len(results)} controls held")
    return len(results)


def sha256_file(path: _Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for block in iter(lambda: handle.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def load_bag() -> tuple[_Path, bytes, mcap_format.Bag]:
    named = os.environ.get(exam.BAG_ENV)
    if not named:
        fail(f"{exam.BAG_ENV} must name the LiDAR bag lidar_ros2.mcap (SHA-256 {exam.BAG_SHA256})")
    path = _Path(named).resolve()
    if not path.is_file():
        fail(f"{exam.BAG_ENV}={named} is not a file")
    data = path.read_bytes()
    if len(data) != exam.BAG_BYTES or hashlib.sha256(data).hexdigest() != exam.BAG_SHA256:
        fail(f"{path} is not the pinned bag: {len(data)} bytes, SHA-256 {hashlib.sha256(data).hexdigest()}")
    try:
        bag = mcap_format.read(data)
    except mcap_format.McapError as error:
        fail(f"the exam's MCAP reader refuses the pinned bag: {error}")
    try:
        exam.judge_bag(bag)
    except exam.ExamError as error:
        fail(str(error))
    return path, data, bag


def convert(bag: _Path, digest: str, channel: str, output: _Path) -> subprocess.CompletedProcess[str]:
    output.unlink(missing_ok=True)
    command = [
        _sys.executable, str(exam.CONVERTER), "--sha256", digest, "--channel", channel, "--output", str(output),
    ]
    environment = {**os.environ, exam.BAG_ENV: str(bag)}
    return subprocess.run(
        command, cwd=ROOT, env=environment, capture_output=True, text=True, timeout=CONVERTER_TIMEOUT_SECONDS
    )


def converted_twice(bag: _Path, digest: str, channel: str, name: str) -> tuple[bytes, bytes]:
    outputs = []
    for run in (1, 2):
        output = SCRATCH / f"{name}-{run}.store"
        result = convert(bag, digest, channel, output)
        if result.returncode != 0 or not output.is_file():
            _sys.stderr.write(result.stderr[-4000:])
            fail(f"the converter exited {result.returncode} on {name} run {run} without writing a store")
        outputs.append(output.read_bytes())
    return outputs[0], outputs[1]


def refused(bag: _Path, digest: str, channel: str, name: str, expected: str) -> None:
    output = SCRATCH / f"{name}.store"
    result = convert(bag, digest, channel, output)
    try:
        exam.judge_refusal(result.returncode, result.stderr, output.exists(), expected)
    except exam.ExamError as error:
        fail(f"{name}: {error}")


def run_converter(bag_path: _Path, bag_data: bytes, bag: mcap_format.Bag) -> tuple[bytes, int]:
    if not exam.CONVERTER.is_file():
        fail(f"{exam.CONVERTER.relative_to(ROOT)} is missing")
    first, second = converted_twice(bag_path, exam.BAG_SHA256, exam.BAG_CHANNEL, "lidar")
    expected = exam.expected_store(bag, exam.BAG_CHANNEL, exam.BAG_PERIOD_NS)
    try:
        exam.judge_converted(first, second, expected, period_pinned=True)
    except exam.ExamError as error:
        fail(f"the pinned bag: {error}")
    print(f"frame-store: converted {len(expected.frames)} frames, {len(first)} bytes, twice identically")

    plain, plain_store, compressed = exam.synthetic_bags(bag)
    plain_path = SCRATCH / "two-channels.mcap"
    compressed_path = SCRATCH / "compressed.mcap"
    plain_path.write_bytes(plain)
    compressed_path.write_bytes(compressed)
    plain_digest = hashlib.sha256(plain).hexdigest()
    one, two = converted_twice(plain_path, plain_digest, exam.BAG_CHANNEL, "two-channels")
    try:
        exam.judge_converted(one, two, plain_store, period_pinned=False)
    except exam.ExamError as error:
        fail(f"the two-channel bag: {error}")

    # The flipped byte sits inside a message payload, so the copy is still a well-formed bag.
    flipped = bytearray(bag_data)
    message = bag.messages[0].data
    flipped[bag_data.index(message) + len(message) // 2] ^= 0x01
    flipped_path = SCRATCH / "flipped.mcap"
    flipped_path.write_bytes(flipped)
    del flipped
    refusals = (
        (flipped_path, exam.BAG_SHA256, exam.BAG_CHANNEL, "flipped-byte", "bag-hash"),
        (plain_path, exam.BAG_SHA256, exam.BAG_CHANNEL, "other-bag", "bag-hash"),
        (compressed_path, hashlib.sha256(compressed).hexdigest(), exam.BAG_CHANNEL, "compressed", "compressed-chunk"),
        (bag_path, exam.BAG_SHA256, "/velodyne_point", "channel-prefix", "channel"),
        (bag_path, exam.BAG_SHA256, "velodyne_points", "channel-unrooted", "channel"),
        (plain_path, plain_digest, "/missing", "channel-missing", "channel"),
    )
    for path, digest, channel, name, expected_class in refusals:
        refused(path, digest, channel, name, expected_class)
    flipped_path.unlink()
    print(f"frame-store: the converter refused {len(refusals)} bags by class")
    return first, 2 + len(refusals)


def run_reader(converted: bytes) -> int:
    corpus = SCRATCH / "corpus"
    corpus.mkdir()
    manifest = SCRATCH / "corpus.txt"
    transcript = SCRATCH / "transcript.txt"
    stores = [("fixture", lambda: exam.FIXTURE.read_bytes()), ("converted", lambda: converted)]
    stores += [(case.name, case.build) for case in exam.cases()]
    expected: list[str] = []
    rows = []
    for name, build in stores:
        data = build()
        path = corpus / f"{name}.store"
        path.write_bytes(data)
        expected += exam.expected_lines(name, data)
        rows.append(f"{name} {path}\n")
        del data
    manifest.write_text("".join(rows))

    host = subprocess.run(["rustc", "-vV"], capture_output=True, text=True, check=True)
    target = next(line.split(": ", 1)[1] for line in host.stdout.splitlines() if line.startswith("host: "))
    command = [
        "cargo", "test", "--target", target, "-p", "slime-components", "--no-default-features",
        "--", exam.RUST_FILTER, "--test-threads=1",
    ]
    environment = {
        **os.environ, "SLIME_FRAME_STORE_CORPUS": str(manifest), "SLIME_FRAME_STORE_TRANSCRIPT": str(transcript),
    }
    result = subprocess.run(command, cwd=ROOT / "components", capture_output=True, text=True, env=environment)
    output = result.stdout + result.stderr
    if result.returncode != 0:
        _sys.stderr.write(output[-4000:])
        fail(f"{' '.join(command)} exited {result.returncode}")
    try:
        tests = exam.judge_test_run(output)
        if not transcript.is_file():
            raise exam.ExamError(f"the Rust exam wrote no transcript at {transcript}")
        exam.judge_transcript(transcript.read_text(encoding="utf-8"), expected)
    except exam.ExamError as error:
        fail(str(error))
    print(f"frame-store: the Rust reader matched {len(stores)} stores in {tests} test(s)")
    return len(stores)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n", 1)[0])
    parser.add_argument("--controls-only", action="store_true", help="run only the judges' controls")
    arguments = parser.parse_args()

    findings = exam.schema_findings(exam.SCHEMA.read_text(encoding="utf-8"))
    if findings:
        fail("the schema differs from the exam: " + "; ".join(findings))
    controls = run_controls()
    if arguments.controls_only:
        return 0

    if not exam.FIXTURE.is_file():
        fail(f"{exam.FIXTURE.relative_to(ROOT)} is missing")
    try:
        exam.judge_fixture(exam.FIXTURE.read_bytes())
    except exam.ExamError as error:
        fail(str(error))

    bag_path, bag_data, bag = load_bag()
    shutil.rmtree(SCRATCH, ignore_errors=True)
    SCRATCH.mkdir(parents=True)
    converted, conversions = run_converter(bag_path, bag_data, bag)
    del bag_data
    stores = run_reader(converted)
    shutil.rmtree(SCRATCH / "corpus", ignore_errors=True)
    print(f"frame-store: PASS: {conversions} converter cases, {stores} reader cases, {controls} controls")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
