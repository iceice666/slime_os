# Framework 13 CPU boot runbook

The one claim in this repository that QEMU cannot close: a named physical
Framework 13 (AMD Ryzen AI 300, target profile
`x86_64-sel4-framework13-ai300`) cold-boots the exact product image from
removable media, renders the root's readiness record on the firmware
framebuffer, and does not write internal storage. This page is the operator
procedure. The gate (`just framework_cpu_boot_check`) can prove that the
validator refuses forged evidence and can judge a record; it cannot produce
one.

What the observation qualifies: the CPU/product boot path on that machine
with that firmware. What it does not: any device (NVMe, USB, input, network,
display driver, audio, suspend, IOMMU) or daily-driver use.
[Targets and portability](../architecture/targets-and-portability.md) states
the boundary; [the Framework plan](../plans/framework-hardware.md) states what
qualification comes next.

## Prerequisites

- A Linux host with `nix develop`, root access for raw block-device I/O, and
  the repository checked out with submodules. The prepare step reads `sysfs`
  to classify disks and refuses anything that is not a whole removable disk.
- A removable USB disk at least as large as the image (it will be
  overwritten).
- The Framework 13 with the internal NVMe present. Its first 16 MiB (LBA 0
  onward: protective MBR, primary GPT, start of the first partition) is the
  *protected region* — hashed before and after, and the observation is refused
  if it changed.
- No serial port exists on the machine; the framebuffer panel is the only
  evidence channel, so you will transcribe lines by hand (a photo is
  optional and recorded by digest).

## 1. Build and qualify the medium

```sh
just framework_media_check
```

This builds `build/framework-media/slime-framework.img` (a deterministic raw
GPT image with one read-only EFI System Partition holding GRUB, the pinned
pc99 kernel, and `slime-root.elf`), writes `slime-boot-media.identity.json`
beside it, proves a byte-identical rebuild, exercises the malformed-image and
unsafe-target refusals, and boots the exact bytes under pinned q35/OVMF to
the resident product graph. `just framework_media_image` builds without
checking. Every later step revalidates the image against its identity record
first; a medium whose bytes drifted is not evidence of anything.

## 2. Write the medium and hash the protected region

```sh
sudo -E just framework_cpu_boot_prepare /dev/sdX /dev/nvme0n1
```

`/dev/sdX` is the **whole** removable disk (a partition path is refused);
`/dev/nvme0n1` is the internal disk to protect. The step:

1. refuses if the protect target is removable or is the same device;
2. hashes the first 16 MiB of the protected disk (evicting page cache first);
3. runs the guarded writer, which refuses non-removable, read-only, mounted,
   or partition targets and prompts before writing (`--yes` is available only
   by calling `scripts/check/check-framework-cpu-boot.py prepare` directly);
4. reads the device back over the image length and refuses if it does not
   match the approved image;
5. writes `build/framework-cpu-boot.state.json` recording the media identity,
   image digest, the generation identity prefix the panel must show, the
   write target, and `sha256Before` of the protected region.

Keep that state file; `verify` needs it and it must be from the same host
session as the boots.

## 3. Two cold boots, no keyboard

Do this on the Framework, not the host you just prepared on:

1. Power the machine off completely.
2. Insert the USB device. Power on and select it in the firmware boot menu.
   Do not type anything after that selection.
3. Wait for the panel. The root renders, in order:

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
   GENERATION 1 ID <16 hex>
   DISPLAY <W>X<H>X<bpp>
   ROOT READY
   IDLE - SAFE TO POWER OFF
   ```

   Transcribe the lines exactly (up to 16). The validator requires the
   `TARGET …` line and that the 16-hex generation prefix appears and equals
   the one recorded at prepare time. Note the display geometry.
4. Power off. Repeat once. **Exactly two** boots are required — not "at least
   two", so a run cannot be retried until it passes.

If a boot does not reach the panel, record its outcome honestly
(`no-boot`, `firmware-refused`, `wrong-profile`, `faulted`); `verify` will
refuse the record, which is the correct result.

## 4. Write the boot observations

Create `boots.json`, an array of two objects in index order:

```json
[
  {
    "index": 0,
    "outcome": "ready",
    "coldStart": true,
    "keyboardUsed": false,
    "targetProfile": "x86_64-sel4-framework13-ai300",
    "generationNumber": 1,
    "generationIdentityPrefix": "0123456789ABCDEF",
    "displayWidth": 2256,
    "displayHeight": 1504,
    "displayBitsPerPixel": 32,
    "recordLines": ["SLIME OS - ROOT RUNNING", "...", "IDLE - SAFE TO POWER OFF"],
    "panelSha256": ""
  },
  { "index": 1, "...": "..." }
]
```

`panelSha256` may be empty or the SHA-256 of a photo you keep outside the
repository. The field set is the `BootObservation` record in
`contracts/cpu-boot-observation/v1/schema.zt`; unknown fields are refused.

## 5. Verify and record

Back on the host, with the USB and the same internal disk present:

```sh
sudo -E python3 scripts/check/check-framework-cpu-boot.py verify \
    --boots boots.json \
    --operator "<your name>" --observed-on YYYY-MM-DD \
    --machine-vendor Framework --machine-product "Laptop 13 (AMD Ryzen AI 300 Series)" \
    --machine-cpu "<cpu string>" \
    --firmware-vendor "<vendor>" --firmware-version "<version>" \
    [--secure-boot]
```

(There is no `just` alias for `verify`; the recipe comment mentions one, but
call the script directly.) It re-hashes the protected region, refuses if it
differs from `sha256Before`, assembles the observation, writes it to
`evidence/framework-cpu-boot/slime-cpu-boot.observation.json`, and validates
it. Then:

```sh
just framework_cpu_boot_check
```

runs the validator's controls (a synthetic good record plus nineteen
mutations that must be refused) and judges the committed record against the
currently built medium. Commit the evidence file with the change that
rebuilt the image.

## What makes `verify` refuse

Any of: protected region changed; medium identity or image digest differ from
the currently built image; write target not removable, or reading back
differently from the image; boot count not exactly two; a boot not
`coldStart`, or with `keyboardUsed`; wrong target profile; missing `TARGET`
line; generation prefix absent from the panel or different from the prepared
image's; any outcome other than `ready`; an operator or machine field empty;
a malformed date. The full list is `validate_record` in
`scripts/lib/cpu_boot_observation.py`.

## When the record goes stale

The observation binds the medium's identity, which includes the digest of
`slime/slime-root.elf` and the embedded generation. **Any change to the root,
a component in the product composition, a contract the generation encodes, or
the pinned kernel unbinds the record**, and `just framework_cpu_boot_check`
refuses until an operator repeats this runbook on the rebuilt image. This is
by design: documentation cannot re-stamp a boot, and neither can QEMU.

Expect the gate to be in the refusing state most of the time between
hardware sessions. The claim that stands meanwhile is the historical one —
that the named machine booted the image identified in the retained record on
the recorded date — not that the current tree boots on it.

## Safety boundary

The medium has no writable product or state partition, requests no input,
and the image grants no PCI bus mastering or internal-storage authority.
Physical development is done from removable media only; internal NVMe writes
stay disabled until identity, bounds, reset/timeout, flush ordering,
interrupted writes, malformed metadata, rollback, recovery, and containment
are observed on disposable hardware first
([plan](../plans/framework-hardware.md)). `just framework_safety_check`
pins the allowlist of compositions whose holders may write storage at all, and
`framework_media_check` refuses a medium whose image compiled an input
authority.
