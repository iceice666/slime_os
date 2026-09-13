# 26. Hermetic generation testing

| | |
| --- | --- |
| Status | parked |
| Route | determinism |
| Depends on | [entry 3](03-nondeterminism-as-capabilities.md) (capability-matrix amendment for clock/entropy objects); the M5.6 health-confirmation path (complete) |
| Enables | flake-free rollback/health CI; completes QEMU's tier-0 role as the deterministic verification platform |
| Now | Blocked on entry 3's amendment; the test-generation and fixture design (which health scenarios need which fixtures) is a paper exercise legal today. |

## Motivation

Entries [3](03-nondeterminism-as-capabilities.md) and
[11](11-flight-recorder-replay.md) are independently parked, but their
composition is the payoff: a test generation binds clock and entropy to
virtual fixtures so the full boot-and-health-check run is
byte-deterministic in CI. Flaky rollback and health scenarios become
impossible — a health timeout fires because the fixture clock advanced
past the declared deadline, not because CI was slow that day.

## What exists today

- M5.6 (complete) provides the health-confirmation path:
  `GenerationControl` authority minted in kernel bootstrap, transferred
  once to the generation-management service, and `just rollback_check`
  exercising pending → attempt consumption → automatic return to
  known-good.
- The object-store and generation formats (M5.4, M5.5, complete) let a
  test generation be content-addressed and sealed like any other.
- Missing: clock/entropy as capability-gated objects (entry 3), without
  which a fixture cannot be substituted for the real source by manifest
  wiring.

## Design sketch

A test generation is a normal generation whose manifest binds the
fixture implementations: a virtual clock object advanced by the test
harness, a seeded entropy stream. Because fixture and hardware implement
the same rights (entry 3's design), the components under test are
identical bytes — only the manifest wiring differs, and that difference
is itself auditable manifest data.

Determinism claim: two CI runs of the same test generation produce
byte-identical console and health-transition traces. This requires the
fixture clock to drive everything the health path observes — timeouts,
attempt deadlines — so the harness, not wall time, decides when the
pending generation is declared failed. QEMU remains the platform; the
claim is about the software stack inside it.

Residual nondeterminism lives below the rights line: timer IRQ timing,
scheduler interleaving. The entry's scope decision is whether
IPC-order determinism suffices (components observe the same message
sequence) or whether instruction-level determinism is claimed. The
former is the pragmatic target; the latter belongs to a different
project.

## Open questions

- Which health scenarios genuinely need fixture time — attempt
  consumption is durable-state driven, but health timeouts are
  wall-clock driven today [INFERENCE: from the timeout's role in M5.6
  fault classification].
- Does the fixture clock need a capability of its own (who may advance
  time — the harness only)?
- Console trace as the determinism oracle: is the serial log stable
  enough to be the compared artifact, or should the health-transition
  trace be a structured object instead?

## Exit-condition sketch

Two CI runs of the same test generation produce byte-identical console
and health-transition traces; a fixture clock advance deterministically
triggers a declared health timeout.

## Probe guidance

Paper today: enumerate the health/rollback scenarios in
`rollback_check` and classify each by its nondeterminism source (durable
state, wall clock, entropy, IRQ timing) to size what the fixtures must
cover. Execution requires entry 3's amendment to land first.
