#!/usr/bin/env python3

"""P5.2 gate: a generation of native ELF component images boots its declared
graph on seL4.

Builds the `sel4` closure and boots its packaged image, asserting that init launches its declared
services with generation-derived native Endpoint capabilities, the services
exercise their bounded operation surface, and their explicit userspace shutdown
and supervision protocol completes before the graph is certified healthy. Raw
Endpoint closure is deliberately not lifecycle evidence: the kernel object
supplies rendezvous transport, not a service-termination protocol.

Modelled on `check-sel4-root-boot.py`, which guards P5.1 against the other
image. The images are separate artifacts on purpose: each gate boots the one it
asserts about, so neither invalidates the other's evidence by being built last.
"""

from __future__ import annotations

import argparse
import copy
import importlib.util
import json
import os
import re
import shutil
import subprocess
import sys
import threading
import time
import tomllib
from pathlib import Path
from typing import NamedTuple, NoReturn

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "lib"))

from harness import (  # noqa: E402
    GENERATION_COMPOSITIONS,
    GENERATION_CONTRACT,
    profile_integer,
    profile_text,
    sha256_file,
)
from closure_image import ClosureImageError, build as build_closure_image  # noqa: E402
from zutai_cli import STDLIB, binary  # noqa: E402

ROOT = Path(__file__).resolve().parents[2]
PINS_PATH = ROOT / "sel4" / "pins.toml"
CLOSURE = "sel4"
# `--no-build` is used only by `check-external-component-admission.py`, which
# builds a mixed-source generation through the legacy image builder's
# `--component-graph --prebuilt-generation` path itself and boots the result
# this gate already knows how to check. That legacy flag still writes this
# fixed path, so `--no-build` reads it directly instead of resolving a
# closure this gate never invokes the legacy builder to produce.
LEGACY_IMAGE = ROOT / "build" / "slime-sel4-graph.elf"
IMAGE: Path | None = None
GENERATOR = ROOT / "scripts" / "build" / "build-generation.py"
FIXTURE = GENERATION_COMPOSITIONS / "sel4.zti"
AUTOMATIC_BINDING_SLOTS = {
    "spawn-service-rpc": 0,
    "spawn-service-sysinfo": 1,
    "spawn-service-sysinfo-context": 3,
}

BOOT_TIMEOUT_SECONDS = 120
INPUT_WAIT_MARKER = r"\[slisp\] resident input wait"

# The product generation carries Slisp beside console and spawn-service.
# Init launches all three and stays alive supervising them. After the first
# `WouldBlock` marker and a short startup-drain interval, this gate sends one
# expression and then the `sysinfo` command through QEMU serial stdin. Pauses
# between bytes force the FIFO empty between keystrokes and catch diagnostics
# redrawn in the middle of a command without racing one-time service startup
# logs. The command arm also proves the shell reaches generation-authorized
# spawn-service dispatch and the child receives its declared launch context.
TERMINAL_MARKER = r"=> spawned sysinfo"

