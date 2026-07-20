# 5. TPM-bound boot state and attestation

| | |
| --- | --- |
| Status | parked |
| Route | hardware; secondary: updates |
| Depends on | M5.6 BootState (complete); M7-class physical hardware work (TPM driver is not currently in any milestone scope) |
| Enables | rollback resurrection resistance on physical media; remote attestation of running generation identity |
| Now | Paper only: NVRAM layout, counter/sealed-data design, and the stage-0 verification flow are designable today; the TPM driver is unscoped. |

## Motivation

Generations are content-addressed; the Framework target has a TPM. The
BootState attempt counter and known-good identity currently live on
disk, so an attacker who reimages the disk to an older state also rolls
back the rollback protection — a known-bad generation becomes bootable
again. Sealing the attempt counter and known-good hash in TPM NVRAM
makes the disk alone insufficient: a rolled-back image cannot resurrect
what the TPM has forgotten. The same binding exposes remote
attestation: verifiable evidence that "this machine runs generation
hash X".

## What exists today

- M5.6 (complete) defines the semantics to protect: attempt counters
  consumed durably before transfer, known-good identity updated only on
  health confirmation, and M5.6a's checked invariants
  (`SelectableBootRootExists`, `PendingAttemptConsumedBeforeTransfer`).
- Stage-0 (M5.5, complete) already hash-verifies the generation before
  transfer — the TPM check slots into the same pre-transfer gate.
- Missing: the TPM driver itself; the register notes it is not in any
  milestone scope. M5.9 (not started) covers the adjacent case of
  BootState reconstruction when slots are unusable, and must define how
  reconstruction interacts with sealed counters.

## Design sketch

The TPM holds the monotonic facts the disk cannot be trusted to keep:
the attempt counter (or its epoch) and the known-good generation hash.
Stage-0's flow gains a verification step: the on-disk BootState must
agree with the TPM-held values — a disk image rolled back to before a
known-bad promotion fails because the TPM's view has moved on.

The failure matrix needs the same rigor M5.6a gave the disk-only
transitions: TPM unavailable, NVRAM cleared (owner change), disk and
TPM out of sync in each direction. The invariant from M5.6a —
always leave a selectable boot root — must survive the binding: a
cleared TPM must not brick a healthy disk. Fail-closed against
resurrection, fail-open only through the explicit M5.9 recovery path.

Attestation is the read direction: the TPM quotes the bound generation
identity so a remote verifier learns what the machine runs. Scope is
limited to boot state; general measured boot is a larger design this
entry does not claim.

## Open questions

- Counter placement: TPM monotonic counter versus NVRAM-sealed value —
  which survives which failure modes (NVRAM clearing, counter
  exhaustion)?
- Disk/TPM desync policy per direction: disk newer than TPM ( TPM
  reset) must route to M5.9 recovery; TPM newer than disk is the
  resurrection case and fails.
- What exactly is attested — generation hash only, or also BootState
  health history?
- Does the QEMU verification story gain a virtual TPM, or does this
  stay Framework-only (breaking the everything-QEMU-verified
  convention)?

## Exit-condition sketch

On the Framework target, reflashing an older generation image fails
stage-0 verification against TPM-held counters.

## Probe guidance

Paper: the NVRAM/counter layout, the four-cell disk×TPM desync matrix
with per-cell policy, and the stage-0 flow amendment — checked against
M5.6a's invariants so the binding cannot violate
`SelectableBootRootExists`. The note also scopes whether a virtual TPM
keeps QEMU as the verification platform.
