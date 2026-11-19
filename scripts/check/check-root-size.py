#!/usr/bin/env python3

"""Budget the source of `slime-root` by role.

Every Rust file under `slime-root/` has exactly one role. The script counts the
code lines of each role and refuses a role, or the series total, that exceeds
its ceiling. It also refuses a file with no role, a file with two roles, and a
dependency of `slime-root` that `DEPENDENCIES` does not name.

A code line is a line that holds at least one character outside whitespace and
comments. The lexer below reads Rust's comments, string literals, raw strings
and character literals, so a brace or `//` inside a literal is text.

A top-level item behind the exact attribute `#[cfg(test)]` is a test item. Its
lines are test lines, from the attribute to the `;` or the closing `}` that
ends the item. A file named `tests.rs` or `*_tests.rs`, and a file under a
`tests/` directory, is a test file. Test lines belong to no role. Any other
`cfg`, and a `#[cfg(test)]` inside an item, leaves the lines as code.

`ROLES` holds the ceilings. This script is the grader set of
`just root_size_check`, so a ceiling or a role assignment changes only through
a planning pull request with an approval (V4). `SERIES` names the roles the
series total covers. The `harness` role holds the build script and the two
`qualification.rs` files. Cargo runs the build script on the host, and a
`slime_*` cfg compiles each qualification scenario into its own root role
image. The role has its own ceiling and is outside the series total.

The script also walks the module tree from each crate root. A walk reads every
`mod name;` declaration of a file and follows it into the file that holds the
module. It reads the file with comments and literals blanked, so text inside
them declares nothing.

The walk from `src/main.rs` finds what only the binary compiles. Host tests
build the library, so they cannot name those lines. The count is reported as
`binaryOnlyCodeLines`, with the count of those files as `binaryOnlyFiles`, and
`BINARY_ONLY_CEILING` bounds the lines.

The walk from `src/lib.rs` finds what the library compiles under a condition.
A file is conditional when a `#[cfg(...)]` attribute stands on its declaration
or on the declaration of a module above it, or when the file holds an inner
`#![cfg(...)]` attribute. A build that does not meet the condition skips the
file, so the host build may not compile it. The count is reported as
`libraryConditionalCodeLines`, with `libraryConditionalFiles`, and
`LIBRARY_CONDITIONAL_CEILING` bounds the lines. Test files are in neither
count.

The script measures every function outside test items, in code lines from the
line of its `fn` to the line of its closing brace. `FUNCTION_LIMIT` bounds a
function. `LONG_FUNCTIONS` names the functions that were longer when the limit
landed, each with a ceiling of its own. A function above the limit that the
table does not name is refused, and so is a named function above its ceiling.
An entry stays after its function is split, as a role's file entry does; a
planning pull request removes it. The script reports the number of functions
above the limit as `functionsOverLimit`, and the longest function of
`LAUNCH_PATH` as `launchPathLongestFunctionCodeLines`.

The script counts the `fatal!` invocations in the files of the library's tree,
outside test items. A function that reaches one stops the root and does not
return, so no host test can call it past that point. The count is reported as
`libraryFatalSites`, and `LIBRARY_FATAL_CEILING` bounds it.

A walk refuses what it cannot follow: a `path` attribute on a declaration, a
declaration inside an inline module, and a declaration that no file or two
files hold. The script also refuses a file that neither walk reaches, other
than the build script and test files, because no build compiles it.

A role may name a file that does not exist. A planning pull request assigns a
file before the implementation creates it, and an entry stays after its file
is deleted.

Known limits. The script counts `slime-root/` only. Code that moves into a
crate the root links leaves the count. `DEPENDENCIES` pins which crates those
are, so a new one is a grader change. A walk reads the `cfg` on a module
declaration and on a whole file. It does not read a `cfg` on an item inside a
file, so an item that the host build skips still counts as compiled. A walk
does not expand macros, so a module that a macro declares is not followed. A
role holds whole files:
`src/object_allocator/qualification.rs` also holds two methods that the
product image calls, and the `harness` role counts them.
"""

from __future__ import annotations

