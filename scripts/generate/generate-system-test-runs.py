#!/usr/bin/env python3

"""Generate `contracts/system-test-run/v1` records from each plane gate's execution inputs.

CP11 declared that QEMU arguments, disk fixtures, injected device behavior,
timeouts, and marker contracts belong to a versioned test-run contract rather
than to the image closure, so a marker oracle or a timeout cannot alter the
image it exercises. CP14's fifth deliverable is moving the real values there.

The records are *frozen declarations*, extracted once and then held fixed —
the same convention `sel4_boot_layout_check` uses for its per-plane layout
fixtures, with `--check` refusing drift and `--bless` re-freezing after an
intended change. That direction matters: after the first freeze the record is
the authority and a checker that quietly grows a disk or doubles a timeout is a
refusal, not a silent update.

What this does *not* do is make the checkers read these records. That is CP15's
migration, and claiming it here would be false: the values below are still
executed from each checker's own constants. What the freeze buys today is that
an execution input cannot change without a reviewed record change.
"""

from __future__ import annotations
import sys as _sys
from pathlib import Path as _Path

_sys.path.insert(0, str(_Path(__file__).resolve().parents[1] / "lib"))

import argparse
import ast
import re

import work_items

from harness import ROOT
from system_image_closure import compile_closure

CHECK_ROOT = ROOT / "scripts" / "check"
RUN_ROOT = ROOT / "contracts" / "system-test-run" / "v1" / "runs"
CLOSURE_ROOT = ROOT / "contracts" / "system-image-closure" / "v2" / "closures"

# The marker contract each plane's expectations are stated against. One
# identity for the seL4 serial marker vocabulary: the planes assert different
# chains *within* it, and those chains stay in their owning checker.
MARKER_CONTRACT = "f03ce9b40628dcb82e3ec97154b2f5ec549a41bfbb9cfcf13a632e40113fd42e"

# Multi-platform checker invocations whose non-default arm is not closure
# reachable, so the record names a literal image instead of resolving one.
# `just riscv64_qemu_check` is the recipe that invokes these, and every target
# it names with `--platform qemu-riscv-virt` and can execute belongs here: a
# real invocation without a record leaves its execution profile and image
# provenance unfrozen. The tuple is `(run name, execution profile, image
# name)`, and a checker may name more than one: MEM-64M's reuse arm boots its
# own RV64 image beside the ceiling arm's, and a single-tuple table would leave
# the second one unrecorded.
EXTRA_RUNS: dict[str, tuple[tuple[str, str, str], ...]] = {
    "sel4-generation": (
        (
            "sel4-generation-qemu-riscv-virt",
            "qemu-riscv-virt",
            "slime-sel4-generation-qemu-riscv-virt.elf",
        ),
    ),
    "sel4-private-memory": (
        (
            "sel4-private-memory-qemu-riscv-virt",
            "qemu-riscv-virt",
            "slime-sel4-private-memory-qemu-riscv-virt.elf",
        ),
        (
            "sel4-private-memory-cycles-qemu-riscv-virt",
            "qemu-riscv-virt",
            "slime-sel4-private-memory-cycles-qemu-riscv-virt.elf",
        ),
    ),
    "sel4-rollback": (
        (
            "sel4-rollback-qemu-riscv-virt",
            "qemu-riscv-virt",
            "slime-sel4-rollback-qemu-riscv-virt.elf",
        ),
    ),
}

# Additional invocations that ARE closure-reachable, unlike the arms above.
# Every separately booted image needs its own frozen record, because
# `check-system-test-run.py` can only freeze and validate an image's execution
# inputs through a record naming it. The tuple is `(run name, closure name,
# execution profile)`; the image resolves through the closure, so there is no
# literal path to name.
EXTRA_CLOSURE_RUNS: dict[str, tuple[tuple[str, str, str], ...]] = {
    "sel4-reclamation": (("sel4-large-image", "sel4-large-image", "qemu-arm-virt"),),
    # IO8 and IO9: the product-graph checker's other arms boot the product
    # graph plus the pwm driver (`just sel4_pwm_graph_check`) and plus the
    # serial driver and heartbeat producer (`just sel4_mavlink_graph_check`).
    "sel4-component-graph": (
        ("sel4-pwm", "sel4-pwm", "qemu-arm-virt"),
        ("sel4-mavlink", "sel4-mavlink", "qemu-arm-virt"),
    ),
    "sel4-private-memory": (
        (
            "sel4-private-memory-fail-large-map",
            "sel4-private-memory-fail-large-map",
            "qemu-arm-virt",
        ),
        (
            "sel4-private-memory-fail-second-allocation",
            "sel4-private-memory-fail-second-allocation",
            "qemu-arm-virt",
        ),
        (
            "sel4-private-memory-cycles",
            "sel4-private-memory-cycles",
            "qemu-arm-virt",
        ),
    ),
    # The network checker boots one image per arm. Its default arm is `sel4-io-network`, and the other
    # thirteen are named here, each by the closure it builds.
    "sel4-io-network": tuple(
        (name, name, "qemu-arm-virt")
        for name in (
            "sel4-io-tcp",
            "sel4-io-tcp-impairment",
            "sel4-io-tcp-listener",
            "sel4-io-tcp-options",
            "sel4-io-driver-reset-in-flight",
            "sel4-io-service-fault-in-flight",
            "sel4-io-lifetime",
            "sel4-io-local",
            "sel4-net",
            "sel4-zenoh",
            "sel4-zenoh-external",
            "sel4-http",
            "sel4-http-public",
        )
    ),
}

