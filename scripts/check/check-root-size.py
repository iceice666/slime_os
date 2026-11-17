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

A role may name a file that does not exist. A planning pull request assigns a
file before the implementation creates it, and an entry stays after its file
is deleted.

Known limits. The script counts `slime-root/` only. Code that moves into a
crate the root links leaves the count. `DEPENDENCIES` pins which crates those
are, so a new one is a grader change. A role holds whole files:
`src/object_allocator/qualification.rs` also holds two methods that the
product image calls, and the `harness` role counts them.
"""

from __future__ import annotations

import subprocess
import sys
import tomllib
from dataclasses import dataclass
from pathlib import Path
from typing import NoReturn

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "lib"))

import devloop_observations  # noqa: E402
from harness import ROOT  # noqa: E402

CRATE = "slime-root"
MANIFEST = ROOT / CRATE / "Cargo.toml"
TEST_ATTRIBUTE = "#[cfg(test)]"


@dataclass(frozen=True)
class Role:
    """`ceiling` bounds the role's code lines. `files` are paths relative to
    `slime-root/`. `trees` are directory prefixes, each ending in `/`. A tree
    gives its role to a file that no role names."""

    ceiling: int
    files: tuple[str, ...] = ()
    trees: tuple[str, ...] = ()


# Each role ceiling is the role's count on the commit that landed this script,
# rounded up to a multiple of 50.
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
        ceiling=5900,
        files=(
            "src/main.rs",
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
    # The fixture root and its child image.
    "fixture": Role(
        ceiling=950,
        files=("src/fixture_runtime.rs",),
        trees=("child/",),
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
# The series held 34294 code lines on the commit that landed this script.
SERIES_CEILING = 34400

# Every crate `slime-root/Cargo.toml` names in a dependency table.
DEPENDENCIES = frozenset({"boot-contracts", "sel4", "slime-proto", "sel4-root-task", "object"})


def fail(message: str) -> NoReturn:
    raise SystemExit(f"root size check failed: {message}")


@dataclass
class Lexed:
    """What the lexer keeps of one file, per line."""

    code: list[bool]  # the line holds a character outside whitespace and comments
    depth: list[int]  # delimiter depth before the line's first character
    ends: list[bool]  # the line holds a `;` or `}` that leaves the depth at 0


def lex(text: str) -> Lexed:
    """Read Rust's lexical structure: comments, literals and delimiters."""
    lines = text.count("\n") + 1
    out = Lexed([False] * lines, [0] * lines, [False] * lines)
    line = depth = index = 0
    size = len(text)

    def span(end: int, held: bool) -> int:
        """Pass a comment or a literal that ends before `end`; a literal's text is code."""
        nonlocal line
        for position in range(index, min(end, size)):
            if text[position] == "\n":
                line += 1
                out.depth[line] = depth
            elif held and not text[position].isspace():
                out.code[line] = True
        return min(end, size)

    while index < size:
        char = text[index]
        if char.isspace():
            index = span(index + 1, False)
        elif text.startswith("//", index):
            end = text.find("\n", index)
            index = size if end < 0 else end
        elif text.startswith("/*", index):
            nesting, end = 1, index + 2
            while end < size and nesting:
                if text.startswith("/*", end):
                    nesting, end = nesting + 1, end + 2
                elif text.startswith("*/", end):
                    nesting, end = nesting - 1, end + 2
                else:
                    end += 1
            index = span(end, False)
        elif char == '"':
            end = index + 1
            while end < size and text[end] != '"':
                end += 2 if text[end] == "\\" else 1
            index = span(end + 1, True)
        elif char == "r" and raw_string(text, index) and not identifier_before(text, index):
            hashes = raw_string(text, index) - 1
            end = text.find('"' + "#" * hashes, index + hashes + 2)
            index = span(size if end < 0 else end + hashes + 1, True)
        elif char == "'":
            index = span(character_literal(text, index), True)
        else:
            out.code[line] = True
            if char in "{([":
                depth += 1
            elif char in "})]":
                depth -= 1
            if char in ";}" and depth == 0:
                out.ends[line] = True
            index += 1
    return out


