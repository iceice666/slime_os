"""Shared serial-marker matching for seL4 plane gates and their controls."""

from __future__ import annotations

import re
from collections.abc import Callable, Iterable, Sequence
from typing import NoReturn

Chain = tuple[str, tuple[str, ...]]
Reject = Callable[[str], NoReturn]


def chains_from_gate(gate: object) -> tuple[Chain, ...]:
    """Return a gate's declaration without flattening causal chains.

    A gate may also declare `EXPECTED_UNORDERED`: markers it requires but whose
    position is not causally ordered against its chains — an independent task's
    completion, typically. Those are appended as their own pseudo-chain so the
    meta-gate's coverage count still sees them. Without this, moving a racy
    marker out of a causal chain reads as *lost* coverage rather than as the same
    coverage asserted correctly (B63).
    """
    chains = getattr(gate, "CHAINS", None)
    if chains is not None:
        declared = [(label, tuple(patterns)) for label, patterns in chains]
    else:
        markers = getattr(gate, "REQUIRED_MARKERS", None)
        if markers is None:
            markers = getattr(gate, "MARKERS", None)
        if markers is None:
            raise AttributeError("gate declares neither CHAINS, REQUIRED_MARKERS, nor MARKERS")
        declared = [("required marker sequence", tuple(pattern for _, pattern in markers))]
    unordered = tuple(getattr(gate, "EXPECTED_UNORDERED", ()))
    if unordered:
        declared.append(("order-independent markers", unordered))
    return tuple(declared)


def marker_count(chains: Sequence[Chain]) -> int:
    return sum(len(patterns) for _, patterns in chains)


def match_marker_contract(
    transcript: str,
    chains: Iterable[Chain],
    failure_markers: Iterable[str],
    reject: Reject,
    *,
    before_reject: Callable[[], None] | None = None,
) -> None:
    """Reject failure evidence and missing or out-of-order markers."""
    for pattern in failure_markers:
        match = re.search(pattern, transcript)
        if match is not None:
            if before_reject is not None:
                before_reject()
            reject(f"failure marker in serial transcript: {match.group(0)!r}")
    for label, patterns in chains:
        position = 0
        for pattern in patterns:
            match = re.compile(pattern).search(transcript, position)
            if match is None:
                if before_reject is not None:
                    before_reject()
                if re.search(pattern, transcript) is not None:
                    reject(f"{label}: marker out of order: {pattern}")
                reject(f"{label}: missing marker: {pattern}")
            position = match.end()
