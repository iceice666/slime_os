# P5.4.3 — M6.4's Dango session, and slot layout as declared data

| Field | Value |
|---|---|
| Date | 2026-08-08 |
| Kind | Defect |
| Status | Verified |
| Scope | `slime-root/src/{main,graph}.rs`, `components/bins/src/bin/init.rs`, `contracts/generation/v1/fixtures/{sel4-dango,sel4-powerbox,sel4-filesystem}.zti`, `scripts/check/check-sel4-{dango-plane,boot-layout,gate-controls}.py`, `Justfile`, `roadmap/00-backlog.md` |
| Roadmap | P5.4.3, P5.4, M6.4 |
| Gates | `just sel4_dango_check`, all 25 seL4 plane gates, `just test_sel4_root`, `just lint_all`, `just fmt_check_all`, `just ruff`, `just typos`, `just devlog_check`, `just contracts_check` |
| Trigger | B30: the dango plane resolved commands but launched none |
| Baseline | Dango reached its prompt and read keys; every spawn request was refused |

## Summary

M6.4 is closed. A scripted console session resolves two commands through the
generation's profile and launches both through the spawn service — the second
carrying a derived working directory and a stdin endpoint — while an undeclared
command is denied at resolution and a malformed line is a parse error.

Every component is the oracle's, unmodified: `dango.rs`, `spawn-service.rs`,
`sysinfo.rs`, `echo-agent.rs`, `console.rs`.

B30 had three causes, and **none was the hypothesis recorded when it was
opened.** All three were in the root, and all three were about the same thing:
where a component's capabilities land.

## Observable symptom

Dango booted, reached `[dango] native runtime ready`, printed its prompt, read
its scripted keystrokes, and printed `resolved:profile` twice. No
`spawn-request:accepted` and no `result:exit:0` ever appeared. The spawn service
was up, its quota live, and it received the 64-byte requests — it simply refused
every one.

## Investigation log

1. Traced the request from `dango.rs::spawn` to the channel: two 64-byte sends
   on channel 1, both received by the spawn service. So the request arrived.
2. Found `SLIME_GRAPH spawn refused task=7 slot=1 ungranted` in the service's
   own output — it was refusing, not failing to receive.
3. Dumped the service's placement markers: slots 1 and 2 were empty. Its
   executables had never been installed.
4. Fixed that, and the *first* command completed. The second failed at
   `capability transfer refused task=8 caps=2`.
5. Dumped dango's placement: input at 4 where its constants say 2. The fixed
   kind order placed directory first, which suits `powerbox-chooser.rs` and not
   dango.
6. Moved both paths to manifest grant order — and found the encoded order is
   alphabetical by grant name, because `build-generation.py` sorts.
7. With the slots right, the remaining refusal was the endpoint: dango attaches
   a stdin end, and `is_transferable` refused endpoints by kind.

## Root cause

**1. Declared executables were never placed for a spawned child.**
`construct_child` installed the parent's grants and the child's declared
factories, but not the executables the generation said it may spawn. A spawned
`spawn-service` found slots 1 and 2 empty and refused every request with
`slot=1 ungranted`. This is P5.4.2c's defect exactly — a child not receiving its
own declared authority — in the one resource kind that slice did not cover.

**2. Declared authority was placed in a fixed kind order, and components
disagree about kinds.** `powerbox-chooser.rs` reads a directory at 1 and input
at 2; `dango.rs` reads input at 2 and a cwd root at 3. No single kind order
satisfies both, and each attempt to fix one broke the other — three boot
failures across two slices, each found by booting.

Both placement paths now walk the **generation's own grant order**, which is
what the oracle does. A component's slot layout *is* the order its generation
declares its grants, so two components can disagree about kinds without either
being wrong.