def raw_string(text: str, index: int) -> int:
    """For `r"`, `r#"`, ... at `index`: one more than the count of `#`. Otherwise 0."""
    end = index + 1
    while end < len(text) and text[end] == "#":
        end += 1
    return end - index if end < len(text) and text[end] == '"' else 0


def identifier_before(text: str, index: int) -> bool:
    """Whether the `r` at `index` continues an identifier other than the `b` of `br"..."`."""
    if index == 0:
        return False
    before = text[index - 1]
    if before == "b":
        return index >= 2 and (text[index - 2].isalnum() or text[index - 2] == "_")
    return before.isalnum() or before == "_"


def character_literal(text: str, index: int) -> int:
    """The end of the character literal at `index`, or `index + 1` for a lifetime."""
    if text.startswith("\\", index + 1):
        end = text.find("'", index + 3)
        return index + 1 if end < 0 else end + 1
    return index + 3 if text.startswith("'", index + 2) else index + 1


def count(text: str) -> tuple[int, int]:
    """(code lines, test lines) of one source file."""
    lexed = lex(text)
    lines = text.split("\n")
    test = [False] * len(lines)
    number = 0
    while number < len(lines):
        if lines[number].strip() == TEST_ATTRIBUTE and lexed.depth[number] == 0:
            # The item ends at its first `;` at depth 0, or at the `}` that returns to depth 0.
            end = next((n for n in range(number + 1, len(lines)) if lexed.ends[n]), len(lines) - 1)
            for covered in range(number, end + 1):
                test[covered] = True
            number = end + 1
        else:
            number += 1
    code_lines = sum(1 for held, tested in zip(lexed.code, test, strict=True) if held and not tested)
    test_lines = sum(1 for held, tested in zip(lexed.code, test, strict=True) if held and tested)
    return code_lines, test_lines


def is_test_file(relative: str) -> bool:
    name = relative.rsplit("/", 1)[-1]
    return name == "tests.rs" or name.endswith("_tests.rs") or "/tests/" in f"/{relative}"


def roles_of(relative: str) -> list[str]:
    """Roles that name the file; when none does, roles whose tree holds it."""
    named = [name for name, role in ROLES.items() if relative in role.files]
    return named or [name for name, role in ROLES.items() if any(relative.startswith(tree) for tree in role.trees)]


def sources() -> list[str]:
    """Rust files under `slime-root/` that Git tracks or does not ignore, relative to it."""
    listed = subprocess.run(
        ["git", "ls-files", "--cached", "--others", "--exclude-standard", "-z", "--", CRATE],
        cwd=ROOT, check=True, capture_output=True, text=True,
    ).stdout.split("\0")
    prefix = f"{CRATE}/"
    return sorted(path[len(prefix):] for path in listed if path.endswith(".rs") and (ROOT / path).is_file())


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
    for name, role in ROLES.items():
        if any(not tree.endswith("/") for tree in role.trees):
            fail(f"role {name}: a tree must end in '/'")
    if set(SERIES) | {"harness"} != set(ROLES):
        fail("SERIES and harness must name every role once")

    code = dict.fromkeys(ROLES, 0)
    test_lines = 0
    problems: list[str] = []
    files = sources()
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

    series = sum(code[name] for name in SERIES)
    for name, role in ROLES.items():
        print(f"[root-size] {name:<13} {code[name]:>6} / {role.ceiling}")
        if code[name] > role.ceiling:
            problems.append(f"role {name} holds {code[name]} code lines, above its ceiling of {role.ceiling}")
    print(f"[root-size] {'series':<13} {series:>6} / {SERIES_CEILING}")
    print(f"[root-size] files={len(files)} testLines={test_lines}")
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
        **{f"{name}CodeLines": code[name] for name in SERIES},
    )
    print("root size check: every file has one role, and every role and the series are within their ceilings")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