import re
import sys
import tomllib
from dataclasses import dataclass
from pathlib import Path
from typing import NoReturn

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "lib"))

import devloop_observations  # noqa: E402
from harness import ROOT  # noqa: E402
from rust_source import SourceError, is_test_file, lex, modules, sources, test_items  # noqa: E402

CRATE = "slime-root"
MANIFEST = ROOT / CRATE / "Cargo.toml"
BINARY_ROOT = "src/main.rs"
LIBRARY_ROOT = "src/lib.rs"
BUILD_SCRIPT = "build.rs"
FUNCTION = re.compile(r"(?<![A-Za-z0-9_])fn\s+([A-Za-z_][A-Za-z0-9_]*)")
FATAL_SITE = re.compile(r"(?<![A-Za-z0-9_])fatal!\s*[(\[{]")


@dataclass(frozen=True)
class Role:
    """`ceiling` bounds the role's code lines. `files` are paths relative to
    `slime-root/`. `trees` are directory prefixes, each ending in `/`. A tree
    gives its role to a file that no role names."""

    ceiling: int
    files: tuple[str, ...] = ()
    trees: tuple[str, ...] = ()


# Each role ceiling is the role's count on the commit that landed this script,
# rounded up to a multiple of 50. Two are newer. `fixture` is 600 since its
# child crate left `slime-root/`. `boot` is 6050, which leaves room for the
# 5984 lines it holds once `src/boot.rs` carries the boot sequence of `main`.
ROLES: dict[str, Role] = {
    # Kernel-object allocation, address spaces, tasks, faults and the graph.
    "core": Role(
        ceiling=15050,
        files=(
            "src/lib.rs",
            "src/object_allocator.rs",
            "src/private_memory.rs",
            "src/task.rs",
            "src/child_vspace.rs",
            "src/graph.rs",
            "src/transfer_window.rs",
            "src/fault.rs",
            "src/supervision.rs",
            "src/cspace.rs",
            "src/root_cspace.rs",
            "src/launched.rs",
            "src/thread_abi.rs",
            "src/vm_attributes.rs",
            "src/event.rs",
            "src/graph_runtime/adaptive.rs",
            "src/graph_runtime/state.rs",
            "src/graph_runtime/services/spawn.rs",
        ),
        trees=("src/object_allocator/", "src/private_memory/"),
    ),
    # Runs once, before the first component: admission, launch, boot media.
    "boot": Role(
        ceiling=6050,
        files=(
            "src/main.rs",
            "src/boot.rs",
            "src/generation.rs",
            "src/graph_runtime.rs",
            "src/graph_runtime/platform.rs",
            "src/peer_endpoint.rs",
            "src/boot_selector.rs",
            "src/boot_selector_block.rs",
            "src/boot_record.rs",
            "src/framebuffer.rs",
            "src/glyph_font.rs",
        ),
        trees=("src/generation/",),
    ),
    # Device regions, IRQ control and IO resource grants.
    "ioResource": Role(
        ceiling=3050,
        files=(
            "src/io_resource.rs",
            "src/device.rs",
            "src/irq_control.rs",
            "src/graph_runtime/services/io_resource.rs",
        ),
    ),
    # Shared buffers and capability transfer between components.
    "sharedBuffer": Role(
        ceiling=3050,
        files=(
            "src/shared_buffer.rs",
            "src/buffer_adapter.rs",
            "src/graph_runtime/services/capability.rs",
        ),
    ),
    # Operation decoding and the service loop.
    "dispatch": Role(
        ceiling=2550,
        files=("src/ipc.rs", "src/graph_runtime/services.rs"),
    ),
    # Lifecycle, scheduling class, directory, notification and wait-set policy.
    "policy": Role(
        ceiling=2100,
        files=(
            "src/lifecycle.rs",
            "src/scheduling.rs",
            "src/directory.rs",
            "src/notification.rs",
            "src/wait_set.rs",
            "src/graph_runtime/services/policy.rs",
        ),
    ),
    # The platform timer, the timer queue and the clock service.
    "timer": Role(
        ceiling=1350,
        files=("src/timer.rs", "src/platform_timer.rs", "src/clock.rs"),
    ),
    # The root side of the fixture. Its child program is `components/root-child`.
    "fixture": Role(
        ceiling=600,
        files=("src/fixture_runtime.rs",),
    ),
    # Root diagnostics and the console endpoint.
    "console": Role(
        ceiling=450,
        files=(
            "src/console.rs",
            "src/diagnostic.rs",
            "src/graph_runtime/console_runtime.rs",
            "src/graph_runtime/services/diagnostic_once.rs",
        ),
    ),
    # The build script and qualification scenarios of root role images.
    "harness": Role(
        ceiling=1750,
        files=(
            "build.rs",
            "src/object_allocator/qualification.rs",
            "src/private_memory/qualification.rs",
        ),
    ),
}

