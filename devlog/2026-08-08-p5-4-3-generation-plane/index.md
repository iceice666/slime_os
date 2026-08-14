# P5.4.3 — M6.5's generation commands, in userspace

| Field | Value |
|---|---|
| Date | 2026-08-08 |
| Kind | Change |
| Status | Verified |
| Scope | `components/bins/src/bin/{sel4-generation-manager,sel4-generation-client,init}.rs`, `components/bins/{Cargo.toml,build.rs}`, `components/bins/src/default_boot_layout.rs`, `contracts/generation/v1/fixtures/sel4-generation.zti`, `scripts/build/{boot_layout,build-generation,build-sel4}.py`, `scripts/check/check-sel4-{generation-plane,boot-layout,gate-controls}.py`, `Justfile` |
| Roadmap | P5.4.3, P5.4, M6.5 |
| Gates | `just sel4_generation_check`, `just sel4_rollback_check`, `just sel4_store_check`, `just sel4_boot_layout_check`, `just sel4_gate_control_check`, `just test_sel4_root`, `just contracts_check`, `just generation_check`, `just lint_all`, `just fmt_check_all`, `just ruff`, `just typos`, `just devlog_check` |
| Trigger | P5.4.2 complete; two scouts ranked M6.5 closest to the proven store/rollback pattern |
| Baseline | `GenerationTransact` was `Mediation::Unavailable`; no seL4 plane had a privileged service |

## Summary

Two components and one channel. The manager holds the plane's only block
capability and is therefore the only thing that can touch BootState; the client
holds one RPC endpoint and nothing else. It drives all five M6.5 operations —
list, inspect, stage, select, rollback — plus every refusal, then tries a direct
`BlockTransact` and is refused.

That last refusal is not a rights check. The client was spawned with exactly one
grant; **there is no slot it holds that names a device**. It knows the BootState
format perfectly well and still cannot write it, which is what M6.5's
"`BOOT_UPDATE` granted only to that service" means once authority is
capabilities rather than a flag.

The gate compares the disk image around the refused arms, because "fail before
BootState changes" is a claim about bytes. Every manager marker carries the
state it left behind, so a refusal that advanced the sequence is caught even
though the client only saw a status code.

This is the first seL4 plane with a *privileged service* rather than a probe,
and it found two defects that only that shape exposes.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `sel4-generation-manager.rs` | The service: five operations over BootState, older-slot-first | M6.5 policy runs above the root |
| `sel4-generation-client.rs` | The unprivileged client, including the direct-device refusal | Authority is what you hold, not what you know |
| `init.rs` | `drive_generation_plane`: mint, spawn both, drop init's copies | A composed channel, not a granted device |
| generation 27, `SEL4_GENERATION_LAYOUT`, build wiring | The plane's artifact | The gate boots what it asserts about |

### Defect 1 — an authority probe that ate a request

Every plane so far distinguishes the spawned instance from the root-launched one
with a probing `recv`: `ERR_BAD_CAP` means no capability, anything else means
the grant is there. That is correct for a *run token*, a slot nobody sends on.

The manager's slot is a live endpoint whose peer sends real requests. Its probe
dequeued the client's `LIST`, then the serve loop waited for a request that had
already been consumed, and the client waited for a reply that would never come.
Both parked; the plane hung with no error.

Fixed by making the probe keep what it takes: `Probe::{Absent, Empty, Request}`,
and the serve loop answers a carried request before dequeuing anything new — so
the client's order is preserved. The lesson is narrow and worth stating: *a
probing receive on a slot whose peer sends traffic is not the same operation as
a probing receive on a run token.*

### Defect 2 — `peer_alive` is about who still names the queue

The manager's loop ends on `ERR_PEER_DEAD`, which never arrived: init still held
its own copies of both queue ends, so the client's end kept a live namer after
the client exited.