# A checker that boots several images attaches QEMU devices in the functions that boot each one, and its arms
# share the checker's timeout and failure markers. For such a checker, drives, devices and faults are read
# from the functions an arm runs, found by following calls between the checker's own functions. An arm whose
# code sits inline in `main()` has no entry function, and `main()` must then pass no QEMU arguments at all.
#
# Two arms can run the same function with a different flag, as `run_tcp_arm(driver_reset=...)` does. ARM_FLAGS
# gives the flag values an arm runs with. Extraction drops the branch of an `if flag:` or `if not flag:` that
# the value rules out, in the function that takes the flag. A test on the flag of any other shape is refused.
ARM_FLAGS: dict[str, dict[str, dict[str, bool]]] = {
    "check-sel4-io-network-plane.py": {
        "sel4-io-tcp": {"driver_reset": False},
        "sel4-io-driver-reset-in-flight": {"driver_reset": True},
    },
}

ARM_ENTRY_FUNCTIONS: dict[str, dict[str, tuple[str, ...]]] = {
    "check-sel4-io-network-plane.py": {
        "sel4-io-network": ("run_authority_arm",),
        "sel4-io-tcp": ("run_tcp_arm",),
        "sel4-io-tcp-impairment": ("run_impairment_arm",),
        "sel4-io-tcp-listener": ("run_listener_arm",),
        "sel4-io-tcp-options": ("run_options_arm",),
        "sel4-io-driver-reset-in-flight": ("run_tcp_arm",),
        "sel4-io-service-fault-in-flight": (),
        "sel4-io-lifetime": (),
        "sel4-io-local": (),
        "sel4-net": ("run_net_arm",),
        "sel4-zenoh": ("run_zenoh_arm",),
        "sel4-zenoh-external": ("run_zenoh_external_arm",),
        "sel4-http": ("run_http_arm",),
        "sel4-http-public": ("run_http_public_arm",),
    },
}

# A planning exclusion belongs to one invocation, never to its whole checker.
# Any implementation artifact appearing requires removing the exclusion first.
# Rows are (canonical item UUID, checker filename, closure name, artifact paths).
PLANNED_RUNS: dict[str, tuple[str, str, str, tuple[str, ...]]] = {}

# Checkers that boot no seL4 QEMU plane of their own, so they own no test run:
# board gates, host-only contract gates, aggregate composers that delegate to
# the planes they compose, and the bless/derivation helpers.
NOT_A_PLANE = {
    "check-sel4-boot-layout.py",  # composes every plane; owns no single run
    "check-sel4-fabric-aggregate.py",  # composes the fault and traffic planes
    "check-sel4-capability-layout.py",  # audits the boot plane's root, no run of its own
    "check-sel4-gate-control.py",  # mutates other gates' transcripts
    "check-sel4-plane.py",  # shared runner
    "check-sel4-root-boot.py",  # root-only aggregate over the boot plane
    "check-sel4-gate-controls.py",  # asserts other gates reject bad input; boots nothing
    "check-sel4-pins.py",  # host-side pin assertion; boots nothing
    "check-sel4-x86-64-image.py",  # validates artifact identity; boots nothing
    "check-sel4-trace-plane.py",  # analyses a transcript; boots nothing
}


def fail(message: str) -> None:
    raise SystemExit(f"system test run generation: {message}")


def plane_checkers() -> list[_Path]:
    validate_planned_runs()
    return [
        path
        for path in sorted(CHECK_ROOT.glob("check-sel4-*.py"))
        if path.name not in NOT_A_PLANE
    ]


def run_name(path: _Path) -> str:
    """`check-sel4-io-block-plane.py` -> `sel4-io-block`."""
    stem = path.stem[len("check-") :]
    if stem.endswith("-plane"):
        stem = stem[: -len("-plane")]
    return stem

