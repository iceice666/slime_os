# Init asks which handle a child declares, not how many

| Field | Value |
|---|---|
| Date | 2026-08-19 |
| Kind | Change |
| Status | Verified |
| Scope | `slime-root/src/ipc.rs`, `components/bins/src/bin/init.rs`, `Justfile` (B23 pin) |
| Roadmap | B70, CP2 |
| Gates | `just test_sel4_root`, `just sel4_stream_check`, `just sel4_visibility_check`, `just sel4_matrix_check`, `just sel4_boot_layout_check`, `just generation_check` |
| Trigger | The standing `FABRIC_MINTED_GRANTS` migration, the last live `fabric_profile` symbol in `init.rs` |
| Baseline | `5803bc7` — `init.rs` `include!`d `fabric_profile.rs` for a per-holder grant count |

## Summary

`init.rs` read one generated symbol, `FABRIC_MINTED_GRANTS`, a per-holder count
of the capabilities a child's owner must supply at spawn. Its two call sites
asked different questions of that one number, and neither needed it. The count
site duplicated a check the root already performs from the generation; the shape
site used the count as a proxy for whether a plane interposes a proxy. A sixth
`resolve_binding` axis, `owned-minted:<name>`, answers the real question by
name, and with both sites migrated `init.rs` `include!`s nothing at all — the
second of the four `build.rs`-private tables in B70's problem statement to leave
init entirely.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `slime-root/src/ipc.rs` | New `owned-minted:<name>` axis and `resolve_owned_minted_slot`, scoping `mintedBindings` by `owner` where the existing `minted:` arm scopes by `holder` | A component asks the generation which handles it must supply, instead of compiling a build-time answer |
| `components/bins/src/bin/init.rs` | Deleted the `profile` module, its `include!`, and `declared_minted_grants`; added `declares_minted` | No generated table is compiled into `init` |
| `components/bins/src/bin/init.rs` | Deleted the matrix plane's count assertion | One authority states the expected grant count, the one that enforces it |
| `components/bins/src/bin/init.rs` | Stream/visibility shape selection branches on `declares_minted(b"fabric-intruder-supervision")` | The branch tests the fact it depends on |
| `Justfile` | B23 pin 130 → 131 | Test-count drift stays visible |

Two comments were corrected as stale rather than as part of the mechanism:
`init.rs:447` described an order as derived by `FABRIC_SUPERVISION`, deleted in
an earlier session, and the `declares_minted` doc header initially described a
`component` parameter the final signature does not take.

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| The query answers "no" where a plane does interpose | `just sel4_visibility_check` | `SLIME_ROOT FATAL … required instance init exit status=1` |
| The query answers "yes" where a plane does not | `just sel4_stream_check` | same, on the stream plane |
| The two prefixes shadow each other in dispatch | `just test_sel4_root` | `owned_minted_names_are_their_own_namespace` fails |
| A drifted composition spawns mis-bound instead of refused | `just sel4_matrix_check` | root's `SLIME_GRAPH spawn preflight count …` line, then refusal |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just test_sel4_root` | 131/131 across 15 modules, after raising the pin from 130 | Direct |
| `just generation_check` | Two isolated builds byte-identical; generation `bbd22ea7…` | Direct |
| `just sel4_stream_check` | Pass — 57 markers, 14 causal chains | Direct |
| `just sel4_visibility_check` | Pass — 26 markers, 12 view records | Direct |
| `just sel4_matrix_check` | Pass, including the unsatisfiable admission arm | Direct |
| `just sel4_boot_layout_check` | 25 plane layouts match their fixtures, unchanged | Direct |
| `just fmt_check_all`, `just lint_all` | Clean | Direct |
| **Perturbation:** `declares_minted` forced to `false` | `just sel4_visibility_check` **fails** | Direct |
| **Perturbation:** `declares_minted` forced to `true` | `just sel4_stream_check` **fails** | Direct |
| Restore from backup, re-run stream | Pass | Direct |