Since B25 a spawn grant is a *copy*, so init dropping its ends removes only
init's name for them. The fix is one `cap_drop` pair after both spawns — but the
reason matters more than the fix, because `drive_sample_plane` deliberately does
*not* drop, and both are right: that plane's children never observe peer death.

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| The client reaches the device directly | it holds no device slot, and tries anyway | "direct device access accepted" |
| A refusal silently commits | every manager marker carries its BootState; refusals must not advance the sequence | sequence mismatch in the pinned marker |
| A refusal is honoured in the log but not on disk | the image is compared before and after | "the generation service modified …" |
| The run committed nothing at all | at least one BootState slot must have changed | "no BootState slot changed" |
| The service writes outside BootState | GPT, store region, and the tail are compared byte for byte | "the generation service modified …" |
| Promotion confirms the wrong generation | `select` naming the known-good must be refused | "wrong select accepted" |
| Staging something outside the closure succeeds | only the candidate may be staged | "unknown stage accepted" |
| An empty rollback reports success | it must answer `NO_PENDING` | "empty rollback accepted" |
| The manager hands a client authority | a reply carrying a capability is a failure | "the manager attached a capability" |
| The probe eats a request again | the manager's loop ends only on peer death, so a swallowed request hangs the gate at its timeout | boot timeout |
| The gate loses evidence | `just sel4_gate_control_check`, pinned at 20 markers | a mutated transcript is accepted |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just sel4_generation_check` | Pass; 20 markers, 4 refusals at unchanged sequences, 4 strict commits | Direct |
| `just sel4_rollback_check`, `just sel4_store_check` | Pass; the structures it shares a partition with are intact | Direct |
| `just sel4_gate_control_check` | Pass; 20 gates reject mutated transcripts and layouts | Direct |
| `just sel4_boot_layout_check` | Pass; 17 plane layouts match their fixtures | Direct |
| The other nineteen seL4 plane gates | Pass | Direct |
| `just test_sel4_root`, `just contracts_check`, `just generation_check` | Pass | Direct |
| `just lint_all`, `just fmt_check_all`, `just ruff`, `just typos`, `just devlog_check` | Pass | Direct |
| M6.5's model-trace validation against M5.6a/M5.6b | Not covered — see below | — |

## Decisions

- **Decision:** One client, not two.
  **Rationale:** two clients racing on one BootState makes the sequence
  nondeterministic and proves nothing the split does not already show. The
  authority claim is about what a client *holds*, and one client holding nothing
  demonstrates it exactly as well as two.

- **Decision:** Leave `GenerationTransact` `Mediation::Unavailable`.
  **Rationale:** same reasoning as `StoreTransact`. The operation names policy —
  which generations exist, what may be staged, when a release advances — and
  putting it in the root would undo the split this plane exists to demonstrate.
  The seL4 port has no generation syscall; it has a component you ask.

- **Decision:** Check refusals against the disk, not only the status.
  **Rationale:** "fail before BootState changes" is a claim about bytes. A
  service that reported a refusal after committing passes every marker.

- **Decision:** Reuse the store fixture and the rollback plane's slot layout.
  **Rationale:** four planes now share one on-disk layout, which is the layout
  being real rather than four fixtures drifting apart.

## Open risks and follow-ups

- [ ] M6.5 requires select/rollback traces to be *validated against the M5.6a
      and M5.6b models*. This plane asserts the transitions happened and that
      refusals did not commit; it does not emit a durable transition trace for
      `bootstate_trace_check` to consume. That is the same gap M5.6c has on
      seL4.
- [ ] Closure and release validation is a two-identity allowlist, not a real
      closure walk. The oracle validates executable closure and release
      continuity before staging; here "outside the closure" means "not the
      candidate".
- [ ] The remaining M6 gaps are M6.3 (directory), M6.4 (dango), M6.6
      (powerbox), and M6.7 (transfer). M6.3's three operations are genuine root
      *mechanism* — a shared namespace root with scoped views and an atomic
      compare-and-swap commit (`kernel/src/capability/mod.rs:193-263`) — so it
      needs a `Resource::Directory` in `slime-root`, not just a component.
      M6.4 and M6.6 depend on it; M6.7 is closest, and its "leave every
      ungranted device byte-identical" arm now has the two-disk harness the
      recovery plane built.

## Artifacts and provenance

- Gate output, the full transcript, and the image comparison:
  [`generation-check.txt`](generation-check.txt).
- The BootState transition model it drives:
  [`devlog/2026-08-08-p5-4-2c-rollback-plane/`](../2026-08-08-p5-4-2c-rollback-plane/index.md).
- The two-disk containment harness the follow-up would reuse:
  [`devlog/2026-08-08-p5-4-2c-recovery-plane/`](../2026-08-08-p5-4-2c-recovery-plane/index.md).
- Related roadmap item: P5.4.3 in
  [`roadmap/07-architecture-portability.md`](../../roadmap/07-architecture-portability.md).
