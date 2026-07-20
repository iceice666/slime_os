# 16. Powerbox UI

| | |
| --- | --- |
| Status | parked |
| Route | lifecycle |
| Depends on | M6 scope (stub) already lists a powerbox-style file dialog service; the capability-matrix horizon tracks the Directory-rights question |
| Enables | user-driven authority granting without ambient file access; the general pattern beyond M6's minimal dialog |
| Now | Pattern and interaction design is paper work; implementation waits on M6's dialog service and the Directory-rights decision. |

## Motivation

Applications never hold an ambient "open file" right; the file dialog is
a system component, and the user's selection gesture itself mints a
single-object capability. Authorization and intent are the same gesture:
the user cannot approve what they did not mean, and the application
cannot receive more than the selected object. This replaces the ambient
home-directory access conventional systems grant every application.

For agent components the pattern generalizes naturally: an agent
requesting access to an object it was not granted triggers the same
chooser, and the user's gesture is the audit record.

## What exists today

- The principle is already structural: spawn supplies no implicit
  environment, working directory, or streams, so components hold only
  manifest-declared grants (README's agentic direction).
- The capability-matrix horizon carries the open question this entry
  depends on: a Directory object kind with READ / WRITE / LIST rights,
  and whether powerbox minting needs more than `derive`.
- M6 (stub) lists a powerbox-style file dialog service as scope; this
  entry covers the general pattern beyond that minimum.
- No UI stack exists at any milestone yet; the chooser's rendering is
  downstream of daily-driver work. [INFERENCE: M7 covers Framework
  daily-driver bring-up; no display/compositor milestone exists.]

## Design sketch

The chooser is a system component holding directory authority the
requesting component lacks. Protocol: the requester opens a channel to
the chooser declaring what it wants (kind, rights, purpose string); the
chooser renders the selection; the user's gesture mints a single-object
capability — derived from the chooser's own grant, narrowed to exactly
the selected object and the declared rights — transferred back along the
channel. The requesting component ends up with a capability it could
not have obtained from the manifest or any peer.

The minted capability is a narrow-only derive, so the pattern fits the
rights algebra ([entry 24](24-rights-algebra-model.md)) without
amendment — unless the Directory question resolves otherwise, which is
why the horizon tracks it.

The general pattern beyond files: any authority whose granting requires
human intent (camera, network destination once
[entry 18](18-network-authority.md) exists) is a powerbox candidate.
The entry's design output should name what is common — the request
schema, the purpose string, the minted single-object grant — and what is
per-domain.

Audit: the gesture is a provenance event; combined with the M5.1
provenance follow-up, "why does this component have this file" has an
explicit answer rooted in a user gesture.

## Open questions

- The horizon's question: does powerbox minting need more than
  `derive` (e.g., minting a capability for an object the chooser can
  list but has not opened)?
- Persistence: is a powerbox grant per-session, or may the generation
  declare it persistent across reboots (and with what rollback
  semantics)?
- Can the chooser itself be interposed ([entry 7](07-schema-interposition.md))
  for dry-runs, or is user-intent minting excluded from membranes?
- Purpose strings: free text, or a declared vocabulary the manifest
  constrains?

## Exit-condition sketch

A component with no directory grants opens the chooser, the user selects
a file, and the component receives a single-object read capability it
could not have obtained otherwise.

## Probe guidance

Paper until M6: define the request/response schema between requester
and chooser, the minted grant's shape, and the per-domain
generalization list. The note feeds the M6 dialog design and the
horizon's Directory-rights decision simultaneously.
