# Physical boot attempt 2 — operator report

Immutable record of the second boot attempt on the named Framework, with the
staged panel markers from attempt 1's change. Frozen — corrections append.

## Reported observation

```
SLIME OS - ROOT RUNNING
TIMER MAPPED - AWAITING TICK
TIMER TICKING
GENERATION ADMITTED
```

Then nothing further.

## What this settles

Attempt 1 left two hypotheses open. This closes the first outright and answers
several questions no QEMU run could:

| Claim | Status after attempt 2 |
|---|---|
| GRUB loops forever in `ExitBootServices` | **Refuted.** `ROOT RUNNING` cannot render unless the handoff completed and `slime-root` executed. The retry message in attempt 1 was GRUB succeeding on a later iteration. |
| Multiboot2 GRUB→seL4 handoff works on this machine | **Confirmed.** |
| seL4 pc99 boots natively on a Ryzen AI 7 350 | **Confirmed.** |
| The firmware negotiates a linear framebuffer, and the kernel exports it | **Confirmed** — the panel is the channel carrying these four lines. |
| The root's own framebuffer decode, mapping, and glyph rendering work on real hardware at the panel's real mode | **Confirmed.** |
| The pc99 HPET assumption holds on this machine | **Confirmed** — `TIMER TICKING` means a real timer interrupt was delivered. This was the predicted most-likely failure and it is not one. |
| Generation admission succeeds against the physical target profile | **Confirmed.** |

The boot therefore reaches the last marker before components launch and stops
in graph launch — between `GENERATION ADMITTED` and the readiness record.

Everything above is first-time physical evidence for the x86-64 lane. None of
it was previously observed on hardware.

## Defects this exposed, both in P6.6's own code

### 1. A CSlot and provenance leak per framebuffer granule

`DeviceRegion::unmap` frees the virtual window and deliberately keeps the frame
capability, because a *bound device* stays bound for the boot. A framebuffer is
the opposite: scanned once, top to bottom. The walk therefore retained one
capability — and one physical-provenance record — per 4 KiB.

Reproduced locally by forcing a larger mode, which is what the real panel is:

```
800x600x24   → SLIME_ROOT allocator baseline live_slots=54
1280x800x32  → SLIME_ROOT allocator baseline live_slots=119
```

The provenance table is sized for the live DMA-capable population
(`MAX_FRAME_ANCHORS + DMA + MMIO` = 448 entries), not for a display. A
panel-sized mode approaches that bound, and the failure it produces lands on
whatever allocates next — the *graph* — which is consistent with where this boot
stopped. Fixed: the walk now releases each finished granule's capability and
CSlot. Baseline at 1280x800x32 returns to 55.

### 2. `fatal!` was invisible on this machine

The more serious defect, and the reason attempt 2's report is still ambiguous
about the *cause* of the stop. `fatal!` printed only to serial. There are 56
`fatal!` sites in the graph-launch path alone, and on a machine with no serial
port every one of them was a silent stop indistinguishable from a hang.

Fixed: `fatal!` now also renders `ROOT FATAL - BOOT ABANDONED` on the panel.
Proven by injecting a real failure — corrupting the generation magic in the
root ELF on the medium, so admission genuinely refuses it:

```
SLIME_ROOT FATAL generation rejected: BadMagic
```

and the panel, decoded from a `screendump`:

```
SLIME OS - ROOT RUNNING
TIMER MAPPED - AWAITING TICK
TIMER TICKING
ROOT FATAL - BOOT ABANDONED
```

The label lands at the correct position — after `TIMER TICKING`, before
`GENERATION ADMITTED` — which is exactly where `BadMagic` occurs.

### 3. The leak fix broke rendering, and the fatal test caught it

Worth recording because it is the kind of defect a success-path test cannot
see. Releasing each granule's capability deleted the *last* capability derived
from the framebuffer's device untyped. seL4 then resets that untyped's free
index to zero, so the next `allocate_device_frame` restarted the walk from the
beginning instead of advancing. The allocator states this invariant at the one
other place it walks a device untyped:

> Keep one child while advancing to the next chunk. If every cap disappeared,
> seL4 would reset the device untyped's free index to zero and the next retype
> would recreate the prefix instead of continuing toward the requested
> physical page.

Observable result: the panel drew two of sixteen pixel rows of the first line
and stopped. The success-path QEMU boot still reported
`SLIME_DISPLAY ready rendered`, because it had already mapped its granules
before the release path ran; only the injected-fatal capture showed the
garbage. Fixed by releasing the *previous* granule only after the next one
exists, so exactly one capability always anchors the untyped — two live frames
at peak rather than one per granule. Guarded by two new unit tests asserting
the walk never revisits a lower offset, within a line and across lines.

## Not yet explained

Why graph launch stops. The leak is fixed and the panel now reports fatals, but
this boot predates both, so its stop has no recorded cause.

Rather than spend the next attempt narrowing that, graph launch is now
instrumented too: it was the one phase with no marker between its start and the
readiness record. `IMAGES STAGED` (child VSpaces built), `ENDPOINTS WIRED`
(peer endpoints materialized), and `COMPONENTS RUNNING` (components and the
console dispatcher started) split it into three, so the next attempt names a
phase rather than a gap. The full chain is thirteen lines, verified rendered
under QEMU.

| Last line on panel | Conclusion |
|---|---|
| `ROOT FATAL - BOOT ABANDONED` | A typed error, and the preceding marker says which phase raised it |
| `GENERATION ADMITTED` | Stops before any image is staged — child VSpace construction or ELF loading |
| `IMAGES STAGED` | Endpoint materialization |
| `ENDPOINTS WIRED` | Component start or the console dispatcher |
| `COMPONENTS RUNNING` | Components run but the graph never reaches healthy — a service-loop or health-accounting condition |
| `IDLE - SAFE TO POWER OFF` | P6.6's exit condition is met |

The image to boot is the rebuilt one; the bytes from attempt 2 are superseded.
