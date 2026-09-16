# Spec-driven work-item bodies and external contract ownership

**Status:** Accepted
**Related work item:** `01a0a546-55e5-734d-ac6b-6a8659cf3363`

## Context

The work-item store is MyQue's, and Slime OS consumes it. Requirements,
verification evidence, and GitHub projection had no owner of comparable
clarity: requirements were prose a reader interpreted, evidence was whatever a
pull request claimed, and any structured format risked being invented here and
then maintained here forever.

Three external projects now publish those contracts and are pinned in the dev
shell: MyQue `v0.2.0.0` (commit `d25241fcbf1d6b1e06283717c246576e88f6fa5d`)
with the `work-item/v2` open-record envelope, its machine API, and the
terminal-record retirement lifecycle; devloop `v0.1.0` (commit
`c78fcf345de424469196297d2be7b479fbb31a71`) with the requirements body schema,
its semantic helpers, and the evidence contracts; and myque-gh `v0.2.0.0`
(commit `376fe90742c11bc0a60236ad327a769dac2b9e13`) for projection. Zutai stays
the `deps/zutai` submodule, whose revision equals devloop's own compiler pin.

The question this record settles is what Slime OS owns once those exist.

## Decision

Slime OS owns four things and nothing more.

1. **The body convention.** A spec-driven item's body after the title is one
   fenced `zti` `dev-spec/v1` block, devloop's `fenced-zti/v1` profile, with its
   consumer record under the `devloop` frontmatter namespace. Another consumer
   could choose differently without any MyQue change.
2. **Gate policy.** `.devloop/policy.json` resolves gate identities to existing
   `just` targets through `scripts/check/devloop-gate.py` and declares the typed
   observations each gate produces. Requirement text never carries a command.
3. **Approval rules.** `scripts/check/devloop-approval.py` applies this
   repository's own preconditions — the item is already on canonical `main`, and
   backlog defects come first — and says nothing about whether an acceptance was
   observed.
4. **Which items are subject.** `just tasks_check` decides that from the
   presence of a `devloop` record and reports what devloop refused.

Everything else is deferred to the owning project. No MyQue or devloop schema is
copied under `contracts/`; the envelope, the requirements schema, the helper
semantics, and the evidence rules are read from the pinned releases.

## Alternatives and trade-offs

- **Keep prose requirements and PR-claimed evidence:** no new dependency, but
  completion stays a judgement call and a regression can be closed by assertion.
- **Implement the requirements format in this repository:** removes an external
  dependency, but Slime OS would then own a schema, a validator, and an evidence
  model that have nothing to do with an operating system.
- **Vendor devloop's schemas under `contracts/`:** makes gates self-contained,
  but forks the format at the first upgrade, which is exactly the failure the
  external split exists to prevent.

The pinned-release split keeps the semantics in one place and keeps this
repository's checks honest about where their authority comes from.

## Consequences

- A spec-driven item cannot be completed without evidence bound to the exact
  requirements, helpers, code, policy, execution inputs, target, image, and run
  epoch that were tested; the newest entry for an obligation decides it.
- A human obligation stays pending until an operator supplies a real
  observation. Nothing in this repository may synthesize one.
- `just tasks_check` requires the pinned devloop whenever an item carries a
  `devloop` record, exactly as it already requires `myque`. Each validation
  compiles and links a native binary, so it costs seconds, not milliseconds;
  there is no cached fast path.
- A direct `myque close` without `--expected` bypasses the eligibility
  boundary. That is MyQue's documented limit; this repository does not claim
  every closure is evidence-enforced.
- Retired items keep resolving offline from `.tasks/terminal/<UUID>.json`.
  `refs/myque/retained/*` needs an explicit refspec to travel between clones,
  and removing an active item file does not shrink Git history.
- Ordinary prose items, including every legacy `work-item/v1` item, remain valid
  and unaffected.

## Revisit conditions

Revisit if an external release stops publishing a contract this repository
depends on, if devloop's compiler pin and `deps/zutai` cannot be kept equal, if
native validation cost makes `just tasks_check` impractical as a routine gate,
or if a second consumer in this repository needs a different body profile.

## References

- [`development-record-ownership.md`](development-record-ownership.md) — the
  record split this one extends to requirements and evidence.
- `AGENTS.md` — work-item, backlog, and gate rules.
- `CONTRIBUTING.md` — the contributor workflow for spec-driven items.
- `.devloop/policy.json` — gate identities and their declared observations.
- `scripts/lib/devloop.py` — the toolchain adapter and the compiler-pin
  assertion.
- <https://github.com/mozufu/myque>, <https://github.com/mozufu/devloop>,
  <https://github.com/mozufu/myque-gh> — the owning projects.
