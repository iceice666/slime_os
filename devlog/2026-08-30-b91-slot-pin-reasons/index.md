# B91: 611 pinned slots, four reasons, and the label that was false for 185 of them

| Field | Value |
|---|---|
| Date | 2026-08-30 |
| Kind | Change |
| Status | Verified |
| Scope | `contracts/generation-manifest/v1/{schema.zt,README.md,fixtures/valid.zti,compositions/*.zti}`, `contracts/system-spec/v1/{schema.zt,systems/reference.zti}`, `scripts/build/build-generation.py`, `scripts/lib/system_spec.py`, `scripts/check/{check-slot-pin-reasons.py,check-system-spec.py}`, `just/contracts.just` |
| Roadmap | B91 |
| Gates | `just contracts_check`, `just system_spec_check`, `just ruff` |
| Trigger | B91's inventory: 616 of 679 instance binding slots explicitly pinned, with no machine-readable distinction between the reasons |
| Baseline | `devlog/2026-08-28-automatic-binding-slots/` migrated thirteen name-resolved declarations across four compositions and left the classification unbuilt |

## Summary

B91 asked for one of two outcomes per pinned capability slot: remove it under a
plane gate, or retain it with a *documented external positional invariant*. This
entry closes it on the second branch for 611 pins and the first branch for five,
after establishing that the first branch was nearly exhausted. `InstanceBinding`
and `SlotPin` gain a required-when-pinned `slotReason`, closed to four values;
`scripts/check/check-slot-pin-reasons.py` re-derives every label from the manifest
rather than trusting it, and refuses a `componentAbi` pin whose holder resolves
that grant by name and compiles no such slot number. Every resolved slot in the
corpus is unchanged — 1084 slots, identical SHA-256 — and `generation.bin` is
byte-identical for both compositions whose sources lost a pin.

