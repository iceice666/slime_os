# Physical boot attempt 6 — the observation

Immutable record of the second cold boot and the `verify` run that closed
P6.6. Frozen — corrections append.

## Reported observation

The operator booted the image twice; both boots showed the same fourteen-line
panel record as [attempt 5](physical-attempt-5.md). `verify` then reported:

```
Protected region after the boots: sha256:a20de77e945f02d3f89efff891fb0b4ae793810a332da1ffc23e77b6d8917adc
Wrote and validated evidence/framework-cpu-boot/slime-cpu-boot.observation.json
```

The first invocation, before the second boot was recorded, was refused:
`observation records 1 boots, but the contract requires exactly 2`. The boot
count is the one input a gate cannot supply for itself.

## The no-write claim

The protected region's post-boot digest is
`a20de77e945f02d3f89efff891fb0b4ae793810a332da1ffc23e77b6d8917adc` — byte for
byte the pre-boot value `prepare` recorded. The first 16 MiB of
`/dev/nvme0n1` (WD_BLACK SN7100 1TB, serial 251057801005) is unchanged across
two cold boots of the removable image.

That span is where a stray write would land first: the protective MBR, the
primary GPT header and its entry table, and the start of the first partition.
Both digests are read from the block device with the page cache evicted
(`posix_fadvise(POSIX_FADV_DONTNEED)`), so a cached read cannot reproduce the
pre-boot bytes from memory and report a match the disk does not support. The
root never held any block-device capability in this generation; the digests are
what make that checkable rather than merely declared.

## What the record binds

`evidence/framework-cpu-boot/slime-cpu-boot.observation.json`, identity
`2ab15f9a63b7e1eb4186039e5b39587f4c0b4e9350924ea640fd04a03a353fe4`,
independently recomputed from the stored fields and matching.

| Field | Value |
|---|---|
| Image | `95a0d213c1249cc73e462d5bed9b9462bea577f63fcb33093c83e025b8a14303` |
| Media identity | `104b5959704a69ed496fe3037b78b2abb34addda3adc60eef043dc403684a20e` |
| USB read-back | equals the image digest |
| Generation | `1`, identity prefix `0F1929F39BF5FA1F`, on the panel and in the medium |
| Machine | Framework, Laptop 13 (AMD Ryzen AI 300 Series), AMD Ryzen AI 7 350 w/ Radeon 860M |
| Firmware | INSYDE Corp. 03.04, Secure Boot disabled |
| Mode | 2256x1504x32 |
| Boots | 2, both `ready`, both `coldStart`, both `keyboardUsed: false` |
| Protected region | `/dev/nvme0n1`, 16777216 bytes, pre == post |

Boot 0 carries the photograph's digest
(`d3e34b99f0f15b179f2313ee4c4160348eee45878be0dd5bb2ffd8f62376b180`). Boot 1's
`panelSha256` is deliberately **empty**: the operator observed it directly, the
contract admits an empty digest for that, and copying boot 0's hash would
assert a photograph that does not exist.

`just framework_cpu_boot_check` now refuses its nineteen forgeries and then
reports the claim rather than `P6.6 is OPEN`:

```
framework_cpu_boot_check: 2 cold boots of image 95a0d213c1249cc7… observed on
Framework Laptop 13 (AMD Ryzen AI 300 Series) (firmware 03.04), internal region
a20de77e945f02d3… unchanged
```

## What this does not claim

P6.6 closes the CPU and product boot path and nothing else. Specifically not
claimed, and each still owned by its own milestone:

- No device is qualified. No PCI enumeration, bus mastering, DMA, keyboard,
  touchpad, NVMe access, network, Wi-Fi, audio, battery, thermal, or
  suspend/resume support exists or is implied. H1 begins that inventory.
- The display is not a device driver. The root writes pixels into a framebuffer
  the firmware already programmed; it sets no mode, enumerates nothing, and
  takes no input.
- The Framework target still reuses the pc99 kernel configuration, including
  its HPET assumption and `KernelSupportPCID OFF`. The timer is observed to
  work on this machine; the configuration is not therefore *right* for it.
- Secure Boot was disabled. The image is unsigned, so a machine with it enabled
  would refuse the medium.
- Internal storage was protected, not used. M5.7's storage-aware boot remains
  open and is a different claim.
- Two boots on one machine. Nothing here speaks to another Framework unit, a
  different firmware revision, or another x86-64 machine.