SERIES = ("core", "boot", "ioResource", "sharedBuffer", "dispatch", "policy", "timer", "fixture", "console")
# The series held 34294 code lines on the commit that landed this script, and
# 33941 after the fixture's child crate left `slime-root/`.
SERIES_CEILING = 34100
# The binary alone compiled 9301 code lines, in 13 files, on the commit that
# landed this measure, and 1119 lines, in `src/main.rs` and
# `src/fixture_runtime.rs`, once the library held the graph runtime and the
# boot sequence.
BINARY_ONLY_CEILING = 1150
# The library compiled 2443 code lines under a `cfg`, in 6 files, on the same
# commit: the boot selector, the boot framebuffer of x86-64 and one
# qualification module.
LIBRARY_CONDITIONAL_CEILING = 2450

# No function may hold more code lines, except those `LONG_FUNCTIONS` names.
FUNCTION_LIMIT = 150
# The functions above the limit on the commit that landed it, as
# `(file, function)`, each with its count then rounded up to a multiple of 50.
# An entry covers every function of that name in that file.
LONG_FUNCTIONS: dict[tuple[str, str], int] = {
    ("src/graph_runtime/services.rs", "serve_instance_graph"): 1500,
    ("src/graph_runtime.rs", "launch_instance_graph"): 1100,
    ("src/graph_runtime/services/spawn.rs", "serve_spawn"): 600,
    ("src/boot.rs", "run"): 550,
    ("src/main.rs", "main"): 450,
    ("src/task.rs", "create"): 450,
    ("src/graph_runtime/services/spawn.rs", "construct_child"): 400,
    ("src/graph_runtime/services/io_resource.rs", "serve_io_resource"): 350,
    ("src/private_memory/qualification.rs", "exercise_failure_rollback"): 350,
    ("src/private_memory/elastic.rs", "grow"): 250,
    ("src/graph_runtime/services/spawn.rs", "preflight_spawn_grants"): 200,
    ("src/graph_runtime/services/capability.rs", "serve_capability_export"): 200,
    ("src/private_memory/qualification.rs", "exercise_mixed_fragmentation"): 200,
    ("src/graph_runtime/services/policy.rs", "serve_lifecycle_request"): 200,
}
# The files that run once, from the console install to the first component.
LAUNCH_PATH = ("src/boot.rs", "src/graph_runtime.rs")
# The library's tree held 160 `fatal!` invocations on the commit that landed
# this measure.
LIBRARY_FATAL_CEILING = 160

# Every crate `slime-root/Cargo.toml` names in a dependency table.
DEPENDENCIES = frozenset({"boot-contracts", "sel4", "slime-proto", "sel4-root-task", "object"})


def fail(message: str) -> NoReturn:
    raise SystemExit(f"root size check failed: {message}")


def count(text: str) -> tuple[int, int]:
    """(code lines, test lines) of one source file."""
    lexed = lex(text)
    test = test_items(text, lexed)
    code_lines = sum(1 for held, tested in zip(lexed.code, test, strict=True) if held and not tested)
    test_lines = sum(1 for held, tested in zip(lexed.code, test, strict=True) if held and tested)
    return code_lines, test_lines