**3. `is_transferable` refused endpoints by kind**, so a shell could not give a
child its stdin. The comment defending that was wrong rather than merely narrow:
it claimed nothing bounds where an endpoint lands, but nothing bounds where a
*loan* lands either — the handle names its receiver and the send path checks it,
which is a check rather than a property of the kind. What bounds every move on
that path is the sender holding `RIGHT_TRANSFER`, and the oracle's `sys_send`
gates on exactly that bit with no kind predicate.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `main.rs` | `construct_child` places declared executables at `1..=n` above the grant list | A spawned child can spawn what its generation says it may |
| `main.rs` | Both placement paths walk manifest grant order | A component's layout is its generation's, not the root's |
| `main.rs` | `declared_resource`, `declared_role` | One rule, two callers |
| `graph.rs` | `is_transferable` admits endpoints | A shell can give a child its stdin |
| three fixtures | Grant names encode each component's expected slot order | The encoded order matches what components read |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A spawned child cannot spawn | `declared executable … slot=1` and the accepted requests | `spawn-request:accepted` missing |
| A denied command reaches the spawn service | exactly two resolutions and two acceptances | "N profile resolutions and M accepted requests" |
| A child gets ambient context | both children report the command and arguments they were given | "did not report the command it was launched with" |
| An undeclared command launches | `resolve-denied` before any request | marker missing |
| The placement order drifts again | `sel4_dango_check`, `sel4_powerbox_check`, `sel4_filesystem_check` together | a component reads the wrong capability |
| Endpoint transfer regresses | the `with-stdin` composition | `capability transfer refused` |
| The gate loses evidence | `just sel4_gate_control_check`, pinned at 14 markers | a mutated transcript is accepted |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just sel4_dango_check` | Pass; 14 markers, 2 resolutions, 2 accepted requests | Direct |
| `just sel4_gate_control_check` | Pass; 25 gates reject 988 mutated transcripts and layouts | Direct |
| `just sel4_boot_layout_check` | Pass; 22 plane layouts match their fixtures | Direct |
| The other twenty-four seL4 plane gates | Pass | Direct |
| `just test_sel4_root`, `just contracts_check` | Pass | Direct |
| `just lint_all`, `just fmt_check_all`, `just ruff`, `just typos`, `just devlog_check` | Pass | Direct |
| The oracle's `check-dango.py` frame-conservation and determinism checks | Not ported — see below | — |

## Decisions

- **Decision:** Place declared authority in manifest grant order rather than a
  fixed kind order.
  **Rationale:** it is what the oracle does, and it is the only rule that lets
  two components disagree about kinds without either being wrong. A fixed order
  is a global constraint on a local decision.

- **Decision:** Widen `is_transferable` rather than special-case stdin.
  **Rationale:** the kind gate was the wrong mechanism. `RIGHT_TRANSFER` already
  bounds the move, the generation already decides who holds it, and the oracle
  gates on nothing else.

- **Decision:** Keep executables above the parent's grant list rather than at
  `1..=n`.
  **Rationale:** tried the other way; every component compiles against a grant
  list starting at 0, so `1..=n` renumbers every plane. What makes the current
  order work is that a spawned component's grant list is fixed by its spawner,
  so `plan.count` is a constant its constants can be written against.

- **Decision:** Encode slot order in grant names for now.
  **Rationale:** it is the smallest change that makes the three planes correct,
  and the alternative — a declared per-component layout contract — is a slice of
  its own. Documented at the point of use rather than left for the next author
  to rediscover.

## Open risks and follow-ups

- [ ] **Slot order is encoded in grant names, which is a sharp edge.**
      `build-generation.py` sorts grants by `(name, source, target)` before encoding,
      so the manifest's *declaration* order is not what reaches the root — the encoded
      order is alphabetical by grant name.

      A component holding several kinds therefore fixes its slot layout by **naming
      its grants in the order it reads them**. `sel4-powerbox.zti` names
      `powerbox-a-root` and `powerbox-b-input` for exactly that reason.

      That works and it is documented at `declared_resource`, but it is not a good
      contract: a rename silently renumbers a component's capabilities, and nothing
      checks it until a plane boots wrong. The follow-up below is the real fix.
- [ ] **A non-bootstrap component's slot layout should be declared data.** Four
      defects across three slices came from the root and a component disagreeing
      about it, each found by booting. The boot layout already solves this for
      the bootstrap component — declared, fixture-checked, and verified by
      `just sel4_boot_layout_check`. Extending it to every component would turn
      this class of boot failure into a build failure, and would remove the
      grant-name ordering hack.
- [ ] The oracle's `check-dango.py` also asserts frame conservation and
      reproduces the session twice to prove determinism. Neither is ported; both
      are transcript-level checks this gate could adopt.
- [ ] `MAX_GRAPH_ITERATIONS` is 32768, raised while chasing the livelock that
      turned out to be the missing executables. It should be re-measured
      downward now that the plane completes.

## Artifacts and provenance

- Gate output, the placement markers, and the session transcript:
  [`dango-check.txt`](dango-check.txt).
- B30's resolved entry: [`roadmap/00-backlog.md`](../../roadmap/00-backlog.md).
- The input mechanism the session reads through, and the first two placement
  defects:
  [`devlog/2026-08-08-p5-4-3-input-mediation/`](../2026-08-08-p5-4-3-input-mediation/index.md).
- The third placement defect:
  [`devlog/2026-08-08-p5-4-3-powerbox-plane/`](../2026-08-08-p5-4-3-powerbox-plane/index.md).
- Related roadmap item: P5.4.3 in
  [`roadmap/07-architecture-portability.md`](../../roadmap/07-architecture-portability.md).