def declared_variants(path: _Path) -> list[tuple[str, str, str, str]]:
    """Invocation names before planning exclusions or artifact resolution."""
    name = run_name(path)
    variants = [(name, closure_name_for(path, name), "qemu-arm-virt", "")]
    for run, profile, image in EXTRA_RUNS.get(name, ()):
        variants.append((run, "", profile, image))
    for run, closure, profile in EXTRA_CLOSURE_RUNS.get(name, ()):
        variants.append((run, closure, profile, ""))
    return variants


def validate_planned_runs() -> None:
    """Refuse stale or ambiguous exclusions, including rows no checker visits."""
    for run, row in PLANNED_RUNS.items():
        if not isinstance(row, tuple) or len(row) != 4:
            fail(f"{run}: malformed planning registration")
        identity, checker, closure, artifacts = row
        if not isinstance(identity, str) or not work_items.UUID.fullmatch(identity):
            fail(f"{run}: planning identity is not a canonical UUID")
        # The shared reader is deliberately shallow. An exclusion additionally
        # needs identity and state in the item's envelope, never its body.
        try:
            item_text = (work_items.ITEMS / f"{identity}.md").read_text(encoding="utf-8")
        except (OSError, UnicodeError) as error:
            fail(f"{run}: cannot read planning item envelope: {error}")
        opening, separator, remainder = item_text.partition("---\n")
        frontmatter, closing, _body = remainder.partition("\n---\n")
        if opening or not separator or not closing:
            fail(f"{run}: planning item lacks a delimited frontmatter envelope")
        fields = {
            key: re.findall(rf"^{key}:[ \t]*(.*)$", frontmatter, re.MULTILINE)
            for key in ("schema", "id", "state")
        }
        if (any(len(values) != 1 for values in fields.values())
                or fields["schema"][0] not in {"work-item/v1", "work-item/v2"}
                or fields["id"][0] != identity
                or fields["state"][0] not in {"open", "active", "blocked", "deferred"}):
            fail(f"{run}: planning item envelope has invalid identity, schema or state")
        matches = [item for item in work_items.items() if item["id"] == identity]
        if (identity not in work_items.identities() or len(matches) != 1
                or matches[0]["state"] not in {"open", "active", "blocked", "deferred"}
                or matches[0]["retired"]):
            fail(f"{run}: planning identity must name one nonterminal work item")
        if (not isinstance(checker, str) or _Path(checker).name != checker
                or checker in NOT_A_PLANE
                or not checker.startswith("check-sel4-") or not checker.endswith(".py")
                or not (CHECK_ROOT / checker).is_file()):
            fail(f"{run}: planning checker is absent or conflicts with inventory")
        variants = declared_variants(CHECK_ROOT / checker)
        if sum(name == run and expected == closure for name, expected, _, _ in variants) != 1:
            fail(f"{run}: planning registration names no matching invocation/closure")
        if not isinstance(artifacts, tuple) or not artifacts:
            fail(f"{run}: planning registration requires implementation artifacts")
        for artifact in artifacts:
            if (not isinstance(artifact, str) or not artifact
                    or _Path(artifact).is_absolute() or ".." in _Path(artifact).parts
                    or not (ROOT / artifact).resolve().is_relative_to(ROOT.resolve())):
                fail(f"{run}: artifact must be a nonescaping root-relative path")
            if (ROOT / artifact).exists() or (ROOT / artifact).is_symlink():
                fail(f"{run}: stale planning registration: artifact {artifact} is present")
        mandatory = {
            str((CLOSURE_ROOT / f"{closure}.zti").relative_to(ROOT)),
            str((RUN_ROOT / f"{run}.zti").relative_to(ROOT)),
        }
        if not mandatory <= set(artifacts):
            fail(f"{run}: planning artifacts must include closure and run declaration")


def run_variants(path: _Path) -> list[tuple[str, str, str, str]]:
    """Every active invocation; absent closures need an independent exemption."""
    validate_planned_runs()
    variants = []
    for name, closure, profile, image in declared_variants(path):
        if name in PLANNED_RUNS:
            if PLANNED_RUNS[name][1] != path.name:
                fail(f"{name}: planning registration conflicts with another checker")
            continue
        if closure and not (CLOSURE_ROOT / f"{closure}.zti").is_file():
            # Only legacy default arms may resolve to a literal image. The
            # aggregate owns the exemption, not a missing closure or item state.
            if name != run_name(path):
                fail(f"{name}: missing active closure {closure}")
            image = booted_image(path)
            aggregate = CHECK_ROOT / "check-system-image-aggregate.py"
            text = aggregate.read_text(encoding="utf-8") if aggregate.is_file() else ""
            block = re.search(r"IMAGES_WITHOUT_CLOSURE = \{(.*?)\n\}", text, re.DOTALL)
            exempt = set(re.findall(r'"(slime-[a-z0-9.-]+\.elf)":', block.group(1))) if block else set()
            if image not in exempt:
                fail(f"{name}: missing active closure {closure} without a literal-image exemption")
        variants.append((name, closure, profile, image))
    return variants


