# P5.4.3 — M6.3's directory mechanism, in the root

| Field | Value |
|---|---|
| Date | 2026-08-08 |
| Kind | Change |
| Status | Verified |
| Scope | `slime-root/src/{graph,main,ipc}.rs`, `components/runtime/src/{lib,syscall}.rs`, `components/bins/src/bin/{sel4-directory-probe,init}.rs`, `components/bins/{Cargo.toml,build.rs}`, `components/bins/src/default_boot_layout.rs`, `contracts/generation/v1/fixtures/sel4-directory.zti`, `scripts/build/{boot_layout,build-generation,build-sel4}.py`, `scripts/check/check-sel4-{directory-plane,component-graph,root-boot,boot-layout,gate-controls}.py`, `Justfile` |
| Roadmap | P5.4.3, P5.4, M6.3 |
| Gates | `just sel4_directory_check`, `just sel4_loan_check`, `just sel4_root_boot_check`, `just sel4_component_graph_check`, `just sel4_boot_layout_check`, `just sel4_gate_control_check`, `just test_sel4_root`, `just lint_all`, `just fmt_check_all`, `just ruff`, `just typos`, `just devlog_check` |
| Trigger | M6.5 closed; M6.3 blocks M6.4 and M6.6, and two scouts found it needs real root mechanism |
| Baseline | `DirectoryInspect`, `DirectoryDerive`, and `DirectoryCommit` all answered `Mediation::Unavailable` |

## Summary

The root now owns directory capabilities: a shared namespace root, scoped views
that derivation may only narrow, and an atomic compare-and-swap commit. A
component holding one unscoped view derives narrower ones, is refused a stale
commit and a scoped one, and sees its commits through every view.

**This is the first P5.4 slice where the answer was "the root must own it."**
Every previous one — the object store, rollback, recovery, generation commands —
moved policy *out* of the kernel into a component. M6.3 splits: what a directory
*contains* is a filesystem component's business over the object store, but the
capability itself is unforgeable shared state with an atomic transition, and
that has to live somewhere neither holder controls.

Three of the eight remaining `Mediation::Unavailable` operations are now
mediated, leaving five.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `graph.rs` | `Resource::Directory { namespace, scope }`, `ScopeId`, `ScopeTable`, `valid_directory_path` | A capability names a view, and a view cannot widen |
| `main.rs` | `Namespaces`: one root per namespace, compare-and-swap commit | Two writers cannot silently lose an update |
| `main.rs` | `serve_directory_{inspect,derive,commit}` | The three operations, gated as the oracle gates them |
| `main.rs` | `Role::DirectoryRoot` placement at boot and in `construct_child` | A declared holder gets its view; a spawned child gets it too |
| `ipc.rs` | The three operations moved to `Mediation::RootService` | The unmediated surface is five, not eight |
| `syscall.rs` | `DIRECTORY_ROOT_BYTES` published | A caller can size its buffer without reading the transport |

### The regression this slice caused, and what it taught

The first implementation inlined a 128-byte `DirectoryScope` directly into
`Resource::Directory`. It compiled, and the directory plane passed.

`just sel4_loan_check` then failed with `Caught cap fault in send phase` on
`init`'s exit path — a plane that has nothing to do with directories.

A `Resource` is copied into every capability slot, and there are
`MAX_TASKS × MAX_CAPS` = 48 × 64 of them. Adding 128 bytes to the variant grew
`GraphTables` from roughly 96 KiB to 432 KiB and cost the root its stack.

Fixed by interning: capabilities carry a `ScopeId`, and `ScopeTable` holds the
paths. That also makes the common case free — almost every directory capability
is unscoped, and every unscoped one shares `ScopeTable::ROOT`.

Worth recording as a rule: **an enum copied into a fixed-size table pays its
largest variant everywhere.** The gate that caught it was unrelated to the
change, which is the argument for running all of them.

### Why these three are mechanism when `StoreTransact` is not

`StoreTransact` names policy — partition selection, allocation, commit ordering
— so mediating it would put decisions in the root that belong above it.