def functions(text: str) -> list[tuple[str, int]]:
    """(name, code lines) of every function with a body, outside test items."""
    lexed = lex(text)
    blanked = lexed.text
    test = test_items(text, lexed)
    found: list[tuple[str, int]] = []
    for declared in FUNCTION.finditer(blanked):
        # The body opens at the first `{` outside the parameter list; a `;` first means no body.
        index, nesting, opened = declared.end(), 0, -1
        while index < len(blanked):
            char = blanked[index]
            if char in "([":
                nesting += 1
            elif char in ")]":
                nesting -= 1
            elif nesting == 0 and char in ";{":
                opened = index if char == "{" else -1
                break
            index += 1
        if opened < 0:
            continue
        nesting, closed = 0, opened
        while closed < len(blanked):
            if blanked[closed] == "{":
                nesting += 1
            elif blanked[closed] == "}":
                nesting -= 1
                if nesting == 0:
                    break
            closed += 1
        first, last = blanked.count("\n", 0, declared.start()), blanked.count("\n", 0, closed)
        if test[first]:
            continue
        found.append((declared.group(1), sum(1 for line in range(first, last + 1) if lexed.code[line] and not test[line])))
    return found


def fatal_sites(text: str) -> int:
    """`fatal!` invocations outside test items."""
    lexed = lex(text)
    test = test_items(text, lexed)
    return sum(1 for site in FATAL_SITE.finditer(lexed.text) if not test[lexed.text.count("\n", 0, site.start())])


def read_source(relative: str) -> str:
    return (ROOT / CRATE / relative).read_text(encoding="utf-8")


def roles_of(relative: str) -> list[str]:
    """Roles that name the file; when none does, roles whose tree holds it."""
    named = [name for name, role in ROLES.items() if relative in role.files]
    return named or [name for name, role in ROLES.items() if any(relative.startswith(tree) for tree in role.trees)]


def dependencies() -> set[str]:
    manifest = tomllib.loads(MANIFEST.read_text(encoding="utf-8"))
    tables = [manifest]
    tables += list(manifest.get("target", {}).values())
    names: set[str] = set()
    for table in tables:
        for kind in ("dependencies", "build-dependencies", "dev-dependencies"):
            names.update(table.get(kind, {}))
    return names