def booted_image(path: _Path) -> str:
    """The `build/slime-*.elf` this checker boots.

    Derived from the checker rather than from the run name, because the two
    differ: `check-sel4-component-graph.py` boots `slime-sel4-graph.elf`. Using
    the name would attribute a plane to the wrong image, which is exactly the
    mapping error the aggregate gate exists to catch.
    """
    text = path.read_text(encoding="utf-8")
    found = sorted(
        set(re.findall(r'"build"\s*/\s*"(slime-[a-z0-9.-]+\.elf)"', text))
        | set(re.findall(r"build/(slime-[a-z0-9.-]+\.elf)", text))
    )
    if not found:
        fail(f"{path.name}: boots no build/slime-*.elf image")
    # A checker touching several images (a plane plus a control arm) is keyed by
    # the one its own name implies, falling back to the single image it names.
    stem = run_name(path)
    preferred = f"slime-{stem}.elf"
    if preferred in found:
        return preferred
    return found[0]


def closure_name_for(path: _Path, name: str) -> str:
    """The closure this checker's own source declares, or its run name.

    Most checkers' closure matches their run name (`sel4-channel` builds the
    `sel4-channel` closure). A checker whose plane image predates the closure
    naming convention — `check-sel4-component-graph.py` and
    `check-sel4-device-plane.py` both build the `sel4` closure — declares a
    `CLOSURE = "..."` constant that names it explicitly; reading that instead
    of guessing from the run name is what keeps this generic rather than a
    second hand-maintained mapping beside the aggregate gate's.
    """
    match = re.search(r'^CLOSURE = "([a-z0-9-]+)"', path.read_text(encoding="utf-8"), re.MULTILINE)
    return match.group(1) if match is not None else name


def closure_identity_for(name: str) -> str:
    """The closure this run exercises, or the empty string when none exists.

    An empty identity is a truthful statement that the plane's image is not yet
    closure-reachable — the eight images CP15 still owns — rather than a
    fabricated digest. `check-system-test-run.py` requires a non-empty identity
    to resolve and an empty one to name an image the aggregate gate agrees is
    exempt.
    """
    candidate = CLOSURE_ROOT / f"{name}.zti"
    if not candidate.is_file():
        return ""
    return compile_closure(candidate).identity.hex()


QEMU_ARGUMENT = re.compile(r'"-device"|"-drive"|"-netdev"|additional_arguments')
DEVICE_TOKEN = re.compile(r'"-device"|"-drive"|"-netdev"|additional_arguments|virtio-[a-z]+-device|usb-kbd')


def _flag_test(test: ast.expr, flag: str) -> bool | None:
    """Whether `test` is `flag` (True), `not flag` (False), or neither (None)."""
    if isinstance(test, ast.Name) and test.id == flag:
        return True
    if isinstance(test, ast.UnaryOp) and isinstance(test.op, ast.Not) and isinstance(test.operand, ast.Name) and test.operand.id == flag:
        return False
    return None


def _mentions(node: ast.AST, flag: str) -> bool:
    return any(isinstance(inner, ast.Name) and inner.id == flag for inner in ast.walk(node))


def _statements_span(statements: list[ast.stmt]) -> range:
    return range(statements[0].lineno, statements[-1].end_lineno + 1) if statements else range(0)


def check_flag_uses(function: ast.FunctionDef, flag: str, local_functions: set[str], where: str) -> None:
    """Refuse every use of `flag` in `function` except the three that extraction can scope to an arm.

    The flag may be the whole test of an `if` or a conditional expression, written bare or with `not`. It may
    also be an argument of a call into another module, such as `link_peer.Peer(hold_first_tcp=flag)`. Any
    other use is refused: an assignment or alias, a loop, an assert, a comprehension filter, a boolean
    operand, and an argument of a function of the checker itself. The rule is on the flag's own name. It does
    not follow a value that another module derives from the flag.
    """
    parents = {child: parent for parent in ast.walk(function) for child in ast.iter_child_nodes(parent)}
    for name in ast.walk(function):
        if not (isinstance(name, ast.Name) and name.id == flag):
            continue
        if not isinstance(name.ctx, ast.Load):
            fail(f"{where}: {flag} is assigned at line {name.lineno}, so the arm cannot be scoped")
        node, parent = name, parents.get(name)
        if isinstance(parent, ast.UnaryOp) and isinstance(parent.op, ast.Not):
            node, parent = parent, parents.get(parent)
        if isinstance(parent, (ast.If, ast.IfExp)) and parent.test is node:
            continue
        if node is name:
            call = parents.get(parent) if isinstance(parent, ast.keyword) else parent
            if isinstance(call, ast.Call) and isinstance(call.func, ast.Attribute):
                continue
            if isinstance(call, ast.Call) and isinstance(call.func, ast.Name) and call.func.id in local_functions:
                fail(f"{where}: {flag} is passed to {call.func.id} at line {name.lineno}, so the arm cannot be scoped")
        fail(f"{where}: {flag} is used at line {name.lineno} in a way extraction cannot scope to an arm")


