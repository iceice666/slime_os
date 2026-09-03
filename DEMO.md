# Trying Slime OS on the Novatek NT98690 H1V1

The P6 lane's end state, hands-on: the board boots seL4 and `slime-root` from
an SD card through its unmodified vendor firmware and hands you a resident
Slisp shell on UART0. Nothing here writes the board's eMMC, and the vendor
firmware remains the recovery path — pulling power at any moment is safe,
because nothing in this image persists anything.

## What you need

- The named H1V1 board with **SW18 at `0x1001`** (never `0x0001` — that is the
  loader's rescue mode).
- Its UART0 wired to your machine (115200 8N1, `/dev/ttyUSB0` below).
- An SD card carrying `slime-sel4-ns02201-h1v1-test-terminator.bin` at the
  root of its first FAT32 partition. If you ran the P6.C gate, it is already
  there. To build it fresh from the repository:

  ```sh
  python3 scripts/build/build-sel4.py --component-graph --platform ns02201-h1v1 --test-terminator
  python3 scripts/build/build-nt98690-payload.py --sel4 \
      --image build/slime-sel4-graph-ns02201-h1v1-test-terminator.elf \
      --output-stem slime-sel4-ns02201-h1v1-test-terminator
  ```

  then copy `build/nt98690-payload/slime-sel4-ns02201-h1v1-test-terminator.bin`
  onto the card yourself — no script in this repository writes removable media.

## Boot it

Open a serial terminal:

```sh
picocom -b 115200 /dev/ttyUSB0
```

(`screen /dev/ttyUSB0 115200` and `minicom -D /dev/ttyUSB0 -b 115200` work
too.) With the board off, **hold Enter and plug in power** — the vendor
U-Boot has `bootdelay=0` and polls for a keypress exactly once, so the byte
must already be waiting when it looks. At the `nvt: ` prompt:

```
mmc dev 0
fatload mmc 0:1 0x10286000 slime-sel4-ns02201-h1v1-test-terminator.bin
booti 0x10286000 - ${fdtcontroladdr}
```

About fifteen seconds of loader, kernel, and root output later:

```
slisp>
```

## Talk to it

You are typing into a userspace component on seL4. The shell holds no device
or MMIO capability: the root task polls UART0 and feeds bytes through the
shell's declared `InputRead` authority.

```
slisp> (define answer 40)
=> 40
slisp> (+ answer 2)
=> 42
slisp> sysinfo
```

`sysinfo` spawns a real component through spawn-service under
generation-declared authority, prints its report, and exits cleanly; `echo-agent`
is the other command in the spawn profile. Definitions persist for the life
of the session, backspace works, and malformed input answers with `! syntax`
or `! arity` rather than anything dying.

## Leave

Press **Ctrl-]**. This image is the gate artifact, so the byte `0x1d` is
intercepted by the root and routed into the SoC watchdog: you get
`SLIME_NT98690 test terminator accepted`, the board resets, and it comes back
up in its vendor firmware, then vendor Linux, untouched. Boot the image again
with the same three commands from `nvt: ` whenever you want it back; the card
keeps the image until you overwrite it.

## One sharp edge: Escape reboots the board

Slisp is one of the four *required* residents, so init fails closed the moment
it exits — and the shell exits on an **Escape** byte (`0x1b`), printing
`[slisp] repl done`. Losing a required resident is a FATAL, and the root
resets the board through the watchdog on the `cause=fatal` path. So an Escape
does not just drop the prompt; it reboots the board back to vendor firmware.

The trap: **arrow keys, Home/End, Page-Up/Down, and function keys all send a
sequence that begins with `0x1b`**, so any of them will trip this. Edit a line
with backspace or by retyping, not with the arrow keys, and use Ctrl-] rather
than Escape to leave. A reboot here is harmless — nothing is written — but it
costs you the session.

The `<<seL4(CPU 0) … CNode … Source slot invalid or empty>>` lines you may see
during a `sysinfo` spawn are benign kernel debug output, not errors.
