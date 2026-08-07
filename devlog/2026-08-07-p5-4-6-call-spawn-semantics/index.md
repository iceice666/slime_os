# P5.4.6 — the C8.6 call plane's real blocker is spawn-grant semantics, not slot numbering

| Field | Value |
|---|---|
| Date | 2026-08-07 |
| Kind | Defect |
| Status | Root-caused |
| Scope | `contracts/generation/v1/fixtures/sel4-call.zti`, `scripts/build/{boot_layout,build-generation}.py`, `components/bins/src/bin/{init,fabric-service}.rs`, `components/bins/build.rs`, `roadmap/00-backlog.md`, `roadmap/07-architecture-portability.md` |
| Roadmap | P5.4.6, B25, C8.6 |
| Gates | none |
| Trigger | Reopening P5.4.6 against the recorded `SlotCursors` diagnosis |
| Baseline | Nine passing seL4 plane gates; the call plane failing at `[fabric] fail: call role request`, recorded in the predecessor entry's [`boot.log`](../2026-08-07-p5-4-6-call-plane/boot.log) |

## Summary

The C8.6 call plane's recorded blocker — `SlotCursors::take`'s `used_slot_zero`
producing a discontiguous control-slot set — is a **consequence of the
fixture's shape**, not a defect in slot allocation, and removing it exposes the
real blocker. Having `init` mint the control pairs and hand them out at spawn
(as `drive_stream_plane` already does) makes the root's channel cursor
irrelevant to the fabric's numbering. The plane still does not pass, and now
fails for a reason that cannot be fixed in a fixture: **`slime-root` treats a
spawn-granted endpoint as a move while the frozen oracle treats it as a copy.**
The x86 call plane depends on the copy — `init` keeps every service half and
transfers each participant's supervision handle afterwards — so the composition
is not portable as written. The boot now reaches a clean deadlock with the
broker holding a role request it cannot answer. B25 has been rewritten from a
numbering claim to this semantic one.

## Observable symptom

- Command: `qemu-system-aarch64 -machine virt,virtualization=on -cpu cortex-a72
  -m 2048 -nographic -serial mon:stdio -kernel build/slime-sel4-call.elf`
- Expected: the C8.6 transcript — role provisioning, correlated replies,
  duplicate/stale rejection, seven distinct terminal events.
- Observed: generation 18 admitted; init's layout resolved exactly as declared
  (`[layout] 0 endpoint-factory` through `[layout] 6 executable
  fabric-call-time`); four control pairs minted; all five components spawned;
  each participant's role request delivered to the broker
  (`SLIME_GRAPH received task=4 channel=2`). Then everything parks. The graph
  ends `live=10` with `parked=8` and `transfers served=0` — no terminal
  markers, no `[fabric]` broker marker, and no progress.
- Exit/fault/serial evidence: [`boot.log`](boot.log). The one failure line,
  `[fabric-call] fail: time phase receive`, is the *root-launched* unconfigured
  instance of `fabric-call-time`, not the participant `init` spawned — the same
  duplicate-instance effect `check-sel4-stream-plane.py` budgets for.

## Investigation log

| Step | Observation | Consequence |
|---|---|---|
| 1 | The failing plane's fabric was **root-launched** (`SLIME_GRAPH staged task=4 component=fabric-service`), while the passing stream plane's is **init-spawned** (`SLIME_GRAPH spawned … component=fabric-service`) | The two planes differ in who provisions the broker, which is what decides where its slots come from — not in any cursor behaviour |
| 2 | Root-launched components take channel slots from `SlotCursors`, which resumes above the factory grants staging installed | The `[0, 3, 4, 5, 6]` set the predecessor entry recorded is a consequence of declaring controls as *generation* grants, not an independent defect |
| 3 | Removing those grants emptied `FABRIC_CALL_CLIENTS` | `_control_sources` (`build-generation.py:833`) derives the broker's caller-identity table from exactly those four grant *names*, so the grants are load-bearing as a naming source even when the endpoints are minted elsewhere. They were restored; only the root's use of them as channels had to go away, which minting achieves |
| 4 | Rebuilt with the pairs minted in `init` | `[layout] 0..6` matches `SEL4_CALL_LAYOUT` slot for slot, and the fabric's four controls arrive as `channel handed parent=5 child=6 … slot=2,3,4,5` — contiguous above its two factories |
| 5 | The broker receives a role request and then never answers it | `SLIME_GRAPH received task=4 channel=2` with no reply. `Broker::provision` reads a role request and then blocks in `consume_supervision` (`call_broker.rs:273-275`), which waits for a supervision handle that nothing on this plane sends |
| 6 | Read both spawn implementations | `distribute_channel_ends` (`slime-root/src/main.rs:3262,3299-3301`) reassigns the channel holder and calls `table.drop_slot`; the oracle's `preflight_spawn_grant` (`kernel/src/task/mod.rs:286`) derives a copy into a fresh vector and never touches the parent's table |
| 7 | Tried granting each participant a handle naming *itself*, so it could send its own | Not constructible: `serve_spawn` installs the supervision handle only into the **parent's** table, and only *after* `construct_child` has built the child's (`main.rs:3586-3603`). A child can never hold a handle naming itself, so the cycle cannot be cut from the component side. Reverted |