REQUIRED_MARKERS: tuple[tuple[str, str], ...] = (
    (
        "generation admitted",
        r"SLIME_ROOT generation admitted number=1 executables=6 instances=6 grants=\d+ ",
    ),
    ("authority manifest reported", r"SLIME_ROOT authority manifest=\["),
    (
        "all catalogue payloads are native ELF images",
        r"SLIME_ROOT graph admitted executables=6 instances=6 slimecm=0 elf=6 unrecognized=0",
    ),
    # C10.2: this generation declares no `privateMemoryBudget` at all, which is
    # the case 22 of the 33 fixtures are in and which the private-memory plane
    # cannot state — that plane exists precisely to carry a budget. `declared=0`
    # is the root reporting that it found no budget resource, printed once
    # before any instance is constructed; the paired failure markers below are
    # what make "and therefore every component is denied" an assertion rather
    # than an inference.
    (
        "the generation declares no private-memory budget",
        r"SLIME_MEM budget holders=0 declared=0",
    ),
    (
        "only root-owned init was staged",
        r"SLIME_GRAPH staged task=0 instance=init executable=init grants=6 bindings=6 window=0x[0-9a-f]+ frames=[1-9]\d* tables=[1-9]\d* entry=0x[0-9a-f]+",
    ),
    (
        "the executable catalogue remained available to spawn",
        r"SLIME_GRAPH staged instances=1 root_autostart=1 loadable_executables=6 slimecm=0 wrong_target=0 unrecognized=0",
    ),
    ("only init was root-activated", r"SLIME_GRAPH activated instances=1"),
    ("init began the declared graph", r"\[init\] launching component graph"),
    (
        "init authorized console through its executable binding",
        r"SLIME_GRAPH spawn authorized task=0 slot=1 component=console grants=0",
    ),
    (
        "console received its installed native Endpoint capability",
        r"SLIME_GRAPH native endpoint task=1 slot=33 side=both",
    ),
    (
        "init spawned console as instance task 1",
        r"SLIME_GRAPH spawned task=0 child=1 component=console grants=0 endpoints=1 notifications=0 handle=\d+ supervision_grants=0 buffer_factory_grants=0",
    ),
    (
        "init authorized spawn-service through its executable binding",
        r"SLIME_GRAPH spawn authorized task=0 slot=5 component=spawn-service grants=3",
    ),
    (
        "spawn-service received its installed native Endpoint capability",
        r"SLIME_GRAPH native endpoint task=2 slot=33 side=both",
    ),
    (
        "init spawned spawn-service as instance task 2",
        r"SLIME_GRAPH spawned task=0 child=2 component=spawn-service grants=3 endpoints=3 notifications=0 handle=\d+ supervision_grants=0 buffer_factory_grants=1",
    ),
    (
        "init authorized Slisp through its executable binding",
        r"SLIME_GRAPH spawn authorized task=0 slot=9 component=slisp grants=0",
    ),
    (
        "Slisp received its two declared service endpoints",
        r"SLIME_GRAPH native endpoint task=3 slot=33 side=both",
    ),
    (
        "Slisp received its second declared service endpoint",
        r"SLIME_GRAPH native endpoint task=3 slot=35 side=both",
    ),
    (
        "init spawned Slisp as instance task 3",
        r"SLIME_GRAPH spawned task=0 child=3 component=slisp grants=0 endpoints=2 notifications=0 handle=\d+ supervision_grants=0 buffer_factory_grants=0",
    ),
    (
        "the supervisor certified the live graph",
        r"SLIME_GRAPH healthy generation=1 instances=[0-9a-f]{16} required=4 live=4 idle=4 failed=0",
    ),
    ("init kept the product graph resident", r"\[init\] product services resident"),
    ("the product identified the Slisp shell", r"Slisp"),
    ("Slisp displayed its prompt", r"slisp> "),
    ("Slisp entered resident input wait", INPUT_WAIT_MARKER),
    ("Slisp received uninterrupted QEMU serial input", r"\(\+ 1 1\)\n=> 2"),
    ("Slisp requested sysinfo through spawn-service", r"sysinfo\n\[spawn-service\] request"),
    ("sysinfo completed through the generation profile", r"\[sysinfo\] spawned through profile"),
    ("sysinfo exited cleanly", r"SLIME_GRAPH component exit task=\d+ status=0"),
    (
        "spawn-service collected detached supervision",
        r"SLIME_GRAPH supervision collected task=2 child=\d+ kind=0",
    ),
    ("Slisp reported the accepted spawn", TERMINAL_MARKER),
)

# Component-spec evidence literal: startup scheduling may print this after the
# terminal Slisp marker, so product admission checks its declared source string
# without making it part of the bounded transcript prefix.
SPAWN_SERVICE_READY = r"\[spawn-service\] ready"
EXPECTED_UNORDERED: tuple[str, ...] = ()

