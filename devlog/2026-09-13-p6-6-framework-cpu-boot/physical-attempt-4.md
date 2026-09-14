# Physical boot attempt 4 — operator report

Immutable record of the fourth boot attempt on the named Framework: the first
that ran the whole chain to its terminal line. Frozen — corrections append.

## Reported observation

The operator reports the boot completed and `IDLE - SAFE TO POWER OFF` was
visible. Photograph: [`physical-attempt-4-panel.jpg`](physical-attempt-4-panel.jpg)
(SHA-256 `2ad9b8839a73b258b12e4ce3fd1f9816a00babe7e789464754a3b1585e98d1f5`).

Transcribed from the photograph, top to bottom:

```
Booting `slime'
SLIME OS - ROOT RUNNING
TIMER MAPPED - AWAITING TICK
Trying to terminate EFI services again          <- GRUB text, overlapping
?GENERATION ADMITTED
IMAGES STAGED
ENDPOINTS WIRED
COMPONENTS PINNING                              <- misread of RUNNING
SERVING - AWAITING GRAPH
SLIME OS
TARGET: X86_64-SEL4-FRAMEWORK13-AI300
GENERATION: 41?1?19329F39BF5FA1F                <- misread; see below
DISPLAY 2256X1504X32
ROOT READY
IDLE - SAFE TO POWER OFF
```

## What this settles

**The full boot path works on the named Framework.** Every stage marker and
every readiness line rendered, ending at the terminal line. Newly confirmed
beyond attempt 3: the service loop reaches its receive, components report, the
graph certifies healthy, and the root reaches its declared terminal state.

**The panel's real mode is `2256x1504x32`** — the Framework 13's native
resolution, and the first time the physical geometry has been recorded. Two
consequences:

* The 32-bpp path runs on hardware. Every QEMU boot so far reported 24 bpp
  (std VGA) or 32 bpp only at 1280x800 (virtio-vga); the real panel is neither.
* At 4 bytes per pixel the pitch is 9024 bytes and the mode spans 13572096
  bytes — 12.9 MB, or 3314 granules. That is 7.4x the 448-entry
  physical-provenance bound, which confirms retrospectively why attempt 2's
  per-granule leak stalled graph launch, and why the leak had to be fixed
  before anything past `GENERATION ADMITTED` could run.

## Defect this exposed

The record rendered, but **not legibly**. Two lines of GRUB's own console text
are still on screen underneath it — ``Booting `slime'`` and `Trying to
terminate EFI services again` — in the firmware's smaller proportional font,
overlapping the bitmap record. Vision analysis of the photograph reports
doubled strokes where they intersect, could not read `TIMER TICKING` at all,
and misread the generation digest as `41?1?19329F39BF5FA1F` where the correct
value is `0F1929F39BF5FA1F`.

Root cause, in this milestone's own code: `write_line` iterated the glyph
bitmaps and wrote only the **set** bits, skipping clear ones with `continue`.
Whatever occupied those pixels stayed. On every QEMU boot the framebuffer
starts black, so background writes were indistinguishable from skipped ones and
the defect was invisible — thirteen prior emulated captures decoded perfectly.
A real panel arrives holding the bootloader's output.

An evidence channel that can be misread is not an evidence channel: a digest
photographed as `41?1?…` cannot bind a boot to a generation, which is precisely
what `contracts/cpu-boot-observation/v1` requires of it.

Fixed: every pixel of a line's cell band is now written, background included,
across the full text width so a short line still erases a longer one beneath
it. The background is painted in the same left-to-right pass as the glyphs,
preserving the ascending-offset invariant the device untyped's monotonic retype
demands.

## Verification of the fix

A first attempt at a regression control booted QEMU with GRUB's `gfxterm` and a
wall of echoed text, intending to dirty the framebuffer before the root ran. It
was **vacuous** — the captured panel showed content only inside the record's own
band and nothing below it, so `gfxterm` never engaged and the framebuffer was
still clean. Reintroducing the defect under that control still decoded all
fourteen lines perfectly. The control was discarded rather than kept.

What replaced it is deterministic and needs no emulator: replay the renderer's
write list into a buffer pre-filled with `0xa5` noise, then assert every pixel
of the band is either glyph-white or black. Reintroducing the defect fails it
with `stale pixel at (…)`; both it and the coverage test pass with the fix.

## Not yet claimed

P6.6's exit condition needs **two** cold boots of one image plus the recorded
observation, and the legibility fix changes the root, so the bytes that
completed this boot are superseded. `build/framework-cpu-boot.state.json`
records that `prepare` ran against the previous image: the USB read back
byte-identical to it (`41d3dd69…`), and the internal NVMe's protected 16 MiB
region hashed `a20de77e945f02d3…` before the boot. That pre-boot digest belongs
to a superseded image, so the next run starts from `prepare` again.

## Next attempt

Reflash, then two cold boots of the same image. The record is complete when
`IDLE - SAFE TO POWER OFF` is legible with no bootloader text showing through,
and the `GENERATION` line reads `0F1929F39BF5FA1F` exactly — that digest, on
the panel, is what binds the observation to the medium.

## Corrections

- **2026-09-13 — the legibility fix was incomplete.** The operator reports that
  after the fix the Slime text's own background is correctly black, but GRUB
  message residue remains **in the screen corners**. That is a second, distinct
  omission of the same kind: the fix painted each line's *cell band*, which
  leaves the top margin, both side margins, and every row below the last line
  never written at all. A record surrounded by bootloader text is still
  ambiguous evidence.

  Now each line clears its **whole screen rows**, edge to edge; the first line
  additionally clears the rows above it; and the terminal line — `render_idle`,
  and the fatal path equally — clears every row below the record. All of it
  stays within the single ascending pass the framebuffer's monotonic retype
  requires, which is why the tail is cleared at the end rather than the screen
  cleared at the start.

  The whole-panel check this should have had from the beginning: of the 480000
  pixels in an emulated 800x600 capture, **0** are now neither exactly black
  nor exactly white. Band-only clearing fails the unit test that mirrors the
  renderer (`a_line_writes_every_pixel_of_its_rows_and_the_top_margin`,
  verified failing on the reverted code and passing on the fix). A QEMU
  framebuffer starts black, so no emulated boot can *demonstrate* residue
  being erased — the purity count is the strongest emulated statement
  available, and the corners are the operator's to confirm.