`DirectoryDerive` names none. It answers "may this holder produce a narrower
view of what it already has", and the honest answer requires state the holder
cannot forge. Same for the commit: the compare is only meaningful if the
compared value lives where neither writer can edit it. The root owns the
capability graph already; a directory capability is one more edge in it.

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A scope escapes outward | `valid_directory_path` rejects `..`, absolute, empty segments, trailing slash; the probe tries five | "an escaping path was accepted" |
| Derivation widens rights | `rights & !source.rights` refuses | "widening derive accepted" |
| Holding a view implies handing out views | derive resolves on `RIGHT_DIRECTORY_DERIVE` alone | "derive without the derive right accepted" |
| A subtree replaces the namespace | commit requires an unscoped writer | "scoped commit accepted" |
| A reader commits | commit resolves on `RIGHT_DIRECTORY_WRITE` | "read-only commit accepted" |
| A stale writer discards another's work | compare-and-swap; the gate counts exactly one stale refusal | "the root recorded N stale commits" |
| A refusal is reported but not honoured | the gate counts the root's *own* commit records: exactly 2, distinct roots | "the root recorded N commits" |
| A view is a snapshot rather than a view | a commit through the unscoped view must be visible through a scoped one | "the scoped view did not see the commit" |
| The `Resource` grows again | `just sel4_loan_check` and `just sel4_root_boot_check` | cap fault; CSlot pin drift |
| The mediated surface silently shrinks | `sel4_component_graph_check` pins it at five operations | the surface check fails |
| The gate loses evidence | `just sel4_gate_control_check`, pinned at 17 markers | a mutated transcript is accepted |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just sel4_directory_check` | Pass; 17 markers, 2 commits, 1 stale refusal, 3 derivations | Direct |
| `just sel4_loan_check` | Pass; the regression above is fixed | Direct |
| `just sel4_root_boot_check` | Pass; CSlot base repinned 853 → 855 for the namespace table | Direct |
| `just sel4_component_graph_check` | Pass; unmediated surface is five operations | Direct |
| `just sel4_gate_control_check` | Pass; 21 gates reject mutated transcripts and layouts | Direct |
| `just sel4_boot_layout_check` | Pass; 18 plane layouts match their fixtures | Direct |
| The other nineteen seL4 plane gates | Pass | Direct |
| `just test_sel4_root`, `just contracts_check`, `just generation_check` | Pass | Direct |
| `just lint_all`, `just fmt_check_all`, `just ruff`, `just typos`, `just devlog_check` | Pass | Direct |
| A filesystem *service* over this mechanism | Not built — see below | — |

## Decisions

- **Decision:** Intern scopes rather than inline them.
  **Rationale:** forced by the loan-plane fault, and better anyway: the unscoped
  case costs nothing, and identical derived scopes share an entry.

- **Decision:** Allow `RIGHT_TRANSFER` on a directory, unlike a block device.
  **Rationale:** M6.3 requires narrow-only directory *transfer*, and M6.6's
  powerbox exists to hand a requester one narrowed view. `serve_directory_derive`
  refuses to add the bit to a capability that does not carry it, so the
  delegation cannot be manufactured.

- **Decision:** Keep the namespace root opaque to the root task.
  **Rationale:** it is a content hash naming a directory object in the store.
  Interpreting it would drag the object store back into the root, undoing
  P5.4.2c.

- **Decision:** No disk in this plane.
  **Rationale:** the mechanism touches no device, and a gate that attached one
  would imply it did.

## Open risks and follow-ups

- [ ] **No filesystem service yet.** M6.3's exit condition is "components browse
      and mutate namespaces only through explicit directory capabilities with
      store-verified metadata"; this covers the capability half, not the
      browsing. A service resolving names through the object store, and the
      `contracts/fs/v1` operations over it, is the other half — the oracle has
      one in `components/bins/src/bin/filesystem-service.rs`, and it is portable
      because it is policy.
- [ ] `MAX_NAMESPACES` is 1. The resource carries an index so raising it is a
      table change, but nothing creates a second namespace.
- [ ] `contracts/capability-transfer` defines no directory kind, so a directory
      cannot ride a channel yet. M6.6's powerbox needs one; that is a schema
      change plus a `descriptor_names` arm.
- [ ] M6.4 (dango) additionally needs `InputRead`, still unmediated.

## Artifacts and provenance

- Gate output, the root's own mechanism records, and the observed rights masks:
  [`directory-check.txt`](directory-check.txt).
- The plane whose fault exposed the sizing regression:
  `just sel4_loan_check`.
- The slice that closed M6.5 and ranked this one next:
  [`devlog/2026-08-08-p5-4-3-generation-plane/`](../2026-08-08-p5-4-3-generation-plane/index.md).
- Related roadmap item: P5.4.3 in
  [`roadmap/07-architecture-portability.md`](../../roadmap/07-architecture-portability.md).