# IO8: the product graph plus `nvt-pwm-driver`, on QEMU, where no PWM block
# exists. Init launches the driver before Slisp (slot 10, task 3), the root
# installs its declared device quota, the driver binds nothing and stays
# resident, and Slisp's `(pwm 0 1600)` reaches it and comes back refused.
# The task numbers and endpoint slots are the ones the root printed on the
# frozen transcript in `devlog/2026-09-13-io8-pwm-servo/`.
PWM_TERMINAL_MARKER = TERMINAL_MARKER
PWM_REQUIRED_MARKERS: tuple[tuple[str, str], ...] = (
    (
        "generation admitted",
        r"SLIME_ROOT generation admitted number=55 executables=7 instances=7 grants=\d+ ",
    ),
    ("authority manifest reported", r"SLIME_ROOT authority manifest=\["),
    (
        "all catalogue payloads are native ELF images",
        r"SLIME_ROOT graph admitted executables=7 instances=7 slimecm=0 elf=7 unrecognized=0",
    ),
    (
        "the generation declares no private-memory budget",
        r"SLIME_MEM budget holders=0 declared=0",
    ),
    (
        "only root-owned init was staged",
        r"SLIME_GRAPH staged task=0 instance=init executable=init grants=7 bindings=7 window=0x[0-9a-f]+ frames=[1-9]\d* tables=[1-9]\d* entry=0x[0-9a-f]+",
    ),
    (
        "the executable catalogue remained available to spawn",
        r"SLIME_GRAPH staged instances=1 root_autostart=1 loadable_executables=7 slimecm=0 wrong_target=0 unrecognized=0",
    ),
    ("only init was root-activated", r"SLIME_GRAPH activated instances=1"),
    ("init began the declared graph", r"\[init\] launching component graph"),
    (
        "init authorized console through its executable binding",
        r"SLIME_GRAPH spawn authorized task=0 slot=1 component=console grants=0",
    ),
    (
        "console received its installed native Endpoint capability",
        r"SLIME_GRAPH native endpoint task=1 slot=33 side=both",
    ),
    (
        "init spawned console as instance task 1",
        r"SLIME_GRAPH spawned task=0 child=1 component=console grants=0 endpoints=1 notifications=0 handle=\d+ supervision_grants=0 buffer_factory_grants=0",
    ),
    (
        "init authorized spawn-service through its executable binding",
        r"SLIME_GRAPH spawn authorized task=0 slot=5 component=spawn-service grants=3",
    ),
    (
        "spawn-service received its installed native Endpoint capability",
        r"SLIME_GRAPH native endpoint task=2 slot=33 side=both",
    ),
    (
        "init spawned spawn-service as instance task 2",
        r"SLIME_GRAPH spawned task=0 child=2 component=spawn-service grants=3 endpoints=3 notifications=0 handle=\d+ supervision_grants=0 buffer_factory_grants=1",
    ),
    (
        "init authorized the pwm driver through its executable binding",
        r"SLIME_GRAPH spawn authorized task=0 slot=10 component=nvt-pwm-driver grants=0",
    ),
    (
        "the pwm driver received Slisp's declared endpoint",
        r"SLIME_GRAPH native endpoint task=3 slot=33 side=both",
    ),
    (
        "the root installed the pwm driver's declared device quota",
        r"SLIME_IO quota task=3 instance=nvt-pwm-driver devices=0 shared_granule=0",
    ),
    (
        "init spawned the pwm driver as instance task 3",
        r"SLIME_GRAPH spawned task=0 child=3 component=nvt-pwm-driver grants=0 endpoints=1 notifications=0 handle=\d+ supervision_grants=0 buffer_factory_grants=0",
    ),
    (
        "init authorized Slisp through its executable binding",
        r"SLIME_GRAPH spawn authorized task=0 slot=9 component=slisp grants=0",
    ),
    (
        "Slisp received its three declared service endpoints",
        r"SLIME_GRAPH native endpoint task=4 slot=33 side=both",
    ),
    (
        "Slisp received its second declared service endpoint",
        r"SLIME_GRAPH native endpoint task=4 slot=36 side=both",
    ),
    (
        "Slisp received its third declared service endpoint",
        r"SLIME_GRAPH native endpoint task=4 slot=35 side=both",
    ),
    (
        "init spawned Slisp as instance task 4",
        r"SLIME_GRAPH spawned task=0 child=4 component=slisp grants=0 endpoints=3 notifications=0 handle=\d+ supervision_grants=0 buffer_factory_grants=0",
    ),
    (
        "the supervisor certified the live graph",
        r"SLIME_GRAPH healthy generation=55 instances=[0-9a-f]{16} required=5 live=5 idle=5 failed=0",
    ),
    ("init kept the product graph resident", r"\[init\] product services resident"),
    ("the product identified the Slisp shell", r"Slisp"),
    ("Slisp displayed its prompt", r"slisp> "),
    ("Slisp entered resident input wait", INPUT_WAIT_MARKER),
    ("Slisp received uninterrupted QEMU serial input", r"\(\+ 1 1\)\n=> 2"),
    (
        "Slisp carried the pwm request to the driver",
        r"\(pwm 0 1600\)\n\[nvt-pwm-driver\] request ch=0 period_us=20000 pulse_us=1600",
    ),
    ("the driver refused for want of a device and Slisp reported it", r"! pwm no-device"),
    ("Slisp requested sysinfo through spawn-service", r"sysinfo\n\[spawn-service\] request"),
    ("sysinfo completed through the generation profile", r"\[sysinfo\] spawned through profile"),
    ("sysinfo exited cleanly", r"SLIME_GRAPH component exit task=\d+ status=0"),
    (
        "spawn-service collected detached supervision",
        r"SLIME_GRAPH supervision collected task=2 child=\d+ kind=0",
    ),
    ("Slisp reported the accepted spawn", PWM_TERMINAL_MARKER),
)
# The driver reports its bind on its own schedule, before or after Slisp's
# startup lines.
PWM_EXPECTED_UNORDERED: tuple[str, ...] = (
    r"\[nvt-pwm-driver\] device absent, refusing requests",
)


