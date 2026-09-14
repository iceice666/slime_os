# Physical boot attempt 5 — operator report

Immutable record of the fifth boot attempt on the named Framework: the first
whose panel record is fully legible. Frozen — corrections append.

## Reported observation

The operator reports the GRUB residue in the screen corners is gone.
Photograph: [`physical-attempt-5-panel.jpg`](physical-attempt-5-panel.jpg)
(SHA-256 `d3e34b99f0f15b179f2313ee4c4160348eee45878be0dd5bb2ffd8f62376b180`).

Transcribed from the photograph — fourteen lines, nothing else on screen:

```
SLIME OS - ROOT RUNNING
TIMER MAPPED - AWAITING TICK
TIMER TICKING
GENERATION ADMITTED
IMAGES STAGED
ENDPOINTS WIRED
COMPONENTS RUNNING
SERVING - AWAITING GRAPH
SLIME OS
TARGET X86_64-SEL4-FRAMEWORK13-AI300
GENERATION 1 ID 0F1929F39BF5FA1F
DISPLAY 2256X1504X32
ROOT READY
IDLE - SAFE TO POWER OFF
```

## What this settles

**The evidence channel now works as an evidence channel.** Attempt 4 ran the
same chain but rendered it over the bootloader's console text, so the field
that binds a boot to a generation could not be read. Independent inspection of
this photograph reports:

* The generation digest read character by character as
  `0 F 1 9 2 9 F 3 9 B F 5 F A 1 F` — sixteen hex characters, no ambiguous
  position. It matches the build manifest's `0f1929f39bf5fa1f` and the
  `generationIdentityPrefix` that `prepare` recorded for this exact image.
  On attempt 4 the same field read as `41?1?19329F39BF5FA1F`.
* `DISPLAY 2256X1504X32`, confirming the native mode again.
* **No** text anywhere in a smaller, lowercase, or proportional font — the
  corners, edges, and margins carry nothing. Attempt 4 had ``Booting `slime'``
  and `Trying to terminate EFI services again` overlapping the record.

So the full-screen clearing change is confirmed on hardware: each line clears
its whole screen rows, the first clears the top margin, and the terminal line
clears every row below the record.

## One transcription discrepancy, resolved

Vision inspection twice read line 5 as `TIMERS STAGED`, once with stated high
confidence after comparing letter shapes. The source emits exactly one such
marker and it is `IMAGES STAGED` (`slime-root/src/graph_runtime.rs`); no code
path produces `TIMERS`. The operator confirmed the panel reads `IMAGES
STAGED`. Recorded as a vision misreading — most likely primed by the two
`TIMER` lines immediately above — and not a panel defect. It is worth keeping
because it shows the limit of OCR as the verification method for this channel:
the photograph is the artifact, and a human reading of it outranks a model's.

## Binding state

`prepare` ran at 22:09 against this exact image, and every link now agrees:

| Field | Value |
|---|---|
| Image | `95a0d213c1249cc73e462d5bed9b9462bea577f63fcb33093c83e025b8a14303` |
| Media identity | `104b5959704a69ed496fe3037b78b2abb34addda3adc60eef043dc403684a20e` |
| USB read-back | identical to the image digest |
| Panel generation | `0F1929F39BF5FA1F`, matching the recorded prefix |
| Protected region | `/dev/nvme0n1` (WD_BLACK SN7100 1TB, serial 251057801005), first 16 MiB, pre-boot `a20de77e945f02d3f89efff891fb0b4ae793810a332da1ffc23e77b6d8917adc` |

Machine and firmware facts were read from the host's own DMI rather than typed:
Framework / Laptop 13 (AMD Ryzen AI 300 Series), board `FRANMGCP07`, INSYDE
Corp. `03.04`, `SecureBoot = 0`. The disabled setting is consistent with an
unsigned image being accepted by the firmware.

## Why P6.6 is still open

One boot. The contract fixes `requiredBoots = 2`, and the validator was run
against a draft record built from this observation to confirm it refuses:

```
REFUSED: observation records 1 boots, but the contract requires exactly 2
```

Everything else in that draft passed on the way to the refusal — medium
binding, write-target digest, machine fields, panel lines, and the generation
cross-check — so the only missing input is a second cold boot of the same
image. Two is fixed rather than a minimum precisely so that a run retried until
it passed cannot be recorded as a success.

The post-boot digest of the protected region is also still unread; `verify`
takes it, and it must be read after the boots rather than assumed.

## Next attempt

One more cold boot of the same image — no reflash, since these are the bytes
`prepare` recorded — then `verify`. Nothing in the root changed after this
attempt, so attempt 5's image is *not* superseded.