def pruned_lines(function: ast.FunctionDef, text: str, flags: dict[str, bool], where: str, local_functions: set[str]) -> set[int]:
    """The source lines of `function` that an arm run with `flags` never executes.

    Only a branch on a parameter of the function itself, written as `if flag:` or `if not flag:`, is pruned.
    `check_flag_uses` refuses every other use of the flag, and a conditional expression on it that holds a
    device or QEMU argument is refused here, so extraction leaves nothing unrecorded.
    """
    parameters = {argument.arg for argument in (*function.args.args, *function.args.kwonlyargs)}
    skipped: set[int] = set()
    for flag, value in flags.items():
        if flag not in parameters:
            continue
        check_flag_uses(function, flag, local_functions, where)
        for node in ast.walk(function):
            if isinstance(node, ast.If) and _mentions(node.test, flag):
                selected = _flag_test(node.test, flag)
                if selected is None:
                    fail(f"{where}: the test on {flag} at line {node.lineno} is not `{flag}` or `not {flag}`, so the arm cannot be scoped")
                skipped.update(_statements_span(node.orelse if selected == value else node.body))
            elif isinstance(node, ast.IfExp) and _mentions(node.test, flag):
                held = DEVICE_TOKEN.search(ast.get_source_segment(text, node.body) or "") or DEVICE_TOKEN.search(
                    ast.get_source_segment(text, node.orelse) or ""
                )
                if held:
                    fail(f"{where}: a conditional expression on {flag} at line {node.lineno} holds a QEMU argument, so the arm cannot be scoped")
    return skipped


def device_tokens(node: ast.AST) -> set[str]:
    """The QEMU device, drive and network tokens written anywhere under `node`.

    A token is a string equal to `-device`, `-drive`, `-netdev` or `usb-kbd`, a string that starts with
    `virtio-<kind>-device`, or the name `additional_arguments` as a variable or a keyword.
    """
    found: set[str] = set()
    for inner in ast.walk(node):
        if isinstance(inner, ast.Constant) and isinstance(inner.value, str):
            value = inner.value
            if value in ("-device", "-drive", "-netdev", "usb-kbd") or re.match(r"virtio-[a-z]+-device", value):
                found.add(value[:40])
        elif isinstance(inner, ast.Name) and inner.id == "additional_arguments":
            found.add("additional_arguments")
        elif isinstance(inner, ast.keyword) and inner.arg == "additional_arguments":
            found.add("additional_arguments")
    return found


def call_closure(functions: dict[str, ast.FunctionDef], entries: tuple[str, ...]) -> set[str]:
    """`entries` and every function of the checker that they call, directly or through another."""
    seen: set[str] = set()
    pending = list(entries)
    while pending:
        name = pending.pop()
        if name in seen or name not in functions:
            continue
        seen.add(name)
        for node in ast.walk(functions[name]):
            if isinstance(node, ast.Call) and isinstance(node.func, ast.Name) and node.func.id in functions:
                pending.append(node.func.id)
    return seen


