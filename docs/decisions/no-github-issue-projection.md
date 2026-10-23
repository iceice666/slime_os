# Work items are not projected into GitHub Issues

**Status:** Accepted
**Decided by:** the project's human owner, 2026-10-23
**Related work items:** `01a1649c-2655-7430-a40d-a4a3220b6d6f` (this retirement),
`01a09a60-86bf-7646-b782-e23fc7230249` (the projection it retires)
**Amends:** the projection part of
[Spec-driven work-item bodies](spec-driven-work-item-bodies.md)

## Context

`.tasks/` is the canonical work-item store. `myque-project.yml` mirrored every
open item into a GitHub Issue and every milestone item into a native milestone
with `myque-gh apply`. It ran on every CI completion, on pull-request events and
every 30 minutes, with issue write permission.

Nothing in the repository read those Issues. devloop, `just tasks_check`, the
closeout workflow and the retirement workflow read `.tasks/` only. The owner
did not read them either.

On 2026-10-22 GitHub's incident "Errors loading Issues, Pull Requests, and
search results" made `GET /repos/{repo}/issues` return `200 OK` with an empty
array and no `Link` header on about half of all requests. `gh api --paginate`
read an empty page as the last page. myque-gh took the truncated list as
complete, decided that the missing items had no Issue, and created new ones.
A later rediscovery in the same run saw both copies and failed with
`duplicate projection identities`. Each trigger repeated this, and 468
duplicate Issues existed before the workflow was disabled by hand.

## Decision

Work items are not projected into GitHub Issues or milestones.

- `myque-project.yml` is deleted, and `.devloop/policy.json` no longer names it.
- A pull request names the item it implements by canonical UUID in its
  description. `myque-gh pr link` and its trailer are no longer used.
- GitHub Issues stay intake for bugs and proposals. An Issue never tracks a
  work item.
- The projected Issues were closed as `not_planned`, each with a comment that
  names the item's file under `.tasks/`. The duplicates lost their identity
  header.
- The myque-gh flake input and the `.#projection` nix shell stay. The closeout
  and retirement workflows run their Python scripts in that shell, and its
  name is part of the shared tool cache key.

## Alternatives

- **Fix myque-gh pagination.** List Issues through GraphQL, which returned
  complete results during the incident, or refuse to create an Issue when the
  REST list disagrees with GraphQL `totalCount`. This keeps a mirror nobody
  reads and a workflow with write access that a future GitHub fault can still
  misuse.
- **Keep the workflow disabled without deleting it.** This needs no change, but
  the guidance would still describe a projection that does not run.

## Consequences

- A GitHub outage no longer changes repository-visible state.
- There is no GitHub view of the item hierarchy or milestone progress.
  `just tasks_list`, `just tasks_graph` and `just devloop render <UUID>` render
  the store.
- Pull requests merged before this decision may still carry a
  `myque:implements` trailer. Nothing reads it.
- The 43 native milestones the projection created stay as they were and no
  longer change.

## Revisit when

- A consumer outside the repository needs work items in GitHub, and myque-gh
  refuses to create Issues from an incomplete listing.