class Composition(NamedTuple):
    closure: str
    fixture: Path
    required: tuple[tuple[str, str], ...]
    unordered: tuple[str, ...]
    commands: tuple[str, ...]
    terminal: str
    summary: str


COMPOSITIONS: dict[str, Composition] = {
    "sel4": Composition(
        closure="sel4",
        fixture=FIXTURE,
        required=REQUIRED_MARKERS,
        unordered=EXPECTED_UNORDERED,
        commands=("(+ 1 1)\n", "sysinfo\n"),
        terminal=TERMINAL_MARKER,
        summary=(
            "seL4 component graph check: init launched console, spawn-service, and "
            "Slisp with generation-declared authority; QEMU serial input evaluated "
            "and launched sysinfo through its declared context endpoint; all four "
            "required resident instances remained live"
        ),
    ),
    "sel4-pwm": Composition(
        closure="sel4-pwm",
        fixture=GENERATION_COMPOSITIONS / "sel4-pwm.zti",
        required=PWM_REQUIRED_MARKERS,
        unordered=PWM_EXPECTED_UNORDERED,
        commands=("(+ 1 1)\n", "(pwm 0 1600)\n", "sysinfo\n"),
        terminal=PWM_TERMINAL_MARKER,
        summary=(
            "seL4 pwm graph check: init launched the pwm driver before Slisp with "
            "its declared device quota; on QEMU the driver bound no device and stayed "
            "resident; Slisp's (pwm 0 1600) reached it and came back no-device; "
            "sysinfo still launched and all five required resident instances "
            "remained live"
        ),
    ),
}
# Both arms for the gate control, which mutates every marker of each.
CHAINS: tuple[tuple[str, tuple[str, ...]], ...] = (
    ("required marker sequence", tuple(pattern for _, pattern in REQUIRED_MARKERS)),
    ("pwm marker sequence", tuple(pattern for _, pattern in PWM_REQUIRED_MARKERS)),
) + tuple(("order-independent marker", (pattern,)) for pattern in PWM_EXPECTED_UNORDERED)

# B50 is a repository-wide cutover. Guard every surviving implementation source
# that could reintroduce the universal dispatcher or product-plane selection;
# generated outputs, historical contracts, docs, and negative-test selectors
# are intentionally outside this exact-source check.
COMPATIBILITY_SOURCE_ROOTS = (
    ROOT / "slime-root" / "src",
    ROOT / "components" / "runtime" / "src",
    # CP3: component sources live below lifecycle-owned roots, with shared
    # helpers in `components/lib` and build-time support in
    # `components/build-support`. The whole component tree is scanned so a new
    # category cannot escape this repository-wide compatibility guard.
    ROOT / "components",
)
# The files that actually *held* the deleted model symbols: the manifest and
# wire schemas that named `endpointCreate` as a right, the builder that encoded
# it, and the decoder plus independent checker that read it back. A guard over
# the Rust component tree alone could not fail on any of them, so the symbols
# would have been unenforced exactly where they lived.
COMPATIBILITY_SOURCE_FILES = (
    ROOT / "scripts" / "build" / "build-generation.py",
    ROOT / "scripts" / "check" / "check-generation.py",
    GENERATION_CONTRACT / "schema.zt",
    ROOT / "contracts" / "generation" / "v5" / "schema.zt",
    ROOT / "contracts" / "generation" / "v5" / "gen_rust.zt",
    ROOT / "boot-contracts" / "src" / "generation.rs",
)

