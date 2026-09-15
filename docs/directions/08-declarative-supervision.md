# 8. Declarative supervision and restart policy

| | |
| --- | --- |
| Route | lifecycle |
| Enables | "let it crash" component design; restart-budget policy as manifest data |

## Motivation

M6 already supplies supervision handles and structured outcomes, but the
general restart-policy semantics remain open. Explore Erlang-style restart
policy as manifest data: restart limits, backoff, and whether state is
`preserve` or `ephemeral` across restarts. "Let it crash" plus
capability re-grant — a component's recovery strategy is declared,
auditable, and rollbackable like every other policy, instead of being
hard-coded per service.

## Design sketch

Policy as manifest data, per component: max restarts within a window,
backoff curve, state disposition on restart, and escalation (what the
supervisor does when the budget is exhausted — report structured failed
status through the health service, or escalate to its own supervisor).
The generation's declared graph implies the supervision tree; the
horizon's supervision handle is the capability a supervisor holds over
its children.

Restart with fresh grants: a restarted component is re-spawned from its
immutable image with grants re-derived from the supervisor's own set —
never retained across the crash, so a crashed component's possibly
confused authority dies with it. This composes with
[entry 25](25-resource-accounts.md): restarts charge the supervisor's
account, bounding restart storms by quota as well as by policy count.

State disposition per binding: `ephemeral` state is fresh on restart,
`preserve` survives, and `snapshotBeforeUpgrade`/`discardOnRollback`
interact with restarts only at generation boundaries — the within-
generation semantics must not contradict M5.6b's cross-generation ones.

Restart storms versus health checks: rapid crash loops must surface as
unhealthy before consuming unbounded quota; the backoff and limit policy
is where this is expressed.

## Open questions

- Who holds the restart budget — the supervisor as policy executor, or
  the generation as policy data the supervisor cannot widen?
- Does a component's declared dependency graph order restarts
  (dependents restarted after dependencies)?
- Is escalation hierarchical (supervisor of supervisors) or flat
  (straight to the health service)?
- How are in-flight channel messages to a crashed component handled —
  failed, queued for the restart, or caller-visible errors?
