# Physical boot attempt 1 — operator report

Immutable record of the first boot attempt of the P6.6 media on the named
Framework. Not a gate transcript: the machine has no serial port, so there is no
captured stream, and this is the operator's account plus what can be deduced
from it. Frozen — corrections append.

## Reported observation

The panel showed, and then stopped changing:

```
booting slime os

trying to terminate EFI service again
```

No further output. No Slime marker of any kind.

## Attribution

Both lines are GRUB's, not Slime's and not seL4's:

- `Booting slime os` is GRUB echoing the `menuentry "slime"` title.
- `Trying to terminate EFI services again` is `grub_printf` in
  `grub_efi_finish_boot_services` (`grub-core/kern/efi/mm.c`), reached when
  `ExitBootServices` returns `EFI_INVALID_PARAMETER`. GRUB frees the memory
  map, re-reads it, and retries in an unbounded `while (1)`.

Confirmed present in the shipped bootloader:

```
$ strings build/media/framework13-ai300-graph/EFI/BOOT/BOOTX64.EFI | grep -i 'terminate EFI'
couldn't terminate EFI services
Trying to terminate EFI services again
```

The boot therefore stopped at or immediately after the GRUB→kernel handoff.
Nothing in Slime had run, so no Slime defect is implicated by this transcript.

## What the report proves regardless of cause

The firmware's own text console works on that machine and was the terminal
carrying these messages. This is load-bearing: the configuration at the time of
the attempt set `terminal_output serial` *only*, and the Framework has no 16550
at `0x3f8`. GRUB's `serial` command must therefore have failed, leaving the
firmware console as the active terminal by fallback. A channel that works by
accident is not an evidence channel, which is what the change below addresses.

## Two hypotheses, neither yet distinguished

| # | Hypothesis | Argues for | Argues against |
|---|---|---|---|
| 1 | GRUB is stuck in the retry loop | Message is the retry itself; some firmware needs the loop | A stuck loop reprints the line on every iteration and would scroll; one line was reported |
| 2 | GRUB succeeded on a later retry and seL4 or the root then stopped | After `ExitBootServices` succeeds, GRUB's console output ceases, so the last line freezes on screen exactly as reported | Nothing yet observed from the kernel |

Hypothesis 2 is the better fit for a *single* frozen line, and it has a
plausible mechanism: the Framework profile deliberately reuses the pc99 kernel
configuration, including its HPET assumption at `0xfed00000`, and the root
blocks in `prove_timer` until a real timer interrupt arrives. On a machine whose
timer this profile assumed rather than observed, that is a silent, permanent
stop — and at the time of the attempt the root rendered nothing until readiness,
so a stop anywhere earlier was indistinguishable from a stop in GRUB.

## Not reproducible under the pinned emulator

`ExitBootServices` succeeds first try under OVMF 202605. Zero occurrences of the
retry message across every captured QEMU boot:

```
$ grep -ci 'terminate EFI services' /tmp/fbprobe/newkernel.log /tmp/fbprobe/gop.log
newkernel.log:0
gop.log:0
```

So the retry is a property of the real firmware. The gate cannot reproduce it,
and no QEMU pass can refute it.

## Ruled out by experiment

Dropping the GOP mode switch to avoid perturbing the EFI memory map was the
obvious evasion. It does not work: with GRUB's video modules absent and only the
kernel's own Multiboot2 framebuffer request tag present, the kernel reports no
framebuffer at all.

```
framebuffer: NONE
READY: True
```

The mode switch and the framebuffer are the same thing, so the readiness channel
cannot be preserved by removing it.

## Change made in response

Observability first, because the next attempt must report *where* it stops
rather than leaving these two hypotheses open:

1. The panel is claimed immediately after the allocator, before the input and
   timer phases, and each stage renders as it passes — `SLIME OS - ROOT
   RUNNING`, `TIMER MAPPED - AWAITING TICK`, `TIMER TICKING`, `GENERATION
   ADMITTED`, then the readiness record. `ROOT RUNNING` alone discriminates
   hypothesis 1 from hypothesis 2 on sight, and `TIMER MAPPED` without `TIMER
   TICKING` names the HPET assumption as the cause.
2. GRUB's console is now `terminal_output console serial` rather than serial
   alone, so bootloader-stage diagnostics reach the panel by declaration
   instead of by fallback. `gfxterm` was considered and rejected: it needs a
   `.pf2` font file, which would add a fifth file to the pinned four-file EFI
   tree, and the firmware console already works.

Verified under QEMU by decoding a `screendump` against the font table — all four
stage lines plus the six record lines render in order.

## Next attempt

Boot the rebuilt image and report the last line visible on the panel. The
expected outcomes and what each would mean:

| Last line on panel | Conclusion |
|---|---|
| `trying to terminate EFI service again`, no Slime line | Hypothesis 1: GRUB really is looping; the fix belongs in the EFI handoff |
| `SLIME OS - ROOT RUNNING` | Handoff works; the root starts and stops before the timer |
| `TIMER MAPPED - AWAITING TICK` | The HPET assumption is wrong for this machine — H1's territory, and the most likely outcome |
| `TIMER TICKING` / `GENERATION ADMITTED` | Timer is fine; failure is later, in admission or component launch |
| `IDLE - SAFE TO POWER OFF` | P6.6's exit condition is met and the observation can be recorded |
