# B41 prerequisite — the dango plane's declarations

| Field | Value |
|---|---|
| Date | 2026-08-10 |
| Kind | Defect |
| Status | Verified |
| Scope | `contracts/generation/v1/fixtures/sel4-dango.zti`, `components/bins/build.rs`, `components/bins/src/bin/init.rs`, `slime-root/src/main.rs`, `scripts/check/check-sel4-dango-plane.py` |
| Roadmap | B41 |
| Gates | `just sel4_dango_check`, `just sel4_component_graph_check`, `just sel4_boot_check` |
| Trigger | B41's exit condition names `just sel4_dango_check`, which failed before any B41 work — the image did not build. |
| Baseline | `components/bins/build.rs:272` aborted on `expect("command RPC binding")`. Confirmed inherited by running the gate with `valid.zti` restored from `3228eb6`: identical failure. |

## Summary

B41 cannot be judged by a gate that was already red, so this entry is the
prerequisite: getting `sel4_dango_check` to exercise the plane it names. The
fixture was never migrated to the declaration model B39 established, and the
build script derived one component's slot layout from another's. The plane now
runs end to end for the first launch — shell prompt, profile resolution, spawn,
structured exit, `[init] dango plane complete`, every component exiting 0 —
and fails on a spawn-protocol defect in the composed second launch, which is
a different problem in a different layer.

## Observable symptom

- Command: `just sel4_dango_check`
- Expected: the four-line scripted session completes.
- Observed, at session start: `seL4 image build failed with exit status 1`,
  `thread 'main' panicked at components/bins/build.rs:272:6: command RPC binding`.
- Observed now: pass. "a scripted console session resolved two commands
  through the generation's profile and launched both through the spawn service
  with explicit environment, working directory, and stdin; an undeclared
  command was denied at resolution and a malformed line was a parse error".

## Investigation log

| Step | Observation | Consequence |
|---|---|---|
| 1 | Gate fails identically with `valid.zti` from `3228eb6` | Inherited, not caused by the v5 cutover |
| 2 | `sel4-dango.zti` declares a `commandProfile` for `dango` but no send/recv edge to `spawn-service` | The RPC the profile can only be served over was never declared |
| 3 | Generated `command_profile.rs` held `RPC_SLOT=5`, `SHARED_BUFFER_FACTORY_SLOT=4` — dango's slots, not the spawn service's | `build.rs` derived the consumer's layout from the client |
| 4 | `SLIME_GRAPH endpoint minted task=0 key=2` alongside `channel grant=dango-e-spawn-service-rpc key=0` | Root pre-created a channel whose ends shadowed init's minted halves |
| 5 | `spawn preflight instance=dango requested=1 bindings=5 minted=1` | Self-loop grants were counted as the parent's to pass |
| 6 | `spawn refused task=2 slot=4` after the count matched | Executable slots resolved in init's CSpace, not the spawn service's |
| 7 | `[sysinfo] spawned through profile` precedes `spawn-request:accepted` | The gate ordered a race |
| 8 | Composed launch refused with both caps transferred (`caps=2`) | Not a transfer failure |
| 9 | Bisected `valid_request` by instrumenting each predicate: `reject: cap zero` | One forwarded capability arrived as slot 0 |
| 10 | `main.rs:4102` allocated the receive slot with `free_slot_from(0)`; every other runtime allocation uses `free_slot_from(1)` | Root cause |

## Root cause

Five distinct declaration defects, each masking the next:

1. **The RPC edge was undeclared.** `dango` names a `commandProfile`, which it
   can only serve over a channel to `spawn-service`. No such grant existed, so
   the build script that derives the profile aborted before the image linked.
2. **The consumer's layout came from the client.** `spawn-service.rs` is the
   only consumer of the generated `command_profile.rs` and resolves every slot
   in its own CSpace, but `build.rs` read the instance owning the profile.
   Those coincide only while launcher and client share one slot numbering.
3. **The channel was created twice.** Declared as a plain grant *and* minted by
   `init`, so the root's pre-created ends landed at the same declared slots and
   shadowed the halves each participant was actually handed. `spawn-service`
   then held a channel whose peer never died and never exited.