def arm_source(path: _Path, run: str, text: str) -> str:
    """The source an arm's drives, devices and faults are read from.

    A checker with no entry table is read whole. For a checker with one, the source is the functions the arm
    runs and the checker's own functions they call, less the branches that a flag in `ARM_FLAGS` rules out. It
    also holds each plain module-level assignment that those functions read, which is how a fault word in a
    module constant reaches an arm.

    A device, drive or network token is not followed through names. It must sit in a function that some arm
    runs, and a token in any module-level statement or in a function that no arm runs is refused. No form of
    binding can hide a token from that rule. An arm declared with no entry function must find no QEMU argument
    anywhere in `main()`, so a device added to an inline arm cannot go unrecorded.
    """
    arms = ARM_ENTRY_FUNCTIONS.get(path.name)
    if arms is None:
        return text
    if run not in arms:
        fail(f"{path.name}: {run} has no entry in ARM_ENTRY_FUNCTIONS")
    body = ast.parse(text).body
    functions = {node.name: node for node in body if isinstance(node, ast.FunctionDef)}
    constants: dict[str, ast.stmt] = {}
    for node in body:
        if isinstance(node, ast.Assign):
            constants.update({target.id: node for target in node.targets if isinstance(target, ast.Name)})
        elif isinstance(node, ast.AnnAssign) and isinstance(node.target, ast.Name):
            constants[node.target.id] = node
    missing = [name for name in arms[run] if name not in functions]
    if missing:
        fail(f"{path.name}: {run} names entry function(s) {missing} that the checker does not define")
    # A device token may sit only in a function that some arm runs. A binding at module level, whatever its
    # form, and a function that no arm runs cannot be tied to an arm, so a token there is refused. This does
    # not follow names, and no form of assignment can hide a token from it.
    owned: set[str] = set()
    for entries in arms.values():
        owned |= call_closure(functions, entries)
    for node in body:
        holder = node.name if isinstance(node, ast.FunctionDef) and node.name not in owned else None
        if isinstance(node, ast.FunctionDef) and holder is None:
            continue
        tokens = device_tokens(node)
        if tokens:
            where = f"function {holder}, which no arm runs," if holder else f"the module-level statement at line {node.lineno}"
            fail(f"{path.name}: {where} holds QEMU device token(s) {sorted(tokens)}, so no record can say which arm attaches them")
    flags = ARM_FLAGS.get(path.name, {}).get(run, {})
    if not arms[run]:
        if flags:
            fail(f"{path.name}: {run} has no entry function, so it cannot have flags")
        main = functions.get("main")
        if main is None or QEMU_ARGUMENT.search(ast.get_source_segment(text, main) or ""):
            fail(f"{path.name}: {run} is declared to attach nothing, but main() passes QEMU arguments or is absent")
        return ""
    lines = text.splitlines()
    seen: set[str] = set()
    skipped: dict[str, set[int]] = {}
    pending = list(arms[run])
    while pending:
        name = pending.pop()
        if name in seen:
            continue
        seen.add(name)
        skipped[name] = pruned_lines(functions[name], text, flags, f"{path.name}: {run}", set(functions))
        for node in ast.walk(functions[name]):
            if (
                isinstance(node, ast.Call)
                and isinstance(node.func, ast.Name)
                and node.func.id in functions
                and node.lineno not in skipped[name]
            ):
                pending.append(node.func.id)
    unused = sorted(flag for flag in flags if not any(flag in {a.arg for a in (*functions[n].args.args, *functions[n].args.kwonlyargs)} for n in seen))
    if unused:
        fail(f"{path.name}: {run} sets flag(s) {unused} that no function it runs takes as a parameter")
    # A module-level name that a function the arm runs reads belongs to the arm, and so does any module-level
    # name that name's value reads. A reference in a branch the arm's flags rule out does not count. A local
    # name that shadows a module-level one is over-included, which can only add a fact and never hide one.
    referenced: set[str] = set()
    queue = [(node, skipped[name]) for name in seen for node in ast.walk(functions[name])]
    while queue:
        node, dropped = queue.pop()
        if isinstance(node, ast.Name) and isinstance(node.ctx, ast.Load) and node.id in constants and node.lineno not in dropped:
            if node.id not in referenced:
                referenced.add(node.id)
                queue.extend((inner, set()) for inner in ast.walk(constants[node.id]))
    pieces = [
        "\n".join(
            line for number, line in enumerate(lines[node.lineno - 1 : node.end_lineno], node.lineno) if number not in skipped[name]
        )
        for name in sorted(seen)
        for node in (functions[name],)
    ]
    pieces += [
        "\n".join(lines[constants[name].lineno - 1 : constants[name].end_lineno]) for name in sorted(referenced)
    ]
    return "\n".join(pieces)


