# Exploratory directions register

Parking lot for every direction that follows from the Slime OS vision but is
not committed work. Entries are grouped by status; each keeps its stable
number so cross-references (ROADMAP follow-up notes, the sequencing table)
stay valid. Entries 11–19 were originally README's "Differentiating
directions" section, moved here so README carries a single pointer.

## Rules

- A direction here is not a commitment. It becomes real only when promoted
  into `ROADMAP.md` with an observable exit condition.
- At most one direction may be in `probing` status at a time. A probe is
  time-boxed work (design note or minimal experiment) that ends in either
  promotion to the roadmap or a return to `parked` with the reason recorded.
- A direction that requires violating project invariants (ambient authority,
  kernel-owned policy, non-deterministic formats) is `rejected`, not shelved.
- Dependencies name the roadmap milestone whose mechanisms the direction
  consumes. A direction whose dependency has not landed can only be probed
  as a host-side or paper exercise, never as kernel code.

## Status values

`parked` — registered, no active work.
`probing` — the single active exploration slot.
`promoted` — moved into ROADMAP.md; this register keeps only a pointer.
`rejected` — decided against, with reason.

## Probing

### 24. Checked model of the capability rights algebra

M5.6a/M5.6b established the checked-contract methodology for BootState,
state, and GC semantics, but the authority invariants are enforced only
by Rust code reviewed against the matrix grammar in
`docs/capability-matrix.md`. Model the rights algebra — derive narrows
only, no transfer path widens rights, object-kind validity, and the
format-v2 rights-string mapping — and check that no operation sequence
lets a component exceed the closure of its initial grants.

- Depends on: nothing; host-side contracts work consuming only the
  capability matrix.
- Exit-condition sketch: a checked model in `contracts/` validated by a
  repository target passes the current matrix rules, and fails under a
  narrow-only-derive-removal mutation and a transfer-widening mutation.
- Status: probing. The probe output is the model plus a promotion
  proposal naming the milestone that keeps model and implementation in
  lockstep; if the rights grammar proves to change faster than a model
  can track, record that finding and return to `parked`.

## Promoted

### 4. Generation-consistent state snapshots

Promoted to `ROADMAP.md` as M5.6b (checked generation, state, and GC
transaction model). The checked contract assigns graph-level snapshot epochs,
pairs each bootable generation with one complete state set, and includes every
retained root in GC reachability before M5.6 implementation semantics freeze.

- Status: promoted.

### 6. Formal model of BootState transitions

Promoted to `ROADMAP.md` as M5.6a (checked BootState transition model): a
TLA+ or Alloy spec of the six transition rules and the power-cut matrix,
checked in CI, proving no interleaving leaves zero bootable roots. See
ROADMAP for deliverables and the exit condition.

- Status: promoted.

### 20. BootState model-implementation conformance

Promoted to `ROADMAP.md` as M5.6c. QEMU fault scenarios emit bounded durable
transition traces that are checked against M5.6a and M5.6b, closing the gap
between a valid abstract model and an implementation that may choose different
linearization points.

- Status: promoted.

### 21. Signed generation release metadata

Promoted to `ROADMAP.md` as M5.8. Content hashes identify immutable bytes but
do not authorize a publisher, so deterministic detached release metadata adds
threshold signatures, bounded trust-root rotation, target binding, and replay
ordering before M6 accepts generations transferred from another machine.

- Status: promoted.

### 22. Recovery, scrub, and BootState reconstruction

Promoted to `ROADMAP.md` as M5.9. Signed removable recovery fails closed when
both BootState slots are unusable, verifies generation and state closure before
reconstruction, and receives write authority only for an explicitly selected
repair target.

- Status: promoted.

## Parked

### 1. Authority diff as a build-pipeline gate

All grants are manifest-declared, so two generations can be diffed by
authority: which component gained which rights. The builder emits the diff;
CI requires an explicit sign-off artifact when any component's rights grow.

- Depends on: M5.5 (manifest v2 with 1:1 rights strings). Host-side only.
- Exit-condition sketch: `just generation_diff A B` prints per-component
  grant changes; a build that widens rights without the sign-off file fails.
- Status: parked. Named as an M5.5 follow-up in ROADMAP.

