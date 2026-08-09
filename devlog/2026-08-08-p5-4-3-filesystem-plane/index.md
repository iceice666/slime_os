# P5.4.3 — M6.3's filesystem service, and the oracle's client unmodified

| Field | Value |
|---|---|
| Date | 2026-08-08 |
| Kind | Change |
| Status | Verified |
| Scope | `contracts/capability-transfer/v1/{schema.zt,gen_rust.zt}`, `components/proto/src/capability_transfer.rs`, `slime-root/src/{graph,main}.rs`, `components/bins/src/bin/{sel4-filesystem-service,init}.rs`, `components/bins/{Cargo.toml,build.rs}`, `components/bins/src/default_boot_layout.rs`, `contracts/generation/v1/fixtures/sel4-filesystem.zti`, `scripts/build/{boot_layout,build-generation,build-sel4}.py`, `scripts/check/check-sel4-{filesystem-plane,directory-plane,root-boot,boot-layout,gate-controls}.py`, `Justfile` |
| Roadmap | P5.4.3, P5.4, M6.3 |
| Gates | `just sel4_filesystem_check`, `just sel4_directory_check`, `just sel4_root_boot_check`, `just sel4_boot_layout_check`, `just sel4_gate_control_check`, `just contracts_check`, `just test_sel4_root`, `just lint_all`, `just fmt_check_all`, `just ruff`, `just typos`, `just devlog_check` |
| Trigger | M6.3's mechanism half landed; the service half was the other blocker for M6.4 and M6.6 |
| Baseline | No seL4 filesystem service; `objectKindDirectory` did not exist |

## Summary

A filesystem service now runs on seL4: it resolves names inside a snapshot tree,
reads and writes objects through the content-addressed store, and derives
subdirectory capabilities on request.

**The client is the oracle's own `directory-probe`, byte for byte.** It is
shared with `just directory_check` and drives the seL4 service without knowing
the service exists. That is the finding: M6.3's userspace half is policy, and
policy ports. What changed underneath is that object bytes come from
`boot_contracts::object_store` over a granted block capability rather than from
a kernel `store_transact` with an ambient `buffer_addr` pointer.

M6.3 is now closed on both halves — the mechanism in the root, the service above
it.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `contracts/capability-transfer/v1` | `objectKindDirectory = 5` | A directory view can be described on the wire |
| `graph.rs` | `is_transferable` admits `Directory` | A client can hand its view to a service |
| `main.rs` | `RIGHT_TRANSFER` in the directory placement mask | `transferable = true` reaches the capability |
| `sel4-filesystem-service.rs` | The oracle's service with a userspace store backend | M6.3's service half runs above the root |
| `init.rs` | `drive_filesystem_plane` | Two components, one channel, no device from init |
| `main.rs` | `DIRECTORY_FIXTURE_ROOT` seeds namespace 0 | A component finds a tree to resolve |

### Why a client hands over its own capability

The service holds a directory view of its own and still does not use it. Every
request carries the *client's* view, attached to the message, and the service
resolves through that.

This is the right shape rather than an accident of the oracle's design: a
service acting on its own authority would be a confused deputy — every client
would get whatever the service could reach. Handing the view across means the
service acts with exactly the caller's authority, and the root narrows it on the
way. It is also what forced `objectKindDirectory`: a capability that must cross
a channel needs a wire kind.

### Three findings while bringing this up

1. **`transferable = true` was reaching the authority but not the capability.**
   The directory placement mask was `RIGHTS_DIRECTORY_ALL`, which excludes
   `RIGHT_TRANSFER`, so the intersection silently dropped the bit the generation
   declared. The send was refused with no indication that the manifest had asked
   for something the placement threw away.

2. **`is_transferable` gated the send path by *kind*, and directories were not
   in it.** The narrow set was justified — only a loan names its own recipient —
   but a directory qualifies for the same reason from the other direction: it
   carries its own scope and rights, and the root narrows both on derivation, so
   what arrives grants what the sender held and no more.

