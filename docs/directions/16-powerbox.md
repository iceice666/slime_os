# 16. Powerbox UI

| | |
| --- | --- |
| Status | parked |
| Route | lifecycle |
| Depends on | M6.3 Directory capabilities and M6.6 console powerbox (complete); [Hardware H8](../../roadmap/04-platform-hardware.md) supplies display/compositor/input for a graphical chooser |
| Enables | user-driven authority granting without ambient access; graphical and non-file generalizations beyond M6.6 |
| Now | M6.6 implements the bounded console file chooser and single-object grant transfer; graphical rendering and per-domain generalization remain open. |

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
- M6.3 defines Directory READ / WRITE / LIST rights and narrow-only
  derivation/transfer.
- M6.6 delivers a console chooser with a versioned request/reply contract,
  cancellation, provenance, and narrow-only single-object capability transfer.
  This entry covers graphical rendering through
  [Hardware H8](../../roadmap/04-platform-hardware.md) and the general pattern
  beyond files.

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

M6.6 establishes the file-domain protocol and grant shape. Further design
should generalize that proven pattern per domain and define the graphical
interaction on Hardware H8 without widening chooser authority.
