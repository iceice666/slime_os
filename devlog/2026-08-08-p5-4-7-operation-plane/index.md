# P5.4.7 — C8.7 bounded native operations on seL4

| Field | Value |
|---|---|
| Date | 2026-08-08 |
| Kind | Change |
| Status | Verified |
| Scope | `contracts/generation/v1/fixtures/sel4-operation.{zti,md}`, `contracts/boot-layout/v1/fixtures/sel4-operation.layout`, `scripts/build/{boot_layout,build-generation,build-sel4}.py`, `scripts/check/check-sel4-{operation-plane,boot-layout,gate-controls}.py`, `components/bins/build.rs`, `components/bins/src/bin/{init,fabric-op-time}.rs`, `components/bins/src/fabric_operation_scenario.rs`, `Justfile` |
| Roadmap | P5.4.7, P5.4, C8.7 |
| Gates | `just sel4_operation_check`, `just sel4_boot_layout_check`, `just sel4_gate_control_check`, `just test_sel4_root`, `just lint_all`, `just fmt_check_all`, `just ruff`, `just typos`, `just devlog_check` |
| Trigger | P5.4.1 recorded C8.7 as uncovered on seL4; P5.4.6 closed the composition it depends on |
| Baseline | Eleven seL4 plane gates, none asserting an operation property; C8.7 proven only by the frozen x86 oracle |

## Summary

C8.7's native-operation surface now has an observed seL4 equivalent. A twelfth
image, `sel4-operation`, boots generation 20: the `navigation` operation route
with two clients, a supervised replacement for the second, a server, and a
capability-routed clock, plus client A's private `nav-backup` route. The broker
and all five participants are the oracle's binaries **unmodified** — the
generation sets the oracle's own `SLIME_FABRIC_OPERATION_CHECK` — and only
`init`'s composition is seL4's. `just sel4_operation_check` asserts 53 markers
across twelve causal chains and is registered in the negative control at a pinned
53.

No new root mechanism was needed. C8.7 is userspace composition over primitives
`slime-root` already answers; the slice is fixture, layout, build variant,
composition, and gate.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `contracts/generation/v1/fixtures/sel4-operation.zti` | Generation 20: seven components, thirteen grants, the two-route operation graph with `inFlightOperations = 4` and `inFlightCalls = 0` | A seL4 plane declares its own graph rather than inheriting the x86 one |
| `contracts/generation/v1/fixtures/sel4-operation.md` | Records only what differs from `sel4-call.md`: the sixth executable, the non-transferable fabric handle, the ceilings | The fixture's non-obvious choices are reviewable rather than reverse-engineered |
| `scripts/build/boot_layout.py` | `SEL4_OPERATION_LAYOUT`, eight rows, registered as the generation-20 replacement | B10: the table the generation declares is the table the root fills |
| `scripts/build/build-generation.py` | `sel4-operation` in `SEL4_MANIFESTS`; its flag row sets `SLIME_SEL4_OPERATION_CHECK` **and** `SLIME_FABRIC_OPERATION_CHECK`; scrub/forward for the new flag | The broker and participants stay byte-identical with the x86 plane |
| `scripts/build/build-sel4.py` | `--operation-plane`, variant `operation`, `root-operation` target dir, `slime-sel4-operation.elf` | Each gate boots the artifact it asserts about |
| `components/bins/build.rs` | Tracks and forwards `SLIME_SEL4_OPERATION_CHECK` | `option_env!` resolves the plane at compile time |
| `components/bins/src/bin/init.rs` | `drive_operation_plane`; the oracle branch now requires the seL4 flag to be absent | Generation 20 cannot walk generation 15's layout |
| `components/bins/src/bin/fabric-op-time.rs`, `fabric_operation_scenario.rs` | Authority probes: `ERR_BAD_CAP` on the phase/barrier slot parks instead of failing | The root-launched unconfigured copy is not the plane's subject |
| `scripts/check/check-sel4-operation-plane.py`, `Justfile` | The gate and `just sel4_operation_check` | C8.7 has a standing seL4 assertion |
| `scripts/check/check-sel4-{boot-layout,gate-controls}.py` | Plane registered in both registries | The new gate is itself guarded |

### The composition

`init` mints five authenticated control pairs — one per participant plus one for
the replacement — keeps the participant half of each, spawns the fabric with the
service halves in `FABRIC_OPERATION_CONTROL_GRANTS` order, then transfers each
participant's supervision handle to the broker over that participant's own
channel. The parent vouches for every identity the broker admits; no participant
ever holds authority naming itself.

This is `drive_call_plane`'s shape, and deliberately so: B25 established that a
spawn grant is a non-consuming copy, which is what lets `init` retain the
introduction side of a channel it also granted.

Two things are specific to this plane:

- **The replacement is a declared identity.** C8.7's restart arm needs the broker
  to admit a *fresh* participant on a channel the dead one never held, while
  keeping the authenticated client index, correlation high-water mark, and
  retained results. So `fabric-op-client-b-restart` is its own component, its own
  route participant, and its own control grant, and `init` vouches for it exactly
  as for the others.
- **A release barrier orders the restart.** The replacement is spawned early, so
  the broker has a channel to park on, but blocks on a private barrier `init`
  sends only after the whole graph exists. Without it the replacement's role
  request could reach the broker before the original client B produced the
  retained result the replacement is supposed to find.

### Why the fabric's handle is not transferable here