## Root cause

A spawn grant naming an `Endpoint` has different semantics in the two
implementations, and the C8.6 composition depends on the oracle's.

`slime-root/src/main.rs::distribute_channel_ends` moves: it calls
`channels.reassign(*channel, parent, child)` and then
`table.drop_slot(*granted_slot)` on the parent. The comment there states the
intent plainly — a minted pair "leaves the parent its other slot, because it
held both and gave one away". That is correct for the case it was written for,
where the parent keeps the *opposite* end. It is wrong for a parent that grants
one end and then needs to use *that same channel* again, because the parent now
holds neither end of it.

The retired kernel copies. `kernel/src/task/mod.rs::preflight_spawn_grant`
performs `cap.derive(grant.rights)` into a fresh vector, which
`spawn_with_caps_for` then installs into the child; neither reads or mutates the
parent's table. So `init.rs` can grant a service half at spawn and still
`cap_transfer` over it afterwards, which is exactly what `launch_fabric_calls`
does for all three participant supervision handles.

The invariant violated is portability of the component graph, which is P5.4's
whole premise: an unmodified component composition must mean the same thing on
both mechanisms. Here it does not, and the divergence is silent — every plane
that hands an end away and never touches it again behaves identically, which is
why nine gates pass over it.

The cycle is what makes this unfixable by reordering. A participant needs a
`RIGHT_SUPERVISE` handle naming the fabric, so the fabric must exist first; the
fabric needs a handle naming each participant, delivered over that
participant's authenticated control channel, so the participants must exist
first. The oracle cuts the cycle by letting init retain the service halves.
With a moving grant there is no end left to cut it with, and step 7 above shows
the obvious alternative — the participant sending its own handle — is not
constructible either, because no component ever holds a handle naming itself.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `scripts/build/boot_layout.py` | `SEL4_CALL_LAYOUT`, registered as the generation-18 replacement | Init's table is factories plus executables only; every control channel is minted at runtime |
| `sel4-call.zti` | Generation 18; the four control grants retained as the broker's naming source but no longer the source of init's endpoints; `transferable` set on `fabric-service` alone | The root no longer numbers the fabric's control ends |
| `init.rs` | `drive_call_plane`, dispatched on a new `SLIME_SEL4_CALL_CHECK` | Init mints the pairs and binds each to one component at spawn, so caller identity stays a capability fact |
| `fabric-service.rs` | Selects the call broker on either flag | The two planes share the broker, so they cannot diverge |
| `build-generation.py`, `build.rs` | Scrub, map, and export the new flag | The manifest being built decides the scenario, not the caller's shell |
| `roadmap/00-backlog.md` | B25 rewritten from a numbering claim to the spawn-semantics one | The filed defect names the mechanism that actually blocks |

## Regression guards

**None, and that is the finding rather than an omission.** The plane does not
pass, so a gate would be a red one.

It is worth stating precisely what is *not* guarded, because an earlier draft of
this entry claimed two guards that do not exist. `check-contracts.py` validates
`fixtures/valid.zti` and never reads `sel4-call.zti`; `check-boot-layout-
resource.py`'s `FIXTURE_PROFILES` contains no generation 18, so `SEL4_CALL_LAYOUT`
is never encoded or compared; and `check-sel4-boot-layout.py`'s `PLANES` omits
`sel4-call`, with no `sel4-call.layout` fixture on disk. All three pass before
and after this change for reasons unrelated to it.