Both perturbations were run because a passing gate proves nothing on its own.
They fail in opposite directions, which is what establishes that the query's
answer is load-bearing on both branches rather than that one branch happens to
be tolerated.

## Decisions

- **Decision:** delete the matrix plane's count assertion rather than migrate it.
- **Rationale:** `preflight_spawn_grants` (`slime-root/src/main.rs:3559`) derives
  `parent_supplied + minted_count` at runtime from the generation and refuses a
  mismatch with a diagnostic naming both operands. That is the same quantity
  `declared_spawn_grant_counts` computes at build time, so init's check was a
  second copy of it. `spawn_boot_with` fails closed on any spawn error, and no
  script under `scripts/` asserts either init error string, so no gate depended
  on where the refusal was stated.
- **Rejected alternative:** keeping the check via the new axis. That would
  reproduce the `FABRIC_INTERPOSITIONS` shape — an assertion whose two operands
  both derive from one manifest, which can only confirm the table agrees with
  itself.

- **Decision:** a distinct `owned-minted:` prefix rather than relaxing the
  `minted:` filter to match either end.
- **Rationale:** one minted record names two instances, and which end asks
  changes the answer. A single name answering both would let an owner-scoped
  lookup satisfy a holder-scoped one, the shadowing the `executable:` fix exists
  to prevent. The host test pins the dispatch property directly: neither prefix
  is a prefix of the other.
- **Rejected alternative:** a new syscall label. The axis needed no ABI change,
  matching how the interposition-hop work also turned out not to need one.

- **Decision:** branch on a name, not on a count.
- **Rationale:** read from the fixtures, `fabric-service`'s minted bindings in
  ascending slot order are publisher(7), subscriber(8), publisher-b(9),
  subscriber-b(10) on `sel4-stream`, and publisher(7), subscriber(8),
  **intruder(9)**, publisher-b(10), subscriber-b(11) on `sel4-visibility`. The
  6-vs-5 count encodes exactly one boolean — whether
  `fabric-intruder-supervision` is declared — and encodes it lossily: `sel4-qos`
  and `sel4-stream` both total 5 through different compositions, `qos` declaring
  `fabric-service-shared-buffer-factory` at slot 1 where `stream` does not.
- **Rejected alternative:** none seriously; the count was a summary of the
  composition where init needed a statement about it.

## Open risks and follow-ups

- [ ] `build-generation.py:3767` still claims this table has "one derivation,
      both readers", naming the boot-layout resource as the second. That is
      wrong: `contracts/boot-layout/v1`'s `LayoutEntry` is
      `{name_identity, slot, role, rights}` with no count field, the counts land
      in the fabric-graph artifact's `mintedGrants` rows, and
      `boot-contracts/src/fabric_graph.rs` decodes no minted field at all. The
      rendered Rust constant was the only reader.
- [ ] `FABRIC_MINTED_GRANTS` is still generated and still rendered into
      `fabric_profile.rs`, now with no consumer. Deleting it — the generator
      expression, the emission, and the checked-in default rows — is the
      close-out, on the `FABRIC_INTERPOSITIONS` pattern.
- [ ] `FABRIC_TRACE_DEPTH` remains the highest site-closure-per-symbol move
      left, blocking four participant files through the two
      `const _: () = assert!(super::FABRIC_TRACE_DEPTH …)` guards in
      `components/bins/src/fabric_occupancy_trace.rs:75-76`.

## Artifacts and provenance

- Focused report: this entry.
- Raw transcript: none retained; every gate result above was read from the
  command's own output in-session.
- Serial/debugger/model output: the two perturbation failures quoted verbatim
  under Verification.
- Related roadmap item: [B70](../../roadmap/00-backlog.md#b70--component-definitions-and-slotroute-bindings-are-compile-time-coupled-to-one-crates-private-manifest-parser-blocking-out-of-tree-components)