The call plane declares `init-fabric-service` `transferable = true` because its
client and server must name the fabric as a shared-payload loan receiver. No
operation record is loaned — every C8.7 message is an inline
`WireOperationEnvelope` at `MAX_MSG` — so nothing here needs the fabric's handle,
and the executable declares `transferable = false`. The four handles that *are*
delegated are the participants' own.

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A C8.7 arm silently stops running | the twelve causal chains in `just sel4_operation_check` | the exact missing or out-of-order marker |
| The replacement is admitted more than once | `[fabric] operation participant restarted` is counted, not matched | "the replacement was provisioned N times, expected 1" |
| A participant never runs, or exits dirty | lifecycle check derived from the root's own spawn records | per-component exit-status mismatch |
| A participant vouches for itself | the introduction count | "parent delivered N supervision introductions, expected 4" |
| The gate itself loses evidence | `just sel4_gate_control_check`, pinned at 53 markers | a mutated transcript is accepted, or the count drifts |
| The declared layout and the filled table diverge | `just sel4_boot_layout_check` | the frozen fixture stops matching the observed `[layout]` block |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just sel4_operation_check` | Pass; 53 markers across 12 causal chains; six spawned tasks and init exited cleanly | Direct |
| `just sel4_gate_control_check` | Pass; 13 gates reject 610 mutated transcripts and layouts | Direct |
| `just sel4_boot_layout_check` | Pass; 10 plane layouts match their fixtures | Direct |
| `just test_sel4_root` | Pass; 113/113 across 13 modules | Direct |
| `just sel4_root_boot_check` | Pass | Direct |
| `just sel4_component_graph_check` | Pass | Direct |
| `just sel4_channel_check` | Pass | Direct |
| `just sel4_loan_check` | Pass | Direct |
| `just sel4_spawn_check` | Pass | Direct |
| `just sel4_sample_check` | Pass | Direct |
| `just sel4_supervision_check` | Pass | Direct |
| `just sel4_crossing_check` | Pass | Direct |
| `just sel4_stream_check` | Pass | Direct |
| `just sel4_qos_check` | Pass | Direct |
| `just sel4_call_check` | Pass | Direct |
| `just lint_all`, `just fmt_check_all`, `just ruff`, `just typos`, `just devlog_check` | Pass | Direct |
| C8.7 semantics themselves | Inherited from `just fabric_operation_check` on the frozen oracle | Inherited |

Every seL4 plane gate was re-run, not only the new one: this slice edits
`build-generation.py`'s shared flag table and `boot_layout.py`'s shared registry.

The last row is deliberate. This gate does not re-derive C8.7 — the markers it
matches are emitted by the oracle's own broker and participants, compiled from
the same sources. What it establishes is that those semantics hold on
`slime-root`, under a composition the seL4 capability model forces to differ.

## Decisions

- **Decision:** Set both `SLIME_SEL4_OPERATION_CHECK` and the oracle's
  `SLIME_FABRIC_OPERATION_CHECK` from this generation.
  **Rationale:** the seL4 flag selects `init`'s composition; the oracle flag
  selects the broker and participants, keeping them byte-identical across the two
  planes — which is the property the gate exists to demonstrate.
  **Rejected alternative:** a seL4-only flag with `||` selectors added to five
  components, which would fork the code the gate is supposed to be about.

- **Decision:** Declare the restart replacement in the graph rather than
  re-spawning client B's executable.
  **Rationale:** the broker admits it on an authenticated control the dead
  participant never held, so the restarted identity is parent-vouched rather than
  inherited. It is also what the oracle graph declares.
  **Rejected alternative:** reusing client B's control channel, which would make
  "the same participant" and "a new participant with the same index"
  indistinguishable to the broker.

- **Decision:** Probe slot authority to tell the spawned participant from the
  root-launched unconfigured copy.
  **Rationale:** the seL4 root launches every component the generation declares,
  so two tasks run the same image from the same generation. Neither an env flag
  nor the manifest-derived layout distinguishes them; the capability does. This
  is P5.4.6's `fabric-call-time` precedent applied to `fabric-op-time` and the
  replacement's barrier.
  **Rejected alternative:** budgeting one expected failure per component in the
  gate, which would make a real failure indistinguishable from the benign one.

- **Decision:** Split the peer-death chain in two.
  **Rationale:** both clients observe the same server death, and their relative
  order is scheduling rather than contract. What is causal is that each client's
  own active operation settles. One chain asserting a fixed order between them
  would be pinning a race.
  **Rejected alternative:** an unordered marker set, which would stop catching
  the orderings that *are* causal within each client.

## Open risks and follow-ups

- [ ] The `nav-backup` route proves an unrelated *operation* route stays live
      through peer death. Unrelated **stream** and **call** routes surviving the
      same event — the rest of C8.7's fourth required check — needs a graph
      carrying all three planes at once. That is C8.10's shape, and
      `just data_fabric_profile_check` already owns the declarative half on x86.
- [ ] `MAX_GRAPH_ITERATIONS` is 2048 (B28). This plane fits, but no gate reports
      how close it came, so the next longer plane may hit it the same way the QoS
      plane did.

## Artifacts and provenance

- Direct gate evidence: [`operation-check.txt`](operation-check.txt), captured
  from `just sel4_operation_check` on 2026-08-08 with the pinned qemu-arm-virt
  profile.
- Control and layout evidence, including the stale `sel4-call.layout` rows this
  slice's bless corrected: [`gate-controls.txt`](gate-controls.txt).
- The composition this one reuses:
  [`devlog/2026-08-08-b25-endpoint-copy-call-plane/`](../2026-08-08-b25-endpoint-copy-call-plane/index.md).
- The inventory that recorded C8.7 as uncovered:
  [`devlog/2026-08-07-p5-4-1-oracle-inventory/`](../2026-08-07-p5-4-1-oracle-inventory/index.md).
- Related roadmap item: P5.4.7 in
  [`roadmap/07-architecture-portability.md`](../../roadmap/07-architecture-portability.md).