def extract(path: _Path, run: str | None = None) -> dict:
    """Read one invocation's execution-only inputs from its checker constants."""
    run = run_name(path) if run is None else run
    if run not in {name for name, _, _, _ in declared_variants(path)}:
        fail(f"{path.name}: unsupported extraction invocation {run}")
    text = path.read_text(encoding="utf-8")
    if path.name == "check-sel4-reclamation-plane.py":
        # Both arms share boot transport, but not timeout or transcript policy.
        # Host fixture/refusal controls are not QEMU execution declarations.
        selected = ({"TIMEOUT", "boot", "run_unwind_arm"}
                    if run == "sel4-reclamation" else
                    {"LARGE_TIMEOUT", "LARGE_FAILURES", "boot", "judge_large_image"})
        segments = {}
        for node in ast.parse(text).body:
            if isinstance(node, ast.FunctionDef):
                names = {node.name}
            else:
                targets = ([node.target] if isinstance(node, ast.AnnAssign)
                           else node.targets if isinstance(node, ast.Assign) else [])
                names = {target.id for target in targets if isinstance(target, ast.Name)}
            for name in names & selected:
                if name in segments:
                    fail(f"{path.name}: duplicate execution declaration {name}")
                segments[name] = ast.get_source_segment(text, node)
        if set(segments) != selected:
            fail(f"{path.name}: missing execution declarations for {run}")
        text = "\n".join(segments.values())
        if run == "sel4-large-image":
            text = text.replace("LARGE_TIMEOUT =", "TIMEOUT =")
    if path.name == "check-sel4-private-memory-plane.py":
        # The frozen variants above are fixed-capacity arms. This transport
        # belongs only to the separately selected adaptive/matrix arms.
        lines = text.splitlines(keepends=True)
        declarations = []
        for node in ast.parse(text).body:
            targets = ([node.target] if isinstance(node, ast.AnnAssign)
                       else node.targets if isinstance(node, ast.Assign) else [])
            if any(isinstance(target, ast.Name) and target.id == "ADAPTIVE_DEVICE_ARGUMENTS"
                   for target in targets):
                declarations.append(node)
        if len(declarations) != 1:
            fail(f"{path.name}: expected one top-level adaptive-only transport declaration")
        node = declarations[0]
        lines[node.lineno - 1:node.end_lineno] = ["\n"] * (
            node.end_lineno - node.lineno + 1
        )
        text = "".join(lines)

    timeouts = re.findall(
        r"^(?:BOOT_TIMEOUT_SECONDS|TIMEOUT|BOOT_TIMEOUT|SESSION_TIMEOUT_SECONDS)\s*=\s*(\d+)",
        text,
        re.MULTILINE,
    )
    # Some planes state their bound as an argument default rather than a module
    # constant. That is the same declaration — the value a plain `just` run
    # executes with — so it counts.
    timeouts += re.findall(r'"--timeout",\s*type=int,\s*default=(\d+)', text)
    if not timeouts:
        fail(f"{path.name}: declares no boot timeout, so its run would be unbounded")
    # The largest declared bound: a plane with a session or recovery timeout
    # beyond its boot timeout may legitimately run that long.
    timeout = max(int(value) for value in timeouts)

    # One `-drive` argument is one attached disk. Counting distinct occurrences
    # rather than parsing QEMU argv keeps this honest about what the checker
    # actually attaches; a checker that attaches two disks in two code paths
    # declares two.
    scoped = arm_source(path, run, text)
    drive_sites = len(re.findall(r'"-drive"', scoped))

    devices = sorted(
        set(re.findall(r'"(virtio-[a-z]+-device)[^"]*"', scoped))
        | set(re.findall(r'"(usb-kbd)"', scoped))
    )

    # A forbidden outcome is written either as a bare literal or as a regex with
    # a trailing matcher (`r"SLIME_ROOT FATAL .*"`). Matching only the literal
    # form silently reported "forbids nothing" for every plane using the regex
    # form, which is the more common one.
    forbidden = sorted(
        {
            marker
            for marker in re.findall(
                r'r?"(SLIME_ROOT FATAL|SLIME_GRAPH FAIL|SLIME_ROOT PANIC)(?: \.\*)?"', text
            )
        }
    )

    # Runtime fault injection, by the specific mechanism each kind uses. These
    # are deliberately narrow: a generic `inject` would match ordinary helper
    # names and declare a fault the plane does not exercise, and an over-broad
    # declaration is worse than none because it is what a reader trusts.
    faults = []
    for kind, pattern in (
        # The checker rewrites persisted bytes before or between boots.
        ("corruption", r"\bcorrupt(?:ed|ion|_[a-z]+)?\b|CORRUPT"),
        # A participant is compiled or driven to die mid-exchange.
        ("peer-death", r"EARLY_EXIT|peer[_ -]death|PEER_DEATH"),
        # A device is made to fail or to report a fault, rather than merely
        # being attached.
        ("device", r"device[_ -]fault|DEVICE_FAULT|fault[_ -]inject"),
    ):
        if re.search(pattern, scoped):
            faults.append(kind)

    return {
        "timeoutSeconds": timeout,
        "drives": drive_sites,
        "devices": devices,
        "forbiddenOutcomes": forbidden,
        "faults": faults,
    }


