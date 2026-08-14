# `sel4-qos.zti` — the C8.5 timed-QoS generation

An eleventh seL4 generation, beside [`sel4.zti`](sel4.md),
[`sel4-channel.zti`](sel4-channel.md), [`sel4-loan.zti`](sel4-loan.md),
[`sel4-spawn.zti`](sel4-spawn.md), [`sel4-sample.zti`](sel4-sample.md),
[`sel4-stream.zti`](sel4-stream.md),
[`sel4-supervision.zti`](sel4-supervision.md),
[`sel4-crossing.zti`](sel4-crossing.md), [`sel4-call.zti`](sel4-call.md), and the
frozen x86 [`valid.zti`](valid.zti). It supplies the clock P5.4.5's remaining
C8.5 arms need.

**It does not pass yet.** Three arms that were unreachable without a clock now
fire — bounded RELIABLE retry accounting, deadline miss, and liveliness loss —
and the plane then stops at `[fabric] fail: no inline retained publisher`. See
[`devlog/2026-08-07-p5-4-5-qos-clock/`](../../../../devlog/2026-08-07-p5-4-5-qos-clock/index.md).

## Why this fixture is the stream graph verbatim

It is byte-identical to [`sel4-stream.zti`](sel4-stream.md) apart from its
generation number. That is deliberate rather than lazy: C8.5's timed arms are
properties of the *same* graph observed with time advancing, so declaring a
different graph would make the gate assert about a different composition than
the stream plane it extends.

Everything the clock needs is added at runtime by `init.rs`, not here — see
below.

## Why generation 19, and why no new boot-layout table

The generation number must differ from the stream plane's, because
`build-generation.py` keys its Cargo target directory on it and two generations
sharing one directory would let the second build reuse the first's `init.elf`.

It needs no `boot_layout.py` entry. Generation 19 is absent from both `OVERRIDES`
and `REPLACEMENTS`, so it resolves the base table pruned to this component set —
the same 31 rows the stream plane resolves, verified by resolving both. The clock
is *minted* rather than declared, so the layout numbers nothing new. A
replacement table would have been a second thing to keep in agreement for no
benefit.

## The clock is not a declared grant

This is the fixture's one load-bearing omission. There is no
`fabric-time-control` grant, and adding one would be wrong twice over: the root
would materialize it as an edge numbered from its own channel cursor, and the
generated profile's control table would gain a participant the graph does not
declare.

Instead `init.rs::drive_stream_plane` mints one pair with `endpoint_create` when
`qos_plane()` holds, and hands the halves out as spawn grants:

- grant **9** to `fabric-service`, matching its literal `TIME_SLOT = 9`. The nine
  grants before it fill `0..=8` exactly, so this position is not free — a
  participant added above moves it, which is why `init.rs` sizes the array from
  `STREAM_PLANE_CLIENTS` and asserts the count rather than writing `9`.
- grant **3** to `fabric-publisher-b`, matching its own `TIME_SLOT = 3`. That
  component drives the scheduled boundaries, so the component that publishes is
  also the one that says what time it is — the wiring the oracle's QoS gate uses.

Every capability this plane needs is therefore a spawn grant, which is why
backlog **B25** — the post-spawn introduction P5.4.6 is stuck on — does not block
it.

## Why the QoS behaviour is not a build flag

It used to be. `build-generation.py` mapped this manifest to two compile-time
selectors — one choosing init's plane driver, one selecting the QoS behaviour in
`fabric-service`, `fabric-publisher-b`, and `fabric-subscriber-b` — and the
cutover deleted both along with the other 41 `SLIME_*_CHECK` flags (B50).

The manifest is the selector now: the builder emits `GENERATION_BOOT_ACTION` from
this fixture's `bootAction`, and each component reads it. That keeps the three
components byte-identical across the stream and QoS planes — which is what
`check-sel4-stream-plane.py`'s unmodified-component assertion demands — without a
build flag able to disagree with the graph it was built for.

It also retires the two defects the flags carried. Backlog **B27** was a flag
table that set and scrubbed in one pass, so the `sel4-qos` row popped the flag
the `sel4-stream` row had just set; and the plane driver needed a second guard
requiring the *absence* of a flag, because otherwise an unrelated branch claimed
this plane and walked the wrong boot layout. Both are unreachable once one
authenticated field decides.

## What still blocks it

`create_late_subscriber` (`fabric-service.rs`) requires a `DURABILITY_RETAINED`
publisher whose retained head is *inline*. This graph's retained publishers are
`fabric-publisher` (inline) and `fabric-publisher-b` (which lends a
`>MAX_INLINE_BYTES` sample), and at the moment the fabric's scheduled boundaries
run, `fabric-publisher` has not yet taken its role — so no inline retained head
exists.

Closing it needs the graph to guarantee that head structurally, by declaring a
retained publisher that publishes small. It is **not** a scheduling nudge away:
adding a `yield_now` before the clock driver's spawn removed all three working
arms, which is recorded as a correction on the devlog entry.
