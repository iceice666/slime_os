# P5.4.3 — input mediation, and four defects a console session exposed

| Field | Value |
|---|---|
| Date | 2026-08-08 |
| Kind | Change |
| Status | Verified |
| Scope | `slime-root/src/{main,graph,channel,parked,ipc}.rs`, `components/bins/src/bin/{init,sel4-filesystem-service}.rs`, `components/bins/{Cargo.toml,build.rs}`, `contracts/generation/v1/fixtures/sel4-dango.zti`, `scripts/build/{boot_layout,build-generation,build-sel4}.py`, `scripts/check/check-sel4-{component-graph,root-boot,gate-controls}.py`, `roadmap/00-backlog.md` |
| Roadmap | P5.4.3, P5.4, M6.4 |
| Gates | `just sel4_input_check`, all 23 seL4 plane gates, `just test_sel4_root`, `just lint_all`, `just fmt_check_all`, `just ruff`, `just typos`, `just devlog_check`, `just contracts_check` |
| Trigger | M6.3 closed; M6.4 was the next gap and `InputRead` was unmediated |
| Baseline | `InputRead` answered `Mediation::Unavailable`; no seL4 plane read a key |

## Summary

`InputRead` is now mediated, and the unmediated surface is **four** operations —
down from nine when P5.4 began. The three that remain besides
`RecoveryReconstruct` are unmediated *by design*, because each names policy that
now runs in userspace.

Building the M6.4 dango plane on top of it surfaced four defects, three of them
in code that had been passing gates for weeks. The plane itself is **not
complete**: Dango boots, reads its scripted session, and resolves commands, but
no launch reaches the spawn service. That is recorded as B30 rather than
claimed.

`just sel4_input_check` gates the mechanism independently of M6.4: generation
31, one component, and three arms — the script decoded in order, an exhausted
script ending its reader, and a refusal from a slot holding no input capability.
The middle arm is the one that would have caught defect 1 below.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `graph.rs`, `main.rs`, `ipc.rs` | `Resource::Input`, `RIGHT_INPUT_READ`, `serve_input_read` | A component reads keys only through a granted capability |
| `main.rs` | `ScriptedInput` with per-task cursors | Each session has its own script |
| `channel.rs` | `WaitTarget::Input`, always ready | A reader waiting on input is not parked forever |
| `parked.rs` | A free list for saved reply CSlots | A long session does not exhaust the CSpace |
| `main.rs` | Both placement paths share one order | A component's slots do not depend on how it was started |
| `main.rs` | `RIGHT_TRANSFER` masked out of the placement gate | Transferability is not a resource kind |
| `build.rs` | `SLIME_COMMAND_PROFILE_MANIFEST` | A plane's command profile comes from its own generation |

### The four defects

**1. `WAIT_KIND_INPUT` resolved to `WaitTarget::Unmediated`, which is never
ready.** A component waiting on input would have parked forever. It had been
that way since the wait set was written, and no plane had ever waited on input
to notice. Fixed to always-ready: `InputRead` answers immediately, with an event
or with `WouldBlock`.

**2. Saved reply CSlots were never reused.** `parked.rs` reserves a slot from the
object allocator's cursor for every parkable call and `delete_slot`s it on
completion — which empties the slot but does not return the *index*, because the
allocator is monotonic. Right for objects that live for the boot; wrong for a
reply that lives for one call. A Dango session makes one parkable call per
keystroke and produced **1220 consecutive** `reply authority unavailable`
refusals before the graph wedged. Fixed with a free list.

**3. The two placement paths disagreed on order.** `launch_component_graph`
placed declared authority block-then-directory; `construct_child` placed it
directory-then-block. Both number the same component's table — one for the
instance the root launches, the other for a child a parent spawns — so the
filesystem service found its device at slot 1 when launched and slot 2 when
spawned. One list, one order, and a comment on each saying so.

**4. `RIGHT_TRANSFER` was being read as a resource kind.** The directory
placement mask includes it so a declared `transferable = true` survives the
intersection — but the *gate* was `authority.rights & right != 0`, so any
component with a transferable grant of any kind was handed a namespace view it
never declared. The loan plane's console, whose only grants are `bufferCreate`
and two `recv`, was getting a directory. Fixed by masking `RIGHT_TRANSFER` out
of the gate while leaving it in the placed rights.