def render(name: str, closure: str, profile: str, facts: dict) -> str:
    """Render one target-qualified record in canonical field order."""

    def text_list(values: list[str], indent: str = "    ") -> str:
        if not values:
            return "[]"
        rows = "".join(f'{indent}  "{value}";\n' for value in values)
        return "[\n" + rows + indent + "]"

    disks = ""
    if facts["drives"]:
        rows = []
        for index in range(facts["drives"]):
            rows.append(
                "    {\n"
                f'      name = "disk-{index}";\n'
                f'      path = "";\n'
                f'      identity = "";\n'
                "      writable = true;\n"
                "    };"
            )
        disks = "[\n" + "\n".join(rows) + "\n  ]"
    else:
        disks = "[]"

    devices = ""
    if facts["devices"]:
        rows = []
        for device in facts["devices"]:
            rows.append(
                "    {\n"
                f'      name = "{device}";\n'
                f'      path = "";\n'
                f'      identity = "";\n'
                "      writable = false;\n"
                "    };"
            )
        devices = "[\n" + "\n".join(rows) + "\n  ]"
    else:
        devices = "[]"

    faults = ""
    if facts["faults"]:
        rows = []
        for kind in facts["faults"]:
            rows.append(
                "    {\n"
                f'      kind = "{kind}";\n'
                f'      target = "";\n'
                f'      value = "";\n'
                "    };"
            )
        faults = "[\n" + "\n".join(rows) + "\n  ]"
    else:
        faults = "[]"

    return (
        "{\n"
        "  formatVersion = 1;\n"
        f'  name = "{name}";\n'
        f'  imageClosureIdentity = "{closure}";\n'
        '  executionKind = "emulator";\n'
        f'  executionProfile = "{profile}";\n'
        f"  disks = {disks};\n"
        "  networks = [];\n"
        f"  devices = {devices};\n"
        f"  faultControls = {faults};\n"
        f'  timeoutSeconds = {facts["timeoutSeconds"]};\n'
        f'  markerContractIdentity = "{MARKER_CONTRACT}";\n'
        f'  forbiddenOutcomes = {text_list(facts["forbiddenOutcomes"], "  ")};\n'
        "}\n"
    )


def outputs() -> dict[_Path, str]:
    emitted: dict[_Path, str] = {}
    for path in plane_checkers():
        for name, closure_name, profile, _image in run_variants(path):
            emitted[RUN_ROOT / f"{name}.zti"] = render(
                name,
                closure_identity_for(closure_name) if closure_name else "",
                profile,
                extract(path, name),
            )
    return emitted


def refresh_identities(emitted: dict[_Path, str]) -> int:
    """Refresh only image references, refusing all execution drift before writing."""
    field = re.compile(r'^  imageClosureIdentity = "[0-9a-f]*";$', re.MULTILINE)
    updates = {}
    for path, expected in emitted.items():
        if not path.is_file():
            fail(f"{path.name}: missing declaration; identity refresh cannot create it")
        current = path.read_text(encoding="utf-8")
        before, after = field.findall(current), field.findall(expected)
        if len(before) != 1 or len(after) != 1:
            fail(f"{path.name}: expected one canonical imageClosureIdentity field")
        if field.sub('', current) != field.sub('', expected):
            fail(f"{path.name}: execution declaration drift; review before re-freezing")
        updates[path] = field.sub(after[0], current)
    for path, contents in updates.items():
        if path.read_text(encoding="utf-8") != contents:
            path.write_text(contents, encoding="utf-8")
    return len(updates)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument(
        "--check", action="store_true", help="refuse any record that drifted from its checker"
    )
    mode.add_argument(
        "--bless", action="store_true", help="re-freeze records after an intended change"
    )
    mode.add_argument(
        "--refresh-identities", action="store_true",
        help="update only closure references; refuse any execution declaration drift",
    )
    arguments = parser.parse_args()

    RUN_ROOT.mkdir(parents=True, exist_ok=True)
    emitted = outputs()

    if arguments.check:
        stale = [
            path.name
            for path, content in emitted.items()
            if not path.is_file() or path.read_text(encoding="utf-8") != content
        ]
        if stale:
            fail(
                f"record(s) disagreeing with their checker: {sorted(stale)}; "
                "re-freeze with `just system_test_run_bless` if the change is intended"
            )
        print(f"{len(emitted)} system test-run records are current")
        return 0

    if arguments.refresh_identities:
        count = refresh_identities(emitted)
        print(f"{count} test-run closure references checked; execution declarations unchanged")
        return 0

    for path, content in sorted(emitted.items()):
        path.write_text(content, encoding="utf-8")
        print(f"Blessed {path.relative_to(ROOT)}")
    print(f"{len(emitted)} system test-run records frozen")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
