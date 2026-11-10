---
name: spec-review
description: Adversarial review of an implementation PR against its landed task/v2 work item. Use after implementation and before the item's evidence is recorded on `main`, or when asked to audit whether a checker actually asserts what the item's requirements claim. Reads the item and the diff only, never the implementer's conversation.
---

# Adversarial spec review

You are the reviewer, not the implementer. Your job is to find where the
implementation, its checker, and the work item disagree — and to say so with a
file and line, not an impression. A clean tool run is not a finding of
correctness; a passing record proves only that the recipe exited zero at the recorded
closure digest.

## Inputs you take, and inputs you refuse

Take:

- The work item: `.tasks/items/<slug>.zt`, read in full as Zutai source —
  problem, area, `owns`, `nonGoals`, requirements, acceptances, predicates,
  invariants, and decisions. The item's area file `specs/<area>.zt` adds the
  invariants that bind it.
- The item's state: `just spec_tasks_status` prints each acceptance as
  enforced, failing, claimed, or advisory, read from the `evidence` branch.
- The targets the acceptances bind and their composition:
  `just spec_closure --explain` lists, per bound target, the recipe
  definitions, grader files, declared inputs, and image derivations that
  decide its closure digest.
- The diff: `git diff origin/main...HEAD`, plus the checker and recipe each
  bound target runs, read in full.
- The judge's verdict: `just spec_check --base origin/main` prints the PR's
  class and any V2 or V3 refusal.

Refuse:

- The implementer's transcript, summary, or commit message as evidence. Read
  the commit message for claims to test, never for results.
- "Verification: … passed" lines. Reproduce the narrowest one yourself if the
  finding depends on it; otherwise treat it as unverified.

## Procedure

Work the four tables below in order and write them out. A row you cannot fill
is a finding.

### 1. Requirement → assertion

For every requirement, list each concrete claim in its `statement`, and in
any `decisions` entry that binds it, with the exact line in the checker that
asserts it. Claims are things like "compares
exact bidirectional payload bytes", "negative controls fail when bytes are
absent", "normal shutdown alone cannot satisfy this", "rejects stale
completions and then exchanges new bytes".

| requirement | claim in statement | checker file:line that asserts it | how it would fail |

A claim with no asserting line is **drift**: the item promised it and the
target does not observe it. A claim asserted only by the recipe's exit code
inherits every weakness of that checker. When the acceptance carries
predicates (`h.observedWhere`), check that the reported observation is
*counted* from
transcript or output evidence, not assigned from a literal or from a
configuration value the fixture already knows.

### 2. Negative controls

For every positive assertion in table 1, name the mutation that would make
it fail and where that mutation is exercised (`check-sel4-gate-controls.py`,
the checker's own controls, or a host test). Missing marker, reordered chain,
explicit failure marker, wrong count, absent bytes, wrong holder. An assertion
with no control is unproven: a regex that never matched anything also never
fails.

### 3. Self-grading

Answer directly:

- Did this PR add or change the checker, marker chain, fixture, or recipe
  that its own acceptances bind? Which files? `scripts/spec/check` refuses
  a bound grader file in an implementation PR (V3). A recipe or checker
  outside every grader set escapes V3, so ask whether the item's targets
  reach it through `just spec_closure --explain`.
- Did it change the item's `owns`, requirements, or acceptances? Only an
  `intent` change may ride in an implementation PR. Any other item edit is
  a planning change.
- Did it change the expected count, bound, or marker in a way that matches
  what the implementation happens to produce? Ask for the fixture-side
  reason the number is what it is.
- Does the item's `nonGoals` list anything the diff does anyway?

### 4. Scope and ownership

- Every implementation file in the diff sits under the item's `owns` (V2)
  and maps to a requirement. List the ones that do not map.
- Generated outputs (`@generated`, `boot-contracts/src/generated/`, derived
  manifests, `contracts/system-image-closure/v2/`) changed only alongside
  their sources, and the closures were regenerated after the last source
  edit — `just system_image_builder_check` and `just system_spec_check` are
  the arbiters.
- Owning docs changed when a contract, procedure, or limitation did
  (`docs/architecture/`, `docs/plans/`, `docs/capability-matrix.md`).
- Claims about hardware, targets, or images are bound to the target, image
  identity, and revision that was actually run.

## seL4 and capability checklist

Slime OS is a capability system on seL4; these are the questions a
domain-blind reviewer will not think to ask. Answer each with a file and line
or "not touched by this diff".

**Authority**

- Does any new operation execute without a rights check in `slime-root/src/ipc.rs`
  or the owning mechanism module? A rights bit must name one root-checked
  operation (`docs/capability-matrix.md` grammar).
- Can a component obtain a capability it was not granted in
  `contracts/system-spec/v1/systems/<plane>.zti`? Look for self-grants, slot
  numbers hard-coded in a component, and transfer paths that skip
  transferability.
- Is any authority ambient: a global path, an environment assumption, a
  well-known slot the code assumes rather than the generation declares?
- Do loopback, same-process, or "trusted" paths skip a check the remote path
  performs?

**Object lifetime and reclamation**

- For every seL4 object the change allocates (CNode, TCB, endpoint,
  notification, frame, untyped retype): where is it reclaimed on task
  death, fault, restart, and normal close? Is the accounting in
  `slime-root/src/object_allocator.rs` decremented on every path?
- After a restart or epoch rotation, is every predecessor handle, queued
  completion, mapped buffer, timer, and charge either settled exactly once or
  refused? "Settled" means the client observes a terminal result.
- Can a handle from epoch N be presented in epoch N+1 and be accepted? Where
  is the epoch compared?

**Memory and IPC boundaries**

- Shared buffers: who maps, who unmaps, and in what order relative to the
  loan or transfer that referenced them? Can the peer keep a mapping after
  the owner reclaimed?
- IPC message bounds: every length, offset, and count from a peer is
  validated before use (`slime-root/src/peer_endpoint.rs`,
  `components/runtime/src/syscall/sel4_transport.rs`).
- Does any code copy from a buffer the peer can still write (TOCTOU across
  the validation and the use)?
- DMA and device memory: is the driver's authority bounded to the rings or
  regions the generation granted, or does it get the device?

**Supervision, faults, and progress**

- A child dying mid-operation: what state does the root or service hold on
  its behalf, and who frees it?
- Is any wait unbounded or any loop a busy poll without a declared budget?
  Coalesced notifications must drain all ready work.
- Do quotas and budgets (buffers, sockets, queues, retries) exist *before*
  activation, and are they per-holder rather than global?

**Determinism and evidence**

- Do the plane checkers compare semantic fields, or byte-identical
  transcripts that will flake on ordering? Are poll-sampled counters exempted
  deliberately?
- Does the generation stay deterministic: same inputs, byte-identical output
  (`just generation_check`)?

## Report

Write findings first, ordered by severity, each with file:line and the
item line it contradicts. Then the four tables. Then what you did not
verify. Do not write "looks good"; write what you checked and what remains
unproven. A review that produces no findings should still list every
requirement claim and its asserting line, because that table is the record the
next reviewer starts from.