| Risk | Guard | Failure signal |
|---|---|---|
| The reshaped fixture stops encoding | none — reached only through `build-sel4.py --call-plane` | The image build fails |
| The generation-18 layout drifts from what the root places | none until `sel4-call` joins `check-sel4-boot-layout.py`'s `PLANES` | — |
| The call plane silently runs a different broker | none — this is what the missing gate would catch | — |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `python3 scripts/build/build-sel4.py --call-plane --skip-pin-check` | Builds — `wrote build/slime-sel4-call.elf` | Direct |
| Boot under the pinned QEMU line | Generation 18 admitted, `[layout] 0..6` matching `SEL4_CALL_LAYOUT` slot for slot, 4 pairs minted, 5 components spawned, role request delivered, then deadlock | Direct — [`boot.log`](boot.log) |
| The nine existing seL4 plane gates | All pass, re-run after the change | Direct |
| `python3 scripts/check/check-contracts.py` | Passes — but does not read this fixture | Direct |
| `python3 scripts/check/check-boot-layout-resource.py` | Passes — 19 fixtures, 18 generation/profile pairs, **none of them generation 18**; unchanged by this diff | Direct |
| `just test_sel4_root` | 109/109 across 13 modules | Direct |
| `just fmt_check_all`, `just lint_all`, `just ruff`, `just typos`, `just devlog_check` | Pass | Direct |
| The fabric's own slot set `[0, 1, 2, 3, 4, 5]` | **[INFERENCE]** — the log shows the four controls at `slot=2,3,4,5`; slots 0 and 1 are the two factory grants at the head of `drive_call_plane`'s grant array. The root emits no per-slot dump for a spawned child | Inferred |
| The C8.6 transcript | **Not observed.** The plane deadlocks before it | Unobserved |
| `just fabric_call_check`, the oracle's own C8.6 gate | **Cannot run on this host** — needs x86 OVMF firmware, absent from this store. Confirmed pre-existing in the predecessor entry | Inherited |

## Decisions

- Decision: rewrite B25 rather than close it, and leave the plane failing.
- Rationale: the entry named the wrong mechanism, and a wrong defect record is
  worse than none — it sends the next author to `SlotCursors`, where they will
  find nothing wrong. The corrected entry names a semantic divergence with a
  concrete exit condition.

- Decision: keep the four control grants in the fixture even though init mints
  the endpoints.
- Rationale: `_control_sources` derives `FABRIC_CALL_CLIENTS` from those grant
  names, in the plane's declared order rather than the builder's sort, and that
  table is how the broker maps a control slot to a caller identity. Deleting
  them emptied it and tripped `request_response_controls`' four-control assert
  before the broker read a single slot. The grants name; the minted endpoints
  authorize.

- Decision: do not work around the divergence inside the components.
- Rationale: the one workaround that looked available — each participant sending
  a handle naming itself — is not constructible, because the root installs a
  supervision handle only into the parent's table and only after the child's is
  built. Anything else would make the plane assert about a composition the
  oracle does not have, which is what P5.4 exists to avoid.

- Rejected alternative: changing `distribute_channel_ends` to copy in passing.
  It is a load-bearing change to the path all nine passing planes take, it needs
  a holder model admitting two tasks per end (`ChannelTable` resolves queues by
  holder), and it deserves its own slice with its own fault injection rather
  than riding along inside a milestone.

## Open risks and follow-ups

- [ ] **B25 is now the blocker for P5.4.6** and must be resolved before the call
      plane can pass. It is a decision about the capability model — copy or move
      — not a bug fix, and it touches every plane.
- [ ] **The reshaped fixture is committed in a non-passing state**, as its
      predecessor was. The scaffolding is independently verified — it builds, it
      admits, the layout matches the root slot for slot, every channel is minted
      and bound, and the broker receives a role request — and the remaining
      failure is one named mechanism.
- [ ] **`sel4-call` is absent from `check-sel4-boot-layout.py`'s `PLANES`** and
      has no blessed `.layout` fixture. Adding both is part of closing P5.4.6,
      and until then the generation-18 table has no guard.
- [ ] **P5.4.6 is not closed.** C8.6's required checks — correlated replies,
      duplicate/stale rejection, one execution of a non-idempotent operation,
      seven distinct terminal events, reclamation on client or server death —
      are all unobserved on seL4.
- [ ] `just fabric_call_check` remains unrunnable on this host, so the oracle
      transcript this plane must reproduce cannot be read here either.

## Artifacts and provenance

- Focused report: this entry.
- Raw transcript: [`boot.log`](boot.log).
- Serial/debugger/model output: [`boot.log`](boot.log).
- Related roadmap item:
  [P5.4.6](../../roadmap/07-architecture-portability.md),
  [C8.6](../../roadmap/02-core-runtime.md),
  [B25](../../roadmap/00-backlog.md).
- Supersedes the diagnosis in
  [`devlog/2026-08-07-p5-4-6-call-plane/`](../2026-08-07-p5-4-6-call-plane/index.md),
  whose body and two corrections all name `SlotCursors`.