Defects 3 and 4 were both caught by *unrelated* gates — the filesystem plane and
the loan plane — which is the argument for running the whole suite after a root
change rather than the plane under work.

### What the key encoding taught

Two mistakes in one small function, each producing a different lie. Encoding a
printable byte as code 9 made every keystroke arrive as a *space*, because 9 is
`Space` and characters are `0x100 | ch`. Omitting bit 32 made every event a key
*release*, which `dango.rs` discards — so the session consumed its whole script
and typed nothing, looking exactly like a keyboard that was not connected.

Both were read out of the decoder afterwards. The encoder and decoder are in
different crates with no shared constant, which is the actual defect; the
numbering should come from `contracts/` like every other wire format.

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A component reads keys without a capability | the `ungranted slot refused` arm | "an ungranted slot answered" |
| A key decodes wrongly | the script is compared byte by byte, including `pressed` | "the decoded key is not the scripted byte" |
| A reader parks forever on input | `WaitTarget::Input` is always ready | a hung plane |
| A long session exhausts the CSpace | the reply free list | `reply authority unavailable` |
| The placement paths drift apart again | `sel4_filesystem_check` boots both copies of one component | `[filesystem] fail: store open` |
| Transferability grants a resource | `sel4_loan_check` | `[console] shared-buffer denied` |
| The unmediated surface changes silently | `sel4_component_graph_check` pins it at four | the surface check fails |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just sel4_input_check` | Pass; 8 markers | Direct |
| All 23 seL4 plane gates | Pass | Direct |
| `just sel4_gate_control_check` | Pass; 23 gates reject 939 mutated transcripts and layouts | Direct |
| `just sel4_boot_layout_check` | Pass; 20 plane layouts match, re-blessed for the new order | Direct |
| `just test_sel4_root`, `just contracts_check` | Pass | Direct |
| `just lint_all`, `just fmt_check_all`, `just ruff`, `just typos`, `just devlog_check` | Pass | Direct |
| The dango plane reaching its markers | **Fails** — B30 | Direct |

## Decisions

- **Decision:** A scripted key source rather than a real keyboard.
  **Rationale:** the pinned QEMU profile has none, and the oracle does exactly
  this — `bootstrap` installs a per-generation script. It is honest about what
  is proved: the authority path and the event encoding, not a PS/2 decoder.

- **Decision:** Per-task cursors.
  **Rationale:** the root launches an unconfigured copy of every component, and
  a shared cursor let that copy drain the script before the spawned session
  asked. The session then parked on an exhausted source, which read as a hung
  component.

- **Decision:** An exhausted script yields `Escape` forever.
  **Rationale:** `dango.rs` loops on `WouldBlock` with a wait this source always
  satisfies, so a spent script would spin until the iteration budget died.
  Escape is the session's own quit key, so the reader ends the way the scripted
  `\x1b` ends the configured one.

- **Decision:** Record B30 rather than keep debugging.
  **Rationale:** the remaining failure is in the spawn-request path between two
  unmodified oracle components, and it needs a trace rather than another guess.
  Four root defects are fixed and gated; shipping those is worth more than
  holding them behind an incomplete plane.

## Open risks and follow-ups

- [ ] The key encoding is duplicated between `slime-root` and
      `components/runtime` with no shared constant. It should be a Zutai
      contract like every other wire format; two mistakes in one function is the
      argument.
- [ ] `MAX_GRAPH_ITERATIONS` is now 32768, raised while chasing the dango
      livelock. The livelock was the real cause and is fixed; the bound should
      be re-measured downward once B30 closes.
- [ ] B30: the dango plane resolves commands but launches none.

## Artifacts and provenance

- B30 and B29 in [`roadmap/00-backlog.md`](../../roadmap/00-backlog.md).
- The plane whose gate caught the placement drift:
  [`devlog/2026-08-08-p5-4-3-filesystem-plane/`](../2026-08-08-p5-4-3-filesystem-plane/index.md).
- The directory mechanism this builds beside:
  [`devlog/2026-08-08-p5-4-3-directory-plane/`](../2026-08-08-p5-4-3-directory-plane/index.md).
- Related roadmap item: P5.4.3 in
  [`roadmap/07-architecture-portability.md`](../../roadmap/07-architecture-portability.md).