The vocabulary took three attempts, and the two discarded ones are the substance
of this entry: each was a plausible partition that a measurement showed to be
false.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `contracts/generation-manifest/v1/schema.zt` | `InstanceBinding.slotReason?`, closed to `bootLayout`/`allocatorOrder`/`encodedLayout`/`componentAbi`, required exactly when `slot` is present | A pin states why its number is written down, in a form a gate can check |
| `scripts/build/build-generation.py` | `pin_removal_effects` measures each pin's removal through the production allocator; `declared_pin_effects` takes the strongest effect over the source and every boot profile; `boot_layout_row_bindings` reads the declared `bootstrapInstance`; `validate_slot_reasons` refuses any label it cannot confirm | The reason is a measured fact, not an assertion, and one label is true of every generation a source can produce |
| `scripts/build/build-generation.py::load_manifest` | Validates before `assign_declared_slots` | Every product build checks the labels, and `slot is None` still means the source omitted it |
| `scripts/check/check-slot-pin-reasons.py` (new) | Repo-wide host gate over 43 manifests and 4 boot profiles; totality, soundness, and a minimality clause auditing `componentAbi` against Rust and C component sources, including names reaching `resolve_binding` through a single-argument wrapper and through runtime-composed affixes | The one label resting on a claim about source is checked against that source |
| `just/contracts.just` | Registers the gate; corrects the stale `system_spec_check` mutation count from 17 to the observed 21 | The claim is gated rather than documented |
| `contracts/system-spec/v1/schema.zt`, `scripts/lib/system_spec.py` | `SlotPin.reason` required and propagated into the derived binding; vocabulary validated at derivation | The derived fixture carries the reason its reviewed source declared |
| `scripts/check/check-system-spec.py` | `slotReason` treated as a post-baseline binding field and asserted separately against the builder's predicate; one new mutation control | The frozen pre-CP1 baseline predates the field, so it is excused from the comparison and checked more strictly than the comparison would have been |
| 41 direct compositions + 2 derived manifests | 611 pins labelled: 152 `bootLayout`, 12 `allocatorOrder`, 187 `encodedLayout`, 260 `componentAbi` | The residue is visible and counted rather than hidden among undifferentiated numbers |
| `contracts/system-spec/v1/systems/reference.zti`, `contracts/generation-manifest/v1/compositions/sel4-io-network.zti` | Five pins removed: `spawn-service/spawn-service-{echo,sysinfo}`, and `io-link-loopback/network-service-link-device` plus `network-service/{network-intruder-service,network-service-link-device}` | Every pin the gate can prove both redundant and name-resolved is gone |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A pin is added with no reason, or a reason with no pin | `just contracts_check` | `pins slot N without a slotReason; expected <reason>` / `slotReason declared for an unpinned binding` |
| A pin claims a reason the manifest contradicts | `just contracts_check` | `declares slotReason 'X' but this manifest implies 'Y'` |
| A typo enters the vocabulary | `just contracts_check` | `unknown slotReason 'X'; expected one of …` |
| A label holds for the composition but not for a boot profile built from it | `just contracts_check` | Same message, qualified `<manifest>#<profile>` |
| A `componentAbi` pin's holder actually resolves by name | `just contracts_check` | `pins slot N as componentAbi, but <executable> resolves that grant by name and compiles no such slot number` |
| A holder's source becomes unreadable, silently voiding the minimality clause | `just contracts_check` | `no readable source implements 'X'`, naming the two allowlists |
| A system-spec pin declares a reason outside the vocabulary | `just system_spec_check` | `slotPins: <holder>/<grant>: unknown reason 'X'` |
| The gate script becomes invalid | `just ruff` | Ruff lint failure |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `python3 scripts/check/check-slot-pin-reasons.py` | Passed — 611 pinned bindings across 43 manifests and 4 boot profiles, 68 automatic; 152/12/187/260; 2 `dango` pins reported exempt | Direct |
| Resolved-slot table before/after, all 43 manifests | Identical: 1084 resolved slots, SHA-256 `95e360e7d32c4203a0956bead6816b7f75b5bde449be54adc14422af4420a92e` on both sides | Direct |
| `build-generation.py` for `sel4-loan` before/after | `generation.bin` SHA-256 `48501549f350ff05ac45c07cbd5cee95e7d03eaccad24d76d4e0a25a79b404e0` on both sides | Direct |
| `build-generation.py` for `sel4-io-network` before/after (the composition that lost a pin) | `generation.bin` SHA-256 `557179d50a2fd9c1ca1126e5dff6a40fa05dbd437f041cdeff22379fabfe70ad` on both sides | Direct |
| Six mutations of `sel4-loan.zti` | Each failed the gate: dropped label; `bootLayout`→`componentAbi`; →`encodedLayout`; →`allocatorOrder`; unknown value; label on an unpinned binding | Direct |
| `allocatorOrder`→`encodedLayout` on `valid.zti` `spawn-service/spawn-service-rpc` | Failed, naming both labels — the two measured classes are mutually discriminated, not merely ordered | Direct |
| Reinstating the removed `io-link-loopback` pin as `componentAbi` | Failed the minimality clause by name | Direct |
| `python3 scripts/check/check-system-spec.py` | Passed — 2 systems, 2 manifests derived semantically identical to their baselines, 21 named mutations refused (was 20) | Direct |
| `python3 scripts/generate/generate-generation-from-spec.py --check` | Both derived artifacts current | Direct |
| `zutai-cli check` on both edited schemas | Passed | Direct |
| `just ruff` | Passed | Direct |
| Independent reviewer oracle over all 43 manifests | 0 classification mismatches against each pin's measured removal effect | Direct |

## Decisions

- Decision: the vocabulary is four values, and three of them are measured from the
  manifest rather than asserted.
- Rationale: two earlier partitions were tried and disproved by measurement.
  (1) A three-value vocabulary defined `allocatorOrder` as "removing this pin
  would move some other resolved slot" but implemented it by comparing the whole
  resolved table *including the pin's own entry*. Measured: of 196 pins so
  labelled, only 11 moved another slot; 185 moved only themselves. Because the
  minimality clause skips every non-`componentAbi` label, those 185 were exempted
  from the residue check on a claim false for them, and 77 were already in exactly
  the state that clause exists to report. (2) Excluding the pin's own entry fixed
  that but pushed all 185 into `componentAbi`, which claims a positional consumer.
  Testing joint removal of the 77 flagged pins showed none were removable: their
  own encoded slots move, so the generation bytes and the installed capability
  layout change. They are neither load-bearing for neighbours nor positionally
  consumed, so they earned their own value, `encodedLayout`.
- Decision: a reason is taken over the source manifest *and* every boot profile it
  declares, strongest wins.