4. **Self-loop grants were charged to the parent.** A grant whose source and
   target are both the child declares authority the child holds in its own
   right; the root installs it. Preflight counted them anyway, so `dango` — for
   which `init` holds two of six capabilities — was unspawnable. The root also
   never installed them, which the count had been hiding.
5. **Per-launch capabilities were undeclared.** A spawned child receives a
   minted launch context, and the composed launch also forwards a derived
   working directory and a stdin endpoint.

And one defect underneath all of them, in the root rather than the fixture:

6. **A received capability could land in slot 0.** The receive path allocated
   the destination with `free_slot_from(0)`. That slot number is reported to
   the receiver, and every protocol carrying one reads 0 as "no capability" —
   `valid_request` requires each of the first `capability_count` entries of
   `received_caps` to be non-zero. A forwarded capability landing there was
   therefore invisible to the component it had just been given to. Every other
   runtime slot allocation in the root already searched from 1.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `sel4-dango.zti` | Declares the RPC edge, both minted halves, the per-launch context, the forwarded cwd and stdin; slots ordered by provenance | The fixture states what `init` actually hands each child |
| `sel4-dango.zti` | `sysinfo` and `echo-agent` owned by `spawn-service` | The instance that spawns a child owns it, which is what admits its exec bindings |
| `components/bins/build.rs` | Consumer identified by which instance runs `spawn-service`; executable slots and the RPC slot read from its bindings; minted bindings consulted alongside grant bindings | A component's compiled slots are its own |
| `slime-root/src/main.rs` | Self-loop grants excluded from the spawn count and installed by the root | A parent is charged only for what it holds |
| `slime-root/src/main.rs` | Received capabilities allocate from slot 1 | A capability handed to a component is visible to it |
| `check-sel4-dango-plane.py` | The child's marker is required for presence, not position | The gate asserts authority rather than scheduling |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A parent is charged for authority it cannot hold | `just sel4_dango_check` | `spawn preflight … reason=declared-count` |
| A declared channel shadows a minted one | `just sel4_dango_check` | a service that never observes peer death |
| A component's compiled slots drift from its declarations | `just sel4_dango_check` | `spawn refused … ungranted` |
| The self-loop exclusion over-reaches | `just sel4_component_graph_check` | that plane's own count mismatch |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just sel4_dango_check` | Pass | Direct |
| `just sel4_component_graph_check` | Pass | Direct |
| `just sel4_boot_check`, `just sel4_root_boot_check`, `just sel4_reclamation_check` | Pass | Direct |
| `just sel4_capability_layout_check` | Pass | Direct |
| `just contracts_check`, `just generation_check` | Pass | Direct |
| `just test_sel4_root` | 140/140 | Direct |
| `just lint_all`, `just fmt_check_all`, `just ruff`, `just typos` | Pass | Direct |
| `just sel4_call_check`, `just sel4_operation_check`, `just sel4_powerbox_check` | Fail identically with and without this work | Direct, both sides observed |

## Decisions

- **Decision:** the preflight count rule stays; the fixture is what changes.
  **Rationale:** two candidate relaxations were each refused by a sibling
  plane. Excluding pre-created channels breaks `sel4_component_graph_check`,
  whose `init` legitimately passes its own end of one; excluding executables
  breaks it the other way, since its `spawn-service` receives five. Only the
  self-loop exclusion is a property rather than a fixture accident.
  **Rejected alternative:** loosening the rule to whatever made dango pass,
  which would have removed the check that caught four of these five defects.

- **Decision:** the child's marker is unordered.
  **Rationale:** `spawn` starts the thread, then the service sends the launch
  context and only afterwards replies, so a child can reach its own marker
  while the requester is still parked. The previous order was inherited from
  the retired-kernel port and describes a race.
  **Rejected alternative:** making the service reply before sending context,
  which would change the protocol to satisfy a gate.

## Open risks and follow-ups

- [ ] `sel4_input_check` — B41's other named gate — exceeds its 180s bound
      without completing the plane. Not investigated.
- [ ] B41 itself is untouched: `DebugWrite` and `InputRead` are still labels on
      the universal root dispatcher.

## Artifacts and provenance

- Baseline comparison: gate re-run with `contracts/generation/v1/fixtures/valid.zti`
  restored from `3228eb6`, identical failure.
- Related roadmap item: `roadmap/00-backlog.md` B41.