FAILURE_MARKERS: tuple[str, ...] = (
    r"SLIME_ROOT FATAL .*",
    r"SLIME_GRAPH FAIL .*",
    r"SLIME_GRAPH component exit .*status=-?[1-9]\d*",
    # A component that could not bind its transfer window would issue no
    # windowed operation at all, and the graph would look quiet rather than
    # broken.
    r"\[slime-rt\] transfer window bind failed",
    # A component image refused after admission, or a payload that reached the
    # loader without being admitted.
    r"SLIME_GRAPH window bind refused",
    # The root task's service loop never draining is a livelock, not a slow
    # component.
    r"SLIME_GRAPH service budget exhausted",
    # seL4's own complaints. `read-only endpoint cap` in particular means a
    # component cannot invoke the root at all, which is silent from the Slime
    # side: the component simply never speaks.
    r"Attempted to invoke a read-only endpoint",
    r"seL4 called fail",
    r"Caught cap fault",
    r"Caught vm fault",
    r"Caught user exception",
    r"panicked at ",
    r"aborted at ",
    r"\(aborted\)",
    # C10.2: with no budget declared, no component may hold a private-memory
    # ceiling and none may grow a page. Two markers rather than one, because
    # they fail on different defects: a nonzero `installed=` means a quota was
    # installed with nothing declaring it, and a served growth means the
    # mechanism handed a page to a holder no generation named.
    r"SLIME_MEM quota task=\d+ instance=\S+ declared=0 installed=[1-9]\d*",
    r"SLIME_MEM grown task=\d+ delta=[1-9]\d*",
)


def fail(message: str) -> NoReturn:
    raise SystemExit(f"seL4 component graph check: {message}")


def load_pins() -> dict[str, object]:
    if not PINS_PATH.is_file():
        fail(f"missing pin manifest: {PINS_PATH.relative_to(ROOT)}")
    try:
        pins = tomllib.loads(PINS_PATH.read_text(encoding="utf-8"))
    except (OSError, tomllib.TOMLDecodeError) as error:
        fail(f"cannot parse {PINS_PATH.relative_to(ROOT)}: {error}")
    if pins.get("schema") != 1:
        fail("unsupported sel4/pins.toml schema (expected 1)")
    if not isinstance(pins.get("qemu_arm_virt"), dict):
        fail("sel4/pins.toml is missing [qemu_arm_virt]")
    return pins


