# Negative controls are required for product judges, not for verification tooling

**Status:** Accepted
**Decided by:** the project's human owner, 2026-10-17
**Amends:** the negative-control rule in [`AGENTS.md`](../../AGENTS.md)
("Verification code discipline") and
[`.agents/skills/verification-checkers/SKILL.md`](../../.agents/skills/verification-checkers/SKILL.md)

## Context

Until now the rule was blanket: "every positive assertion has a negative
control". It has been applied to two kinds of assertion. Some say something
about Slime OS itself, for example that a plane emits its marker chain, that a
judge refuses a malformed frame, or that a builder output is deterministic.
Others say something about the repository's verification and process tooling,
for example that the devloop adapter reports its diagnostics, that the closeout
publisher refuses a stale checkout, or that a work-item retirement script
refuses a bad remote.

The second kind has grown fastest. On main at `c98759b8`, nine exam and
control modules held 7,895 lines, up from 3,600 on 2026-10-01. They more than
doubled in sixteen days, faster than any other Python category.
Each of those controls tests a tool that itself tests something, and the rule
gave no place to stop: a control suite is an assertion too.

## Decision

A negative control is **required** for every positive assertion a
**product judge** makes. A product judge is a checker whose assertion is about
Slime OS behavior or about a format, wire, or boundary Slime OS owns or reads:
QEMU and physical-machine gates, independent wire and capture judges, contract
and system-spec checkers, builder determinism checks, and the gate-control
suite (`scripts/check/check-sel4-gate-controls.py`) that supplies the controls
for the plane gates.

A negative control is **not required** when the assertion is about the
repository's own verification or process tooling: the devloop and MyQue
adapters, work-item closeout, retirement, and publication automation, CI
selection logic, and any other test of a checker rather than of the product.
Adding such a control is a new exam. Under [`HUMAN.md`](../../HUMAN.md)
("Process") that is a human decision, not something a work item adds by
default.

Mixed modules are split by assertion, not by file. For example,
`scripts/lib/console_record_controls.py` holds a production route parser that
the IO-network plane imports as well as transcript mutations. The parser is
part of a product judge and stays.

## Alternatives

- **Keep the blanket rule.** Every assertion stays proven, but meta-tests keep
  growing with no stopping point, and every process change has to carry its
  own controls.
- **Drop negative controls everywhere.** This saves the most lines, but a
  product gate with no control can pass on a quiet transcript. That is the
  exact failure the rule exists to prevent.

## Consequences

- Existing tooling-only controls can be removed. Each removal is a grader
  change, so it lands with a work item. Acceptances of done items stay as
  history; open items whose acceptances bind a removed control are re-planned
  first.
- A product judge's controls are unchanged. Consolidating them, for example by
  parameterizing the gate-control mutations, must keep every control that
  protects a product assertion.
- Review findings that ask for a control on verification tooling are follow-up
  proposals, not blocking findings.

## Revisit when

A tooling defect without a control lets a false product claim pass. Examples
are a gate adapter reporting `passed` for a failed recipe, or a closeout
publishing an item whose evidence was stale. The tool that failed then gets a
control, and this record should name it.