### 2. Revocable and time-bounded grants

The capability matrix has narrow-only `derive` but no revocation story.
Explore kernel-maintained derivation trees so a proxy can revoke its own
subtree, and generation-declared grant lifetimes reclaimed by the health
service. Primary motivation: agent authority should be leaseable ("write
access for thirty minutes"), which the current model cannot express.

- Depends on: provenance follow-up (M5.1); touches capability-table design.
- Exit-condition sketch: a proxy revokes a derived grant; further use by the
  original holder fails with a structured error while sibling grants survive.
- Status: parked. Research-heavy; design note before any kernel change.

### 3. Nondeterminism sources as capabilities

Make wall clock and entropy kernel objects gated by rights. A manifest can
then declare a component deterministic (no clock/entropy grants), making it
a pure function of its IPC inputs: bit-reproducible across boots. This is
the shared foundation the flight recorder, replay, and attestation
directions all implicitly need.

- Depends on: capability-matrix rows for new object kinds; no milestone
  consumes it yet, so it is a matrix amendment proposal first.
- Exit-condition sketch: a manifest-declared deterministic component
  produces byte-identical output across two boots given identical IPC
  inputs.
- Status: parked.

### 5. TPM-bound boot state and attestation

Generations are content-addressed; the Framework target has a TPM. Seal the
BootState attempt counter and known-good hash in TPM NVRAM so a rolled-back
disk image cannot resurrect a known-bad generation, and expose remote
attestation of "this machine runs generation hash X".

- Depends on: M5.6 (BootState), M7-class physical hardware work (TPM driver
  is not currently in any milestone scope).
- Exit-condition sketch: on the Framework target, reflashing an older
  generation image fails stage-0 verification against TPM-held counters.
- Status: parked.

### 7. Schema-driven interposition toolchain

Membranes and dry-run proxies are already claimed in README's agentic
direction. What is missing: because all IPC is schema-first, a membrane can
be generated from `contracts/` — recording, throttling, sanitizing, and
fault injection for any endpoint with zero hand-written protocol code. M5.3
fault injection is the hand-written instance of this general mechanism.

- Depends on: M5.2a contract tooling (exists). Userspace/host tooling.
- Exit-condition sketch: a generated membrane records and replays the block
  protocol; replay reproduces a `storage_fault_check` failure
  deterministically.
- Status: parked. Named as an M5.3 follow-up in ROADMAP.

### 8. Declarative supervision and restart policy

The capability matrix horizon already lists supervision handles as an M6
prerequisite, but the semantics are open. Explore Erlang-style restart
policy as manifest data: restart limits, backoff, and whether state is
`preserve` or `ephemeral` across restarts. "Let it crash" plus capability
re-grant.

- Depends on: M6 spawn-service prerequisites (supervision handles, endpoint
  minting); interacts with M5.6b snapshot semantics and M5.6 fault
  classification.
- Exit-condition sketch: a manifest-declared policy restarts a killed
  component with fresh grants up to its limit, then reports a structured
  failed status through the health service.
- Status: parked.

### 9. Manifest static analysis and grant-graph introspection

The component graph and all grants are declarative, so questions like
"which components can reach block-write" or "what is agent X's worst-case
blast radius" are answerable without running the system. With the M5.1
provenance follow-up, a runtime introspection service can expose the live
grant graph. Provenance answers "why is this allowed"; this answers "what
could happen".

- Depends on: M5.5 (machine-readable grants); provenance for the runtime
  half. Host-side half is tooling only.
- Exit-condition sketch: `just authority_query` answers "which components
  can reach BlockDevice write" from the manifest alone, matching runtime
  provenance on a test graph.
- Status: parked. Named as an M5.5 follow-up in ROADMAP.

### 10. Distributed capabilities

Cross-machine sync (direction 14) moves objects and activation. The step
beyond: a channel endpoint that proxies to a service on another machine,
with grants serialized as unforgeable capabilities over the wire
(CapTP-style). Introduces revocation (direction 2) and partition semantics,
so it stays paper until sync exists.

- Depends on: direction 14; direction 2.
- Exit-condition sketch: none yet; design note only.
- Status: parked.

### 11. IPC flight recorder and deterministic replay

All component input crosses channel boundaries, so recording at that
boundary yields deterministic re-execution of a single component. A bug
report becomes a generation hash plus an IPC trace.

- Depends on: M5.3 already records driver IPC during fault injection and
  names this as the intended foundation; deterministic replay of arbitrary
  components additionally wants direction 3 (nondeterminism as
  capabilities).
- Exit-condition sketch: a recorded trace of a non-driver component
  re-executes byte-identically; a failure report consists of a generation
  hash plus a trace artifact.
- Status: parked. Named as an M5.3 follow-up in ROADMAP.

### 12. Generation bisect

Generations form a content-addressed parent chain, so "which update
regressed this" is automatable as safe boot-and-health-check bisection.

- Depends on: M5.6; ROADMAP names it a follow-up enabled by that milestone.
- Exit-condition sketch: given a known-good and a known-bad generation
  identity, an automated run boots intermediate generations under QEMU
  health checks and identifies the first bad parent link unassisted.
- Status: parked.

### 13. Shadow boot

A pending generation can be health-checked in a constrained sub-graph or
guest VM before real activation consumes a boot attempt.

- Depends on: M5.6; ROADMAP names it a follow-up enabled by that milestone.
- Exit-condition sketch: a deliberately unhealthy pending generation fails
  its shadow health check and is rejected with the real BootState attempt
  counter untouched.
- Status: parked.

### 14. Cross-machine generation sync

A generation is a manifest plus content-addressed objects; moving a system
to a new machine is object transfer plus activation, including capability
grants and state policy — not dotfile reconstruction.

- Depends on: M5.8 release authorization and the M6 transfer path. M6 scope
  already lists "generation sync/transfer between machines"; this entry tracks
  the general capability beyond that minimum.
- Exit-condition sketch: an authorized QEMU-built generation transfers to a
  second machine and activates there with grants and state policy intact.
- Status: parked. Partially scoped by M6; still needs its own exit condition.

### 15. Zutai-defined state migrations

State schema upgrades expressed as pure Zutai transformations are
deterministic, dry-runnable before activation, and covered by the same
rollback contract as the boot graph.

- Depends on: M5.6b state-transaction semantics; Zutai evaluation in the
  build pipeline (host-side is acceptable).
- Exit-condition sketch: a schema v1→v2 migration written in Zutai dry-runs
  against a fixture state binding, then applies during activation; rollback
  restores the v1 binding per policy.
- Status: parked.

### 16. Powerbox UI

Applications never hold an ambient "open file" right; the file dialog is a
system component, and the user's selection gesture itself mints a
single-object capability. Authorization and intent are the same gesture.

- Depends on: M6 scope already lists a powerbox-style file dialog service;
  the capability-matrix horizon tracks the Directory-rights question.
- Exit-condition sketch: a component with no directory grants opens the
  chooser, the user selects a file, and the component receives a
  single-object read capability it could not have obtained otherwise.
- Status: parked. Minimal version is M6 scope; this entry covers the
  general pattern.

### 17. Per-component energy accounting

Scheduler-attributed energy per component and per channel activity, with
policy such as background power budgets carried as grants.

- Depends on: M7 daily-driver quality goals; the capability-matrix horizon
  questions whether accounting is authority or read-only telemetry
  (EnergyAccount row).
- Exit-condition sketch: on the Framework target, a busy-looping background
  component is throttled past its generation-declared energy budget;
  accounting is readable per component.
- Status: parked.

### 18. Per-destination network authority

Network access is a capability to explicit endpoints declared by the
generation, making exfiltration surface auditable in the manifest —
particularly relevant for agent components.

- Depends on: M7 networking; the capability-matrix horizon tracks the
  NetworkDestination object shape.
- Exit-condition sketch: a component holding a capability for one declared
  destination cannot connect to any other address or port; the manifest
  lists every reachable destination.
- Status: parked.

### 19. MPK/PKU lightweight compartments

A third isolation tier between full components and same-address-space code
for latency-sensitive boundaries, using user-space protection keys
available on the target CPU.

- Depends on: M7; explicitly an optional optimization that does not block
  the milestone exit condition.
- Exit-condition sketch: two compartments share an address space; a PKU
  violation in one is reported as a structured fault without terminating
  the other.
- Status: parked.

### 23. Generation build-provenance attestations

Deterministic generation artifacts can carry a host-side attestation naming
the source revision, builder identity and version, build type, normalized
parameters, resolved dependency digests, and resulting generation identity.
Release signatures answer who authorized deployment; provenance separately
answers how the bytes were produced and supports rebuilding and incident
response.

- Depends on: M5.5 deterministic generation output; naturally accompanies the
  M5.8 release pipeline but is not parsed by stage-0.
- Exit-condition sketch: a verifier accepts provenance whose subject matches
  the generation identity and rejects altered inputs, dependency digests,
  builder identity, or output identity.
- Status: parked. Host-side only; promote after the release pipeline has a
  stable builder identity and attestation storage location.

### 25. Resource accounts as capabilities

The capability model governs what a component may do but not how much it
may use: `MAX_TASKS`, `MAX_CAPS`, and `CHANNEL_QUEUE` are global
constants, and the capability-matrix debt register already records
unreaped task-table entries. The matrix horizon names per-spawner
accounting and SharedBuffer creation quota without a unifying story.
Introduce a `ResourceAccount` kernel object (memory pages, task slots,
endpoint slots, queue depth) that spawn charges to the spawner's
account; `derive` splits an account for a child, exhaustion is a
structured error, and child exit returns quota to the parent.
Whole-machine resource allocation becomes generation manifest data:
declarative, auditable, and rollbackable like every other grant.

- Depends on: M6 spawn prerequisites (endpoint minting, non-consuming
  derive-copy, supervision handles); subsumes the matrix horizon rows
  for per-spawner accounting and SharedBuffer creation quota.
- Exit-condition sketch: a service holding a two-task account cannot
  spawn a third; a child's quota returns to the parent account on exit;
  the generation manifest declares the initial account distribution and
  the builder bounds it.
- Status: parked. Genode's resource trading is the reference design;
  the delta here is that the account distribution is generation data
  with rollback semantics.

### 26. Hermetic generation testing

Entries 3 (nondeterminism as capabilities) and 11 (flight recorder and
replay) are independently parked, but their composition is the payoff: a
test generation binds clock and entropy to virtual fixtures so the full
boot-and-health-check run is byte-deterministic in CI. Flaky rollback
and health scenarios become impossible, completing QEMU's tier-0 role
as the deterministic verification platform.

- Depends on: entry 3 (capability-matrix amendment for clock/entropy
  objects); the M5.6 health-confirmation path.
- Exit-condition sketch: two CI runs of the same test generation produce
  byte-identical console and health-transition traces; a fixture clock
  advance deterministically triggers a declared health timeout.
- Status: parked.

### 27. Policy-carrying generations

Stage-0 verifies hashes and format versions; entries 1 and 9 analyze
authority only at build time in CI. A generation can additionally carry
a machine-checkable invariant section — for example, "no component
outside the allowlist reaches `BLOCK_WRITE`" — computed by the builder
from the manifest and re-verified by stage-0 or the health service
before activation. The CI gate becomes a boot gate: hand-editing a
manifest to widen grants without recomputing the invariant section
makes the generation unbootable.

- Depends on: M5.5 machine-readable grants (complete); the M5.6
  activation path.
- Exit-condition sketch: a generation whose grants violate its carried
  invariant is rejected before control transfer; a valid generation
  with a tampered invariant section fails verification.
- Status: parked.

### 28. Accelerator compute objects

The agentic direction makes a language model a userspace service, but
the Framework target's NPU and GPU have no corresponding authority
story: entry 17 accounts energy, not compute. Introduce an
`Accelerator` object kind with queue-submission rights and
generation-declared compute budgets, so an agent component's inference
authority and budget are manifest data like every other grant.

- Depends on: M7 hardware bring-up and IOMMU-enforced DMA; a
  capability-matrix row for the new object kind.
- Exit-condition sketch: a component without the accelerator capability
  cannot submit work; a component past its declared budget is rejected
  or throttled with a structured error; the manifest lists every
  component holding accelerator authority.
- Status: parked.

### 29. Schema-declared state merge

Entry 14 moves objects between machines and entry 15 migrates state
schemas, but neither answers what happens when the same state binding
evolves independently on two machines. Attach a pure Zutai merge
function to a state schema; sync performs a deterministic three-way
merge whose result is byte-identical on both machines, and a
non-mergeable conflict is a structured error that retains both roots.

- Depends on: entries 14 and 15; Zutai evaluation in the sync path
  (host-side acceptable initially).
- Exit-condition sketch: a fixture state diverged on two machines merges
  to byte-identical bytes on both; a schema without a merge function
  rejects divergent sync rather than silently picking a winner.
- Status: parked.

### 30. Deterministic on-device builds

M6 scope includes native components that inspect, build or stage
generations, but build semantics are open. Define a build step as a
manifest-declared deterministic component (consuming entry 3) whose
inputs and outputs are content-addressed objects; the object store
deduplicates build products naturally, and rebuilding the running
system on-device reproduces the host build byte-identically. The
"builds are pure functions" property becomes enforceable inside the OS,
and stage-0 verification applies to locally built generations
unchanged.

- Depends on: entry 3 (deterministic components); the M6 builder; M5.8
  so locally built generations still require release authorization.
- Exit-condition sketch: an on-device build and the host build of the
  same normalized source produce byte-identical generation objects; a
  build component holding no clock or entropy grants cannot embed a
  timestamp.
- Status: parked.

## Rejected

None yet.

## Sequencing

| Wave | Directions | Why then |
| --- | --- | --- |
| 0 — before M5.6 implementation | 6 (M5.6a), 4 (M5.6b) | Both are promoted checked contracts; transition and state/GC semantics must freeze before implementation. |
| 1 — with and after M5.6 | 20 (M5.6c), 1, 9, 12, 13, 24 | Trace conformance closes the model/implementation gap; authority analysis, bisect, and shadow boot consume machine-readable manifests or rollback machinery. Entry 24 is dependency-free contracts work in the M5.6a methodology. |
| 2 — late M5 to M6 | 21 (M5.8), 22 (M5.9), 23, 7, 11 (recording), 8, 3, 14, 15, 16, 11 (replay), 25, 26, 27, 29, 30 | Release trust and recovery must precede cross-machine activation; spawn prerequisites then unlock supervision, resource accounts, migration, powerbox, and general replay. Entries 26 and 30 consume entry 3, entry 27 consumes the M5.6 activation path, and entry 29 follows 14 and 15. |
| 3 — M7 and beyond | 5, 2, 10, 17, 18, 19, 28 | Physical TPM, revocation, distributed authority, accelerator control, and daily-driver hardware respectively. |

## Research references

These sources informed entries 4 and 20–25; the resulting contracts remain
Slime-specific rather than adopting any external system wholesale.

- [Genode Foundations](https://genode.org/documentation/genode-foundations/)
  informs entry 25's account-derived resource delegation; the Slime delta is
  carrying the account distribution as rollbackable generation data.

- [TLA+ implementation trace validation](https://arxiv.org/html/2404.16075v2)
  motivates M5.6c's finite-trace conformance check and documents its limits.
- [The Update Framework specification](https://theupdateframework.github.io/specification/latest/)
  informs M5.8's threshold trust, versioned root rotation, and replay checks.
- [Android A/B updates](https://source.android.com/docs/core/ota/ab) and
  [Verified Boot flow](https://source.android.com/docs/security/features/verifiedboot/boot-flow)
  motivate retaining a bootable fallback and advancing rollback protection
  only after the pending system is confirmed successful.
- [OSTree atomic upgrades](https://ostreedev.github.io/ostree/atomic-upgrades/),
  [OSTree deployments](https://ostreedev.github.io/ostree/deployment/), and
  [Nix GC roots](https://nix.dev/manual/nix/2.34/package-management/garbage-collector-roots)
  inform M5.6b's deployment/state pairing and explicit reachability roots.
- [seL4 capDL](https://docs.sel4.systems/projects/capdl/) informs the static
  authority questions in entries 1 and 9.
- [SLSA build provenance](https://slsa.dev/spec/v1.2/build-provenance) informs
  entry 23's separation of build provenance from release authorization.