def main() -> int:
    if sys.argv[1:]:
        fail(f"unknown arguments {sys.argv[1:]}; the script takes none")
    for name, role in ROLES.items():
        if any(not tree.endswith("/") for tree in role.trees):
            fail(f"role {name}: a tree must end in '/'")
    if set(SERIES) | {"harness"} != set(ROLES):
        fail("SERIES and harness must name every role once")

    code = dict.fromkeys(ROLES, 0)
    test_lines = 0
    problems: list[str] = []
    files = sources(CRATE)
    if not files:
        fail(f"no Rust file under {CRATE}/")
    for relative in files:
        code_lines, tested = count((ROOT / CRATE / relative).read_text(encoding="utf-8"))
        owners = roles_of(relative)
        if is_test_file(relative):
            test_lines += code_lines + tested
            continue
        if len(owners) != 1:
            problems.append(f"{CRATE}/{relative} has {len(owners)} roles {owners}; a file has exactly one")
            continue
        code[owners[0]] += code_lines
        test_lines += tested

    for root in (BINARY_ROOT, LIBRARY_ROOT):
        if root not in files:
            fail(f"{CRATE}/{root} is missing; a walk starts there")
    try:
        binary_tree = modules(files, BINARY_ROOT, read_source)
        library_tree = modules(files, LIBRARY_ROOT, read_source)
    except SourceError as error:
        fail(f"{CRATE}: {error}")
    for relative in sorted(set(binary_tree) & set(library_tree)):
        problems.append(f"{CRATE}/{relative} is a module of the binary and of the library")
    for relative in files:
        if relative not in binary_tree and relative not in library_tree and relative != BUILD_SCRIPT and not is_test_file(relative):
            problems.append(f"{CRATE}/{relative} is a module of neither the binary nor the library; no build compiles it")

    def code_lines(tree: list[str]) -> int:
        return sum(count((ROOT / CRATE / relative).read_text(encoding="utf-8"))[0] for relative in tree)

    binary = sorted(relative for relative in binary_tree if not is_test_file(relative))
    conditional = sorted(relative for relative, gated in library_tree.items() if gated and not is_test_file(relative))
    binary_lines = code_lines(binary)
    conditional_lines = code_lines(conditional)

    over_limit = 0
    launch_longest = 0
    for relative in files:
        if is_test_file(relative):
            continue
        for name, length in functions(read_source(relative)):
            if relative in LAUNCH_PATH:
                launch_longest = max(launch_longest, length)
            if length <= FUNCTION_LIMIT:
                continue
            over_limit += 1
            ceiling = LONG_FUNCTIONS.get((relative, name))
            if ceiling is None:
                problems.append(f"{CRATE}/{relative}: function {name} holds {length} code lines, above the limit of {FUNCTION_LIMIT}")
            elif length > ceiling:
                problems.append(f"{CRATE}/{relative}: function {name} holds {length} code lines, above its ceiling of {ceiling}")
    library_fatal = sum(fatal_sites(read_source(relative)) for relative in library_tree if not is_test_file(relative))

    series = sum(code[name] for name in SERIES)
    for name, role in ROLES.items():
        print(f"[root-size] {name:<13} {code[name]:>6} / {role.ceiling}")
        if code[name] > role.ceiling:
            problems.append(f"role {name} holds {code[name]} code lines, above its ceiling of {role.ceiling}")
    print(f"[root-size] {'series':<13} {series:>6} / {SERIES_CEILING}")
    print(f"[root-size] files={len(files)} testLines={test_lines}")
    print(f"[root-size] {'binaryOnly':<13} {binary_lines:>6} / {BINARY_ONLY_CEILING} in {len(binary)} file(s)")
    if binary_lines > BINARY_ONLY_CEILING:
        problems.append(f"the binary alone compiles {binary_lines} code lines, above its ceiling of {BINARY_ONLY_CEILING}")
    print(f"[root-size] {'conditional':<13} {conditional_lines:>6} / {LIBRARY_CONDITIONAL_CEILING} in {len(conditional)} file(s)")
    if conditional_lines > LIBRARY_CONDITIONAL_CEILING:
        problems.append(
            f"the library compiles {conditional_lines} code lines under a cfg, above its ceiling of {LIBRARY_CONDITIONAL_CEILING}"
        )
    print(f"[root-size] {'functions':<13} {over_limit:>6} above {FUNCTION_LIMIT} code lines; the longest on the launch path holds {launch_longest}")
    print(f"[root-size] {'fatalSites':<13} {library_fatal:>6} / {LIBRARY_FATAL_CEILING} in the library")
    if library_fatal > LIBRARY_FATAL_CEILING:
        problems.append(f"the library holds {library_fatal} fatal! invocations, above its ceiling of {LIBRARY_FATAL_CEILING}")
    if series > SERIES_CEILING:
        problems.append(f"the series holds {series} code lines, above its ceiling of {SERIES_CEILING}")

    found = dependencies()
    for name in sorted(found - DEPENDENCIES):
        problems.append(f"{CRATE}/Cargo.toml names dependency {name}, which DEPENDENCIES does not")
    for name in sorted(DEPENDENCIES - found):
        problems.append(f"DEPENDENCIES names {name}, which {CRATE}/Cargo.toml does not")

    if problems:
        fail("\n  " + "\n  ".join(problems))
    devloop_observations.record(
        seriesCodeLines=series,
        harnessCodeLines=code["harness"],
        filesObserved=len(files),
        binaryOnlyCodeLines=binary_lines,
        binaryOnlyFiles=len(binary),
        libraryConditionalCodeLines=conditional_lines,
        libraryConditionalFiles=len(conditional),
        functionsOverLimit=over_limit,
        launchPathLongestFunctionCodeLines=launch_longest,
        libraryFatalSites=library_fatal,
        **{f"{name}CodeLines": code[name] for name in SERIES},
    )
    print("root size check: every file has one role, and every role, the series, both walks, every function and the fatal sites are within their bounds")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