- Rationale: `resolve_boot_profile` drops instances and filters each survivor's
  bindings, which changes what the allocator would do. Measured on `valid.zti`'s
  `default` profile: two `powerbox-chooser` pins are redundant in the full
  composition and load-bearing in the narrowing. A per-binding field can honestly
  mean only something true of every generation the source can produce.
- Decision: `boot_layout_row_bindings` reads the declared `bootstrapInstance`.
- Rationale: an earlier draft inferred it from executable roles. The two agree
  across all 43 manifests today, which is precisely why the inference was
  dangerous — it was a second statement of a declared fact, the drift class B71
  deleted, and a divergence would have mislabelled every pin in that composition.
- Decision: the minimality clause fails closed on a holder whose source cannot be
  read, with two named allowlists rather than a silent default.
- Rationale: `evidence.get(name, empty)` made the clause vacuous for exactly the
  holders it could not inspect, which is indistinguishable from a pass. `slisp` is
  a real product component and its C sources are now scanned (`INPUT_SLOT`,
  `SPAWN_SERVICE_SLOT`); `dango` is a retired identity whose component spec
  declares no implementation, so its 2 pins are exempt and counted in the output.
- Rejected alternative: remove every pin the allocator reproduces. Allocator
  equality is necessary but not sufficient — it says nothing about a frozen
  artifact, an encoded layout, or a positional consumer, which is the same error
  the 2026-08-28 entry rejected and which the 185-pin measurement re-confirmed.
- Rejected alternative: generate the vocabulary as constants from Zutai. Every
  sibling closed Text set in this contract (`Object.kind`, `Executable.role`,
  `State.policy`, `NotificationBinding.role`) is `Text` in the schema, prose in
  the README, and a plain Python table in the builder. `slotReason` never reaches
  the v5 binary encoding, so it belongs in that category rather than with the
  numeric rights discriminants the root also decodes.

## Open risks and follow-ups

- [ ] `componentAbi` is 260 pins and is the only class a future migration can
      shrink. The minimality clause reaches 4 of them: 2 are exempt as sourceless,
      and 254 are held by components that never resolve that grant by name, where
      the clause has nothing to object to. It is a tripwire for pins that *become*
      migratable, not evidence the other 254 were audited — the gate's own
      docstring now says so, and the honest next step is a per-component pass
      confirming each positional consumer rather than another regex.
- [ ] `encodedLayout` is 187 pins. They cannot be dropped without changing the
      encoded layout, but a composition whose layout is not frozen by a `*.layout`
      fixture could in principle absorb the change under its own plane gate. That
      is a per-composition question this entry does not answer.
- [ ] `compiles_slot` matches the number anywhere in the holder executable's
      sources, not on the code path holding the binding. `io-driver-probe` runs as
      both `io-driver-supervisor` and `io-driver-worker` on different branches of
      `main`; the worker's `DEVICE_SLOT`/`REGION_SLOT` constants would suppress the
      clause for a supervisor-side pin, which resolves purely by name. All four
      currently suppressed pins are the worker's, so today's labels are right, but
      the predicate does not distinguish the roles.
- [ ] Neither the minimality clause nor the soundness predicate checks *which*
      number a pin carries — only that it is pinned and what the allocator would do
      without it. Permuting two positionally-consumed pins inside one holder
      therefore passes every host gate added here. It is caught by the owning
      plane's QEMU boot, since the component then binds the wrong capability, but
      no host gate names it.
- [ ] Minted and notification bindings remain fully explicit (83 and 322) and carry
      no reason. Their namespaces and positional consumers are separate, and B91
      scoped itself to instance bindings.
- [ ] The five removed pins are guarded by `just contracts_check`'s resolved-slot
      equality rather than by a new assertion in `io_network_check` and
      `system_spec_check`; the byte-identity evidence above is direct but was
      observed in this session rather than pinned into those gates.

## Artifacts and provenance

- Focused report: this entry.
- Raw transcript: none; every command result above was observed directly in the
  implementation session.
- Serial/debugger/model output: none. No QEMU plane gate was run — the change is
  confined to host-side manifest source, its builder validation, and host gates,
  and the generation bytes every plane boots are proven identical by SHA-256
  comparison rather than by re-observing a boot.
- Related roadmap item: [`roadmap/00-backlog.md`](../../roadmap/00-backlog.md) B91.
- Predecessor: [`devlog/2026-08-28-automatic-binding-slots/`](../2026-08-28-automatic-binding-slots/index.md).