3. **A gate control had gone stale.** `check-sel4-gate-controls.py` mutated the
   channel plane's blessed layout by replacing the literal `slots=2` — and that
   plane's layout has since grown to three slots, so the mutation was a no-op
   and the control silently stopped controlling. It now derives the mutation
   from the fixture. A control that cannot fail is worse than no control,
   because it reads as coverage.

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| The service answers from its own authority | at least four transfers each way must be observed | "the client's directory view did not reach the service" |
| An interrupted transition loses the root | the `interrupted transition preserved root` arm | marker missing |
| A committed root is not visible | write then read-back through the service | `root transition committed` missing |
| A derived view escapes its subtree | the `scoped boundary enforced` arm | marker missing |
| Two clients race on one namespace | exactly one `done` | "N clients completed the scenario" |
| The service scribbles on the GPT | compared byte for byte | "modified the GPT or protective MBR" |
| The write arms do nothing | the image must differ overall | "no snapshot was committed" |
| A stale mutation makes a control vacuous | the layout mutation is derived from the fixture | the control fails when `check_shape` weakens |
| The gate loses evidence | `just sel4_gate_control_check`, pinned at 11 markers | a mutated transcript is accepted |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just sel4_filesystem_check` | Pass; 11 markers, 10 capability transfers | Direct |
| `just sel4_directory_check` | Pass; the mechanism underneath still holds | Direct |
| `just sel4_gate_control_check` | Pass; 22 gates reject 918 mutated transcripts and layouts | Direct |
| `just sel4_root_boot_check` | Pass; CSlot base repinned 856 → 860 | Direct |
| `just sel4_boot_layout_check` | Pass; 19 plane layouts match their fixtures | Direct |
| The other twenty seL4 plane gates | Pass | Direct |
| `just contracts_check`, `just generation_check`, `just test_sel4_root` | Pass | Direct |
| `just lint_all`, `just fmt_check_all`, `just ruff`, `just typos`, `just devlog_check` | Pass | Direct |
| M6.3 against the oracle's `just directory_check` markers | Not compared line by line — see below | — |

## Decisions

- **Decision:** Reuse `directory-probe` unmodified rather than write a seL4
  client.
  **Rationale:** it is the strongest available evidence that the service half is
  portable. A client written for this plane could accidentally encode the
  service's quirks; the oracle's cannot, because it predates it.

- **Decision:** Give the service its own store rather than a store endpoint.
  **Rationale:** `StoreTransact` stays unmediated by design (P5.4.2c). The
  service links `boot_contracts::object_store` and drives sectors, exactly as
  `sel4-store-probe` does.

- **Decision:** Seed the namespace root in the root task, hardcoded.
  **Rationale:** the oracle does the same in `bootstrap::directory_fixture_root`,
  and for the same reason — resolving a snapshot means reading the store, which
  is userspace's. The identity matches the oracle's byte for byte, because both
  come from `build-directory-fixture.py`.

- **Decision:** Tolerate the root-launched copy's nonzero exit.
  **Rationale:** `directory-probe` is shared with the oracle and carries no seL4
  authority probe. Adding one would modify the component whose being unmodified
  is the point. The gate scopes its lifecycle assertion to the composition, as
  the full-graph plane does.

## Open risks and follow-ups

- [ ] The gate asserts this plane's markers, not the oracle's
      `just directory_check` marker set. The two clients are the same binary, so
      the arms are the same, but nobody has diffed the two gates' expectations.
- [ ] `MAX_NAMESPACES` is still 1, and the service serves one client. A second
      client sharing the namespace would exercise the compare-and-swap under
      contention, which the mechanism supports and nothing tests.
- [ ] M6.6's powerbox now has the transfer kind it needed. It still needs a
      chooser holding authority the requester lacks, plus `contracts/powerbox/v1`.
- [ ] M6.4 (dango) additionally needs `InputRead`, still unmediated.
- [ ] M6.7 is blocked on B29 — two QEMU virtio transports share one granule and
      the root maps a frame once, so only one device is brought up.

## Artifacts and provenance

- Gate output, the observed capability transfers, and the image comparison:
  [`filesystem-check.txt`](filesystem-check.txt).
- The mechanism this runs on:
  [`devlog/2026-08-08-p5-4-3-directory-plane/`](../2026-08-08-p5-4-3-directory-plane/index.md).
- The object store it reads through:
  [`devlog/2026-08-08-p5-4-2c-object-store/`](../2026-08-08-p5-4-2c-object-store/index.md).
- B29, which blocks M6.7: [`roadmap/00-backlog.md`](../../roadmap/00-backlog.md).
- Related roadmap item: P5.4.3 in
  [`roadmap/07-architecture-portability.md`](../../roadmap/07-architecture-portability.md).