def generator_module(name: str):
    spec = importlib.util.spec_from_file_location(name, GENERATOR)
    if spec is None or spec.loader is None:
        fail("cannot import the generation builder")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def check_automatic_binding_slots(fixture: Path) -> None:
    """Omitted product bindings must resolve to the frozen layout."""
    environment = dict(os.environ)
    environment["ZUTAI_STDLIB_ROOT"] = str(STDLIB)
    process = subprocess.run(
        [str(binary()), "json", str(fixture)],
        cwd=ROOT,
        env=environment,
        check=False,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    if process.returncode:
        fail(f"cannot decode {fixture.relative_to(ROOT)}: {process.stderr.strip()}")
    try:
        manifest = json.loads(process.stdout)
    except json.JSONDecodeError as error:
        fail(f"cannot parse decoded {fixture.relative_to(ROOT)}: {error}")
    spawn_service = next(
        (instance for instance in manifest["instances"] if instance["name"] == "spawn-service"),
        None,
    )
    if spawn_service is None:
        fail("product generation declares no spawn-service instance")
    bindings = {binding["grant"]: binding for binding in spawn_service["bindings"]}
    for grant in AUTOMATIC_BINDING_SLOTS:
        binding = bindings.get(grant)
        if binding is None:
            fail(f"spawn-service does not bind {grant}")
        if "slot" in binding:
            fail(f"spawn-service/{grant} redundantly pins slot {binding['slot']}")

    resolved = generator_module("slime_build_generation_product_slots").assign_declared_slots(
        copy.deepcopy(manifest)
    )
    resolved_spawn = next(
        instance for instance in resolved["instances"] if instance["name"] == "spawn-service"
    )
    resolved_bindings = {
        binding["grant"]: binding["slot"] for binding in resolved_spawn["bindings"]
    }
    for grant, expected in AUTOMATIC_BINDING_SLOTS.items():
        if resolved_bindings.get(grant) != expected:
            fail(
                f"spawn-service/{grant} resolved to slot {resolved_bindings.get(grant)}, "
                f"expected {expected}"
            )
    print(
        f"product manifest: {len(AUTOMATIC_BINDING_SLOTS)} binding slots "
        "omitted and resolved unchanged",
        flush=True,
    )


def build_image(closure: str) -> None:
    global IMAGE
    try:
        built = build_closure_image(closure)
    except ClosureImageError as error:
        fail(str(error))
    actual = sha256_file(built.image, fail)
    if actual != built.digest():
        fail(
            f"{built.image} SHA-256 is {actual}, but the build result records "
            f"{built.digest()}; the image changed after it was built"
        )
    IMAGE = built.image




def boot(profile: dict[str, object], composition: Composition) -> str:
    """Boot the image and return the serial transcript.

    The root task suspends itself once the graph has drained, so QEMU stays
    alive afterwards and waiting for an exit would always time out. Serial
    output is read line by line and the guest is killed as soon as the terminal
    or any failure marker appears.
    """
    qemu = shutil.which("qemu-system-aarch64")
    if qemu is None:
        fail("qemu-system-aarch64 is not on PATH")
    command = [
        qemu,
        "-machine",
        profile_text(profile, "machine", fail),
        "-cpu",
        profile_text(profile, "cpu", fail),
        "-smp",
        str(profile_integer(profile, "cpus", fail)),
        "-m",
        f"size={profile_integer(profile, 'memory_mib', fail)}M",
        "-nographic",
        "-serial",
        "mon:stdio",
        "-kernel",
        str(IMAGE),
    ]
    print(f"[boot] {' '.join(command)}", flush=True)
    input_wait = re.compile(INPUT_WAIT_MARKER)
    terminal = re.compile(composition.terminal)
    failures = re.compile("|".join(FAILURE_MARKERS))
    lines: list[str] = []
    try:
        process = subprocess.Popen(
            command,
            cwd=ROOT,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
            bufsize=1,
        )
    except OSError as error:
        fail(f"cannot run QEMU: {error}")
    assert process.stdin is not None
    # A wedged guest emits nothing, so the deadline cannot live in the read
    # loop; a watchdog kills QEMU, which closes the pipe and ends the loop.
    watchdog = threading.Timer(BOOT_TIMEOUT_SECONDS, process.kill)
    watchdog.start()
    sent_expression = False
    try:
        assert process.stdout is not None
        for line in process.stdout:
            lines.append(line.rstrip("\n"))
            if failures.search(line):
                break
            if not sent_expression and input_wait.search(line):
                time.sleep(0.5)
                for command in composition.commands:
                    for character in command:
                        process.stdin.write(character)
                        process.stdin.flush()
                        time.sleep(0.05)
                sent_expression = True
                continue
            if sent_expression and terminal.search(line):
                break
    finally:
        timed_out = not watchdog.is_alive()
        watchdog.cancel()
        process.terminate()
        try:
            process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()
    transcript = "\n".join(lines)
    if timed_out and terminal.search(transcript) is None:
        print(transcript)
        fail(f"boot exceeded {BOOT_TIMEOUT_SECONDS}s without completing sysinfo")
    return transcript


def report_transcript(transcript: str) -> None:
    tail = transcript.splitlines()[-40:]
    if tail:
        sys.stdout.write("--- serial transcript (tail) ---\n")
        sys.stdout.write("\n".join(tail) + "\n")
        sys.stdout.write("--- end transcript ---\n")
        sys.stdout.flush()


def check_deleted_compatibility_surface() -> None:
    forbidden = (
        "enum Operation",
        "Operation::",
        "enum Mediation",
        "Mediation::",
        "MAX_OPERATION_LABEL",
        "RETIRED_POLICY_LABELS",
        "RETIRED_INPUT_READ_LABEL",
        "RETIRED_BLOCK_TRANSACT_LABEL",
        "block_transact",
        "block_transact_sector",
        "block_transact_write",
        "serve_block_transact",
        "BlockTransaction",
        "RETIRED_STORE_TRANSACT_LABEL",
        "RETIRED_DIRECTORY_LABEL",
        "GraphTables",
        "SERVICE_ROOT_DISPATCH",
        "endpointCreate",
        "right_roles",
        "channel_aliases",
        "SLIME_SEL4_CHANNEL_CHECK",
        "SLIME_SEL4_CALL_CHECK",
        "SLIME_SEL4_OPERATION_CHECK",
        "SLIME_SEL4_STREAM_CHECK",
        "SLIME_SEL4_QOS_CHECK",
        "SLIME_SEL4_VISIBILITY_CHECK",
    )
    guarded = list(COMPATIBILITY_SOURCE_FILES)
    for source_root in COMPATIBILITY_SOURCE_ROOTS:
        guarded.extend(sorted(source_root.rglob("*.rs")))
    root_only = ("enum Operation", "Operation::")
    for path in guarded:
        if not path.is_file():
            fail(f"missing {path.relative_to(ROOT)}")
        source = path.read_text(encoding="utf-8")
        symbols = forbidden
        if path.parent != ROOT / "slime-root" / "src":
            symbols = tuple(symbol for symbol in forbidden if symbol not in root_only)
        for symbol in symbols:
            if symbol in source:
                fail(
                    f"{path.relative_to(ROOT)} retains deleted compatibility "
                    f"surface {symbol!r} (B50)"
                )
    if (ROOT / "slime-root" / "src" / "virtio_blk.rs").exists():
        fail("slime-root/src/virtio_blk.rs retains the retired product block driver (B83)")
    selector = ROOT / "slime-root" / "src" / "boot_selector_block.rs"
    if not selector.is_file():
        fail("the boot selector's bounded pre-admission block reader is missing (B83)")
    print("repository block surface: root product transaction path deleted", flush=True)
    print("repository service surface: compatibility model deleted", flush=True)


def check_transcript(transcript: str, composition: Composition) -> None:
    for pattern in FAILURE_MARKERS:
        match = re.search(pattern, transcript)
        if match is not None:
            report_transcript(transcript)
            fail(f"failure marker in serial transcript: {match.group(0)!r}")
    position = 0
    for description, pattern in composition.required:
        match = re.compile(pattern).search(transcript, position)
        if match is None:
            report_transcript(transcript)
            if re.search(pattern, transcript) is not None:
                fail(f"marker out of order: {description} ({pattern})")
            fail(f"missing marker: {description} ({pattern})")
        position = match.end()
    for pattern in composition.unordered:
        if re.search(pattern, transcript) is None:
            report_transcript(transcript)
            fail(f"missing unordered marker: {pattern}")
    # Each task's window is its own region, which is the property the pinned
    # addresses used to carry before component code size made them brittle. Two
    # tasks bound at one base would mean one staging area serving both, and a
    # payload one wrote appearing in the other's `recv`.
    bases = dict(re.findall(r"SLIME_GRAPH window bound task=(\d+) base=(0x[0-9a-f]+)", transcript))
    if len(set(bases.values())) != len(bases):
        fail(f"two tasks bound the same transfer window base: {bases}")
    print(
        f"windows: {len(bases)} tasks each bound a distinct region "
        f"({', '.join(f'task {t}@{b}' for t, b in sorted(bases.items()))})",
        flush=True,
    )

    terminals = re.findall(composition.terminal, transcript)
    if len(terminals) != 1:
        fail(f"expected exactly one sysinfo completion marker, saw {len(terminals)}")


def main() -> None:
    global IMAGE
    parser = argparse.ArgumentParser(
        description="Boot the seL4 component-graph image and assert ordered markers"
    )
    parser.add_argument(
        "--no-build",
        action="store_true",
        help="boot the already-built image instead of rebuilding it first",
    )
    parser.add_argument(
        "--composition",
        choices=sorted(COMPOSITIONS),
        default="sel4",
        help="which product composition to boot: the product graph, or it plus the pwm driver",
    )
    parser.add_argument(
        "--transcript",
        type=Path,
        default=None,
        help="also write the serial transcript to this path, for a devlog entry",
    )
    arguments = parser.parse_args()
    composition = COMPOSITIONS[arguments.composition]

    if Path.cwd().resolve() != ROOT:
        fail(f"run from repository root: {ROOT}")
    if arguments.no_build and arguments.composition != "sel4":
        fail("--no-build boots the legacy product image, which is the sel4 composition")
    pins = load_pins()
    check_automatic_binding_slots(composition.fixture)
    if arguments.no_build:
        IMAGE = LEGACY_IMAGE
    else:
        build_image(composition.closure)
    check_deleted_compatibility_surface()
    profile = pins["qemu_arm_virt"]
    assert isinstance(profile, dict)
    transcript = boot(profile, composition)
    if arguments.transcript is not None:
        arguments.transcript.parent.mkdir(parents=True, exist_ok=True)
        arguments.transcript.write_text(transcript + "\n", encoding="utf-8")
    check_transcript(transcript, composition)
    print(composition.summary)


if __name__ == "__main__":
    main()
