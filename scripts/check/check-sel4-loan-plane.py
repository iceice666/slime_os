#!/usr/bin/env python3

"""P5.3.2 gate: a loan crosses between components on seL4, against quotas the
generation declared.

Boots `build/slime-sel4-loan.elf` -- the image whose root task embeds the
loan-plane generation, `contracts/generation/v1/fixtures/sel4-loan.zti` -- and
asserts ordered markers for each half of P5.3.2's exit condition:

1. a component loans a sealed subrange to a receiver named by capability, and
   the receiver maps it read-only and returns it exactly once;
2. each of the four quota classes fails at ceiling+1 against limits decoded
   from the generation, without disturbing an unrelated holder.

The receiver is `sample-receiver`, unmodified: the same binary the x86 oracle's
`just sample_plane_live_check` runs. That it needs no change is the load-bearing
claim -- a component written against the retired kernel's loan ABI runs on seL4
because the ABI is the same one, not because the scenario was rewritten to suit
whatever the root task happened to implement.

Modelled on `check-sel4-channel-plane.py`, which guards P5.3.1 against a
different image. The four seL4 images are separate artifacts on purpose: each
gate boots the one it asserts about, so none invalidates another's evidence by
being built last.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import shutil
import subprocess
import sys
import threading
import tomllib
from pathlib import Path
from typing import NoReturn

ROOT = Path(__file__).resolve().parents[2]
PINS_PATH = ROOT / "sel4" / "pins.toml"
IMAGE = ROOT / "build" / "slime-sel4-loan.elf"
MANIFEST = ROOT / "build" / "slime-sel4-loan.identity.json"
BUILD_SCRIPT = ROOT / "scripts" / "build" / "build-sel4.py"
IMAGE_VARIANT = "loan"

BOOT_TIMEOUT_SECONDS = 120

# The loaned payload, in bytes. Two pages -- far past the 64-byte control
# message bound, which is what makes the loan necessary rather than an
# optimization.
PAYLOAD_BYTES = 8192
MAX_MSG_BYTES = 64

REQUIRED_MARKERS: tuple[tuple[str, str], ...] = (
    (
        "the loan generation was admitted",
        # Five grants: three channel edges plus the two `bufferCreate` grants
        # P5.3.3 made load-bearing. Before that slice the factory slot was
        # discarded and the budget alone admitted an allocation (B13), so this
        # graph reached its loan with no factory grant declared at all.
        r"SLIME_ROOT generation admitted number=\d+ components=3 grants=5 ",
    ),
    (
        "all three payloads are native ELF and no legacy image was activated",
        r"SLIME_ROOT graph admitted; legacy SLIMECM images not activated "
        r"components=3 slimecm=0 elf=3 unrecognized=0",
    ),
    (
        # The quota half's precondition: every holder's ceiling read from the
        # generation's `shared-buffer-budget` resource, which is what makes
        # every refusal below a refusal against a *declared* limit rather than
        # against a constant compiled into the root task.
        #
        # The three per-holder lines are asserted by `check_declared_quotas`
        # rather than here, because the root prints them in task-id order and
        # task ids are assigned by staging order -- so requiring a particular
        # order among them would pin something the milestone does not claim.
        "every declared holder was budgeted",
        r"SLIME_GRAPH quotas declared=3 budgeted=3 holders=3",
    ),
    (
        # B13. The factory grant and the budget are independent gates: the
        # grant authorizes the operation, the budget bounds it. This is the
        # first, asserted before any ceiling is grazed so the refusal cannot be
        # a quota answer wearing another name -- `class=ungranted`, not
        # `class=quota`.
        #
        # Two arms in one marker pair, deliberately: an empty slot and a slot
        # holding real authority of another kind are refused identically, which
        # is what stops a component probing its own table by watching which
        # error comes back.
        "a slot holding no factory cannot allocate, whatever the budget says",
        r"SLIME_GRAPH buffer create refused task=\d+ class=ungranted",
    ),
    (
        "init observed the ungranted-factory refusal",
        r"\[init\] ungranted buffer factory refused",
    ),
    (
        # Quota class 1 of 4: pages. A five-page region against a four-page
        # ceiling, refused before a frame is allocated.
        "the page ceiling refused a region one page past it",
        r"SLIME_GRAPH buffer create refused task=\d+ pages=5 class=quota",
    ),
    ("init observed the page refusal", r"\[init\] page quota refused"),
    (
        # Quota class 2 of 4: buffers. Three single-page regions stay inside the
        # four-page budget, so it is the buffer count and nothing else that bites.
        "the buffer ceiling refused a third region",
        r"SLIME_GRAPH buffer create refused task=\d+ pages=1 class=quota",
    ),
    ("init observed the buffer refusal", r"\[init\] buffer quota refused"),
    (
        # Quota class 3 of 4: mappings. A mapping of a region already charged,
        # so no page or buffer limit is involved.
        "the mapping ceiling refused a third mapping",
        r"SLIME_GRAPH buffer map refused task=\d+ slot=\d+ class=quota",
    ),
    ("init observed the mapping refusal", r"\[init\] mapping quota refused"),
    (
        # Every probe charge handed back, so the loan below runs against
        # ceilings that are entirely unspent and its own refusals are
        # unambiguous.
        "the probes released every charge they took",
        r"\[init\] quota probes reclaimed",
    ),
    ("init wrote the payload", r"\[init\] payload written"),
    (
        # A loan requires an irreversibly sealed source. Checked before sealing,
        # because afterwards it is unobservable.
        "an unsealed region was not loanable",
        r"SLIME_GRAPH loan refused task=\d+ slot=\d+ class=unsealed",
    ),
    ("init observed the unsealed refusal", r"\[init\] unsealed loan denied"),
    (
        # "Named by capability" is the exit condition's own wording, so the ways
        # of naming a receiver badly are asserted, not just the way that works.
        # An empty slot and a slot holding real authority of the wrong kind are
        # both refused; the second is the sharper one, since it is the buffer's
        # own slot -- authority this component genuinely holds.
        "an empty slot cannot name a receiver",
        r"SLIME_GRAPH loan refused task=\d+ slot=\d+ class=absent-or-ambiguous",
    ),
    (
        "a slot holding the wrong kind cannot name a receiver",
        r"SLIME_GRAPH loan refused task=\d+ slot=\d+ class=absent-or-ambiguous",
    ),
    ("init observed both refusals", r"\[init\] unnamed receiver denied"),
    (
        # The generation's delegation bit. `dango-output` is declared
        # `transferable = false`, and everything else about this loan would
        # succeed -- sealed source, live receiver at the other end of a channel
        # init holds -- so the bit is the only thing refusing it.
        "a loan cannot be minted over an edge the generation did not delegate",
        r"SLIME_GRAPH loan refused task=\d+ slot=\d+ class=undelegated",
    ),
    ("init observed the delegation refusal", r"\[init\] undelegated loan denied"),
    (
        # The loan itself: an exact sealed subrange, bound to the task at the
        # other end of the channel init named. `to=` is the claim that the
        # receiver was resolved through a capability rather than taken from the
        # caller's word.
        "the loan was minted against the receiver the caller named by capability",
        rf"SLIME_GRAPH loan created task=\d+ slot=\d+ id=\d+ to=\d+ offset=0 "
        rf"length={PAYLOAD_BYTES}",
    ),
    ("init observed the loan", r"\[init\] loan created"),
    (
        # Quota class 4 of 4: loans. The ceiling is one and it is spent, so a
        # second loan of the same sealed region is refused by the quota rather
        # than by anything about the range.
        "the loan ceiling refused a second loan",
        r"SLIME_GRAPH loan refused task=\d+ slot=\d+ class=quota",
    ),
    ("init observed the loan refusal", r"\[init\] loan quota refused"),
    (
        # The transfer. This is the mechanism P5.3.1 refused outright and this
        # slice adds, and it is narrow by construction: a loan is the only
        # resource kind the root will move.
        #
        # `side=` rather than `to=` since B25: an end may have co-holders, so
        # the transfer binds to the receiving *side* of the channel and is
        # collected by whichever holder dequeues the message.
        "the loan capability moved to the receiving side",
        r"SLIME_GRAPH capability transfer task=\d+ channel=\d+ side=\w+ caps=1",
    ),
    ("init sent the descriptor", r"\[init\] loan transferred"),
    (
        # A move, not a copy: the sender can no longer name what it sent.
        "the sender can no longer name the transferred loan",
        r"\[init\] transferred loan released by sender",
    ),
    (
        "the receiver received the descriptor and the loan",
        r"\[sample-receiver\] descriptor received",
    ),
    (
        # The receiver's own denial arms, all four from the unmodified binary:
        # a descriptor naming another loan, a map past the loaned range, a
        # writable map of a read-only loan, and a second return.
        "a malformed descriptor mapped nothing",
        r"\[sample-receiver\] malformed descriptor mapped nothing",
    ),
    (
        # An seL4 frame capability records exactly one mapping, and a loan is
        # two holders mapping the same frames — so the receiver's mapping must
        # go through a distinct capability, recorded against the exact mapping
        # it backs. Asserted at the moment it is minted, because the terminal
        # `aliases=0` cannot establish it: a boot that never aliased at all also
        # ends at zero, and its unmaps would tear down the lender's view while
        # the receiver's silently survived.
        "the receiver's mapping went through its own frame capability",
        r"SLIME_GRAPH frame aliased frame=\d+ vaddr=0x\w+ live=\d+",
    ),
    (
        "the receiver mapped exactly the loaned bytes",
        r"SLIME_GRAPH loan mapped task=\d+ slot=\d+ id=\d+",
    ),
    ("the receiver observed the mapping", r"\[sample-receiver\] loaned bytes mapped"),
    (
        "a loan grants no write access",
        r"\[sample-receiver\] loan stays read-only",
    ),
    (
        # The end-to-end claim: the receiver reconstructed a payload larger than
        # the control-message bound from bytes that never entered a queue.
        "the receiver verified the whole payload",
        r"\[sample-receiver\] payload verified",
    ),
    (
        "the loan was returned",
        r"SLIME_GRAPH loan returned task=\d+ slot=\d+ id=\d+",
    ),
    (
        # Single-return. The second return finds no capability, because the
        # first emptied the slot.
        "a returned loan cannot be returned again",
        r"\[sample-receiver\] loan returned once",
    ),
    ("init observed the settlement", r"\[init\] receiver settled"),
    (
        # C7.5 retention, from the other side: the creator could not reclaim
        # while the loan was outstanding, and can now.
        "the creator reclaimed once the loan had settled",
        r"\[init\] released",
    ),
    (
        "an unrelated holder received on its channel",
        r"\[console\] unrelated holder intact",
    ),
    (
        # The unrelated-holder claim, and the only assertion that establishes
        # it. Init has by now exhausted all four of its own ceilings; console
        # then runs a full create/map/write/seal/unmap/release against its own
        # declared quota and succeeds. A ceiling that leaked across holders
        # would report `quota exhausted` here instead.
        "an unrelated holder's own quota was undisturbed by the exhaustion",
        r"\[console\] shared-buffer quota live",
    ),
    (
        # One loan deliberately left between its send and a receive that never
        # happens, so the root's transit reclamation is the only thing that can
        # settle it. Without this the arm is uncovered and looks covered -- a
        # fault injection with `transit.reclaim` removed still passed the gate
        # before this was added.
        "one loan was left in flight for the root to reclaim",
        r"\[init\] loan stranded in flight",
    ),
    ("init completed the scenario", r"\[init\] loan plane complete"),
    (
        "the graph drained with every window and table reclaimed",
        r"SLIME_GRAPH served live=0 unsupported=0 unimplemented=0 ",
    ),
    (
        # The terminal marker, and the reclamation claim. All six zeros are
        # load-bearing: a loan whose lender died unsettled, a mapping a dead
        # receiver still held, a region nothing reclaimed, a capability parked
        # in flight that no task can name, a page the adapter failed to unmap,
        # or a frame alias still holding a mapping the root believes exists
        # would each be a graph that only appeared to drain.
        "every loan, mapping, region, alias, and in-flight capability was reclaimed",
        r"SLIME_GRAPH loans served=\d+ loans=0 mappings=0 regions=0 transit=0 "
        r"orphans=0 aliases=0",
    ),
)

FAILURE_MARKERS: tuple[str, ...] = (
    r"SLIME_ROOT FATAL .*",
    r"SLIME_GRAPH FAIL .*",
    # The scenarios' own assertions. Every one means an operation returned
    # something other than what the plane promises, and the component says so
    # rather than exiting quietly.
    r"\[init\] loan plane fail: .*",
    r"\[sample-receiver\] fail: .*",
    # A holder whose reclamation could not complete leaves live state charged to
    # a dead task. The terminal marker's counts would show it, but this names
    # the cause at the point it happened.
    r"SLIME_GRAPH holder reclaim incomplete .*",
    # A component that could not bind its transfer window would issue no
    # windowed operation at all, and the graph would look quiet rather than
    # broken.
    r"\[slime-rt\] transfer window bind failed",
    r"SLIME_GRAPH window bind refused",
    r"SLIME_GRAPH park refused .*",
    # A channel the generation declared that the root could not place. This
    # graph declares none, so any occurrence means the fixture and the boot
    # layout have drifted apart.
    r"SLIME_GRAPH channel unplaced .*",
    r"SLIME_GRAPH service budget exhausted",
    # seL4's own complaints. `read-only endpoint` in particular means a
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
)


def fail(message: str) -> NoReturn:
    raise SystemExit(f"seL4 loan plane check: {message}")


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


def profile_text(profile: dict[str, object], key: str) -> str:
    value = profile.get(key)
    if not isinstance(value, str) or not value:
        fail(f"sel4/pins.toml [qemu_arm_virt].{key} must be non-empty text")
    return value


def profile_integer(profile: dict[str, object], key: str) -> int:
    value = profile.get(key)
    if not isinstance(value, int) or isinstance(value, bool):
        fail(f"sel4/pins.toml [qemu_arm_virt].{key} must be an integer")
    return value


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    try:
        with path.open("rb") as handle:
            while chunk := handle.read(1024 * 1024):
                digest.update(chunk)
    except OSError as error:
        fail(f"cannot hash {path.relative_to(ROOT)}: {error}")
    return digest.hexdigest()


def build_image() -> None:
    command = [sys.executable, str(BUILD_SCRIPT), "--loan-plane"]
    print(f"[build] {' '.join(command)}", flush=True)
    try:
        process = subprocess.run(command, cwd=ROOT, check=False)
    except OSError as error:
        fail(f"cannot run the seL4 image build: {error}")
    if process.returncode != 0:
        fail(f"seL4 image build failed with exit status {process.returncode}")


def check_manifest() -> None:
    if not MANIFEST.is_file():
        fail(
            f"missing identity manifest {MANIFEST.relative_to(ROOT)}; "
            "run `just sel4_loan_check`"
        )
    try:
        manifest = json.loads(MANIFEST.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        fail(f"cannot parse {MANIFEST.relative_to(ROOT)}: {error}")
    if not isinstance(manifest, dict) or manifest.get("kind") != "slime-sel4-image-identity":
        fail(f"{MANIFEST.relative_to(ROOT)} is not a Slime seL4 identity manifest")
    # The four images are built from the same sources and differ only in which
    # generation the root task embeds, so booting the wrong one would fail on
    # markers rather than on identity. Checking the variant reports the actual
    # cause instead.
    if manifest.get("variant") != IMAGE_VARIANT:
        fail(
            f"{MANIFEST.relative_to(ROOT)} records variant "
            f"{manifest.get('variant')!r}, not {IMAGE_VARIANT!r}; "
            "rebuild with `--loan-plane`"
        )
    image = manifest.get("image")
    if not isinstance(image, dict) or not isinstance(image.get("sha256"), str):
        fail("identity manifest does not record the packaged image digest")
    if not IMAGE.is_file():
        fail(f"missing packaged image {IMAGE.relative_to(ROOT)}")
    actual = sha256_file(IMAGE)
    if actual != image["sha256"]:
        fail(
            f"{IMAGE.relative_to(ROOT)} SHA-256 is {actual}, but the identity manifest "
            f"records {image['sha256']}; rebuild before booting"
        )


def boot(profile: dict[str, object]) -> str:
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
        profile_text(profile, "machine"),
        "-cpu",
        profile_text(profile, "cpu"),
        "-smp",
        str(profile_integer(profile, "cpus")),
        "-m",
        f"size={profile_integer(profile, 'memory_mib')}M",
        "-nographic",
        "-serial",
        "mon:stdio",
        "-kernel",
        str(IMAGE),
    ]
    print(f"[boot] {' '.join(command)}", flush=True)
    terminal = re.compile(REQUIRED_MARKERS[-1][1])
    failures = re.compile("|".join(FAILURE_MARKERS))
    lines: list[str] = []
    try:
        process = subprocess.Popen(
            command,
            cwd=ROOT,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
            bufsize=1,
        )
    except OSError as error:
        fail(f"cannot run QEMU: {error}")
    # A wedged guest emits nothing, so the deadline cannot live in the read
    # loop; a watchdog kills QEMU, which closes the pipe and ends the loop.
    watchdog = threading.Timer(BOOT_TIMEOUT_SECONDS, process.kill)
    watchdog.start()
    try:
        assert process.stdout is not None
        for line in process.stdout:
            lines.append(line.rstrip("\n"))
            if terminal.search(line) or failures.search(line):
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
        report_transcript(transcript)
        fail(f"boot exceeded {BOOT_TIMEOUT_SECONDS}s without reaching the final marker")
    return transcript


def report_transcript(transcript: str) -> None:
    tail = transcript.splitlines()[-40:]
    if tail:
        sys.stdout.write("--- serial transcript (tail) ---\n")
        sys.stdout.write("\n".join(tail) + "\n")
        sys.stdout.write("--- end transcript ---\n")
        sys.stdout.flush()


def check_transcript(transcript: str) -> None:
    for pattern in FAILURE_MARKERS:
        match = re.search(pattern, transcript)
        if match is not None:
            report_transcript(transcript)
            fail(f"failure marker in serial transcript: {match.group(0)!r}")
    position = 0
    for description, pattern in REQUIRED_MARKERS:
        match = re.compile(pattern).search(transcript, position)
        if match is None:
            report_transcript(transcript)
            if re.search(pattern, transcript) is not None:
                fail(f"marker out of order: {description} ({pattern})")
            fail(f"missing marker: {description} ({pattern})")
        position = match.end()
    check_declared_quotas(transcript)
    check_quota_classes(transcript)
    check_payload_crosses_the_message_bound()




FIXTURE = ROOT / "contracts" / "generation" / "v1" / "fixtures" / "sel4-loan.zti"

# The one ceiling this file needs by value rather than by comparison: the four
# quota probes are written against `init`'s limits, so a fixture edit that
# changed them would need matching probe edits. Checked against the fixture
# below rather than trusted.
INIT_QUOTA = {"pages": 4, "buffers": 2, "mappings": 2, "loans": 1}


def fixture_quotas() -> dict[str, tuple[int, int, int, int]]:
    """The ceilings `sel4-loan.zti` declares, read from the fixture itself.

    Parsed rather than hand-copied into this file. The claim under test is that
    the root's ceilings came from the *generation*; comparing its output to a
    constant transcribed here would only establish that two files in this
    repository agree with a third thing typed twice, and a fixture edit would
    silently invalidate the gate rather than fail it.

    The manifest is canonical Zutai, but the budget entries are a flat list of
    scalar fields, so a line-oriented read is sufficient and avoids depending on
    the Zutai binary from a gate that is not otherwise about the format.
    """
    try:
        text = FIXTURE.read_text(encoding="utf-8")
    except OSError as error:
        fail(f"cannot read {FIXTURE.relative_to(ROOT)}: {error}")
    entries = re.findall(
        r"\{\s*holder\s*=\s*\"([^\"]+)\"\s*;\s*"
        r"bytePages\s*=\s*(\d+)\s*;\s*"
        r"bufferCount\s*=\s*(\d+)\s*;\s*"
        r"mappingCount\s*=\s*(\d+)\s*;\s*"
        r"loanCount\s*=\s*(\d+)\s*;",
        text,
    )
    if not entries:
        fail(f"{FIXTURE.relative_to(ROOT)} declares no shared-buffer budget entries")
    return {name: tuple(int(value) for value in limits) for name, *limits in entries}


def check_declared_quotas(transcript: str) -> None:
    """Every holder received exactly the ceiling the generation declared.

    Order-independent on purpose: the root reports these in task-id order, and
    task ids follow staging order, which the milestone claims nothing about.
    What it does claim is that each holder's ceiling came from the generation,
    so each is checked by name against the fixture as parsed.
    """
    declared = fixture_quotas()
    reported = {
        name: tuple(int(value) for value in limits)
        for name, *limits in re.findall(
            r"SLIME_GRAPH quota task=\d+ component=(\S+) pages=(\d+) buffers=(\d+) "
            r"mappings=(\d+) loans=(\d+)",
            transcript,
        )
    }
    if reported != declared:
        fail(
            f"the root declared {reported}, not the ceilings "
            f"{FIXTURE.relative_to(ROOT)} states ({declared})"
        )
    # The probes are written against init's exact numbers, so a fixture edit
    # that moved them would make every ceiling+1 probe test the wrong thing
    # while still refusing. Fail here rather than pass on a coincidence.
    if declared.get("init") != tuple(INIT_QUOTA[key] for key in ("pages", "buffers", "mappings", "loans")):
        fail(
            f"{FIXTURE.relative_to(ROOT)} declares init {declared.get('init')}, but the "
            f"quota probes in init.rs are written against {INIT_QUOTA}"
        )
    print(
        f"quota: every holder received exactly the ceiling "
        f"{FIXTURE.relative_to(ROOT)} declares for it",
        flush=True,
    )


def check_quota_classes(transcript: str) -> None:
    """Each of the four ceilings refused, and each named its own class.

    The ordered markers assert that four refusals happened in the right places.
    This asserts something the ordering cannot: that every refusal came from a
    *quota* rather than from some other check that happens to refuse the same
    request. A region refused for a malformed range would satisfy the ordered
    marker for the page ceiling equally well and would prove nothing about the
    generation's declared limits.
    """
    # `loan refused` carries no operation word, while `buffer create refused`
    # and `loan mapped refused` do, so the word is optional. A pattern that
    # required it would silently miss both loan-mint refusals -- the unsealed
    # one and the loan ceiling -- and report three classes where there are four.
    refusals = re.findall(
        r"SLIME_GRAPH (?:buffer|loan)(?: \w+)? refused task=\d+ [^\n]*class=(\w+)",
        transcript,
    )
    quota_refusals = [name for name in refusals if name == "quota"]
    if len(quota_refusals) != len(INIT_QUOTA):
        fail(
            f"the transcript records {len(quota_refusals)} quota refusals, not the "
            f"{len(INIT_QUOTA)} classes the milestone requires "
            f"({', '.join(sorted(INIT_QUOTA))}); refusal classes seen: "
            f"{', '.join(refusals) or 'none'}"
        )
    print(
        f"quota: all {len(INIT_QUOTA)} declared ceilings refused at ceiling+1 "
        f"({', '.join(sorted(INIT_QUOTA))})",
        flush=True,
    )


def check_payload_crosses_the_message_bound() -> None:
    """The loaned payload must exceed what a control message can carry.

    Asserted against the source rather than inferred from the transcript,
    because a payload that shrank below the message bound would still be loaned,
    still be mapped, and still verify -- the gate would pass while the property
    that makes a loan necessary at all went unexercised.
    """
    if PAYLOAD_BYTES <= MAX_MSG_BYTES:
        fail(
            f"the loaned payload is {PAYLOAD_BYTES} bytes, which fits in the "
            f"{MAX_MSG_BYTES}-byte control message and needs no loan"
        )
    print(
        f"payload: {PAYLOAD_BYTES} bytes exceeds the {MAX_MSG_BYTES}-byte control "
        "message bound, so it can only cross as a loan",
        flush=True,
    )


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Boot the seL4 loan-plane image and assert ordered markers"
    )
    parser.add_argument(
        "--no-build",
        action="store_true",
        help="boot the already-built image instead of rebuilding it first",
    )
    arguments = parser.parse_args()

    if Path.cwd().resolve() != ROOT:
        fail(f"run from repository root: {ROOT}")
    pins = load_pins()
    if not arguments.no_build:
        build_image()
    check_manifest()
    profile = pins["qemu_arm_virt"]
    assert isinstance(profile, dict)
    check_transcript(boot(profile))
    print(
        "seL4 loan plane check: a sealed subrange was loaned to a receiver named by "
        "capability, mapped read-only, returned once, and reclaimed; all four declared "
        "quota classes refused at ceiling+1 without disturbing an unrelated holder"
    )


if __name__ == "__main__":
    main()
