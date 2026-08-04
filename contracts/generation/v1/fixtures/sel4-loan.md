# `sel4-loan.zti` — the P5.3.2 loan-plane generation

A fourth seL4 generation, beside [`sel4.zti`](sel4.md),
[`sel4-channel.zti`](sel4-channel.md), and the frozen x86 [`valid.zti`](valid.zti).
It declares the smallest graph that can exercise both halves of P5.3.2's exit
condition: a loan crossing between two components, and four declared quota
ceilings refusing at ceiling+1 without disturbing a third holder.

## Why a fourth generation

The same mechanical reason as `sel4-channel.zti`'s: `init.rs` selects its
scenario with `option_env!`, resolved at compile time, so one component build
cannot serve two gates. A separate generation — and so a separate image — is
required rather than preferred.

It also keeps P5.3.1's evidence intact. `just sel4_channel_check` asserts
`sel4-channel.zti`'s graph marker-for-marker, down to task ids, channel keys,
and exact send and receive counts. Adding a third component and a loan grant to
it would rewrite all of that for reasons unrelated to what that gate proves.

## The three components

### `init` — the lender

Stands in for `sample-lender`, which is what the x86 oracle uses. Not
`sample-lender` itself, because that component is *spawned* by init on x86 and
receives its channel end and its buffer factory through spawn grants — a
mechanism this cutover does not have until P5.3.3. Using init as the lender is
what lets the loan plane be exercised without the spawn plane.

Its ceiling is `4 / 2 / 2 / 1`, and every number is load-bearing:

- **4 pages** bounds the two-page loaned region plus the two single-page probe
  regions, so the buffer probe below stays inside the page budget and it is
  unambiguously the buffer count that refuses;
- **2 buffers** is what the third probe region exceeds;
- **2 mappings** is what the third probe mapping exceeds;
- **1 loan** is what the second loan of the sealed region exceeds.

Each probe asks for exactly one more than one ceiling with the other three
unspent, so a refusal names the class it was aimed at rather than whichever
limit happened to be reached first. The probes release everything they took
before the loan runs, so the loan's own refusals are equally unambiguous.

### `sample-receiver` — the receiver, unmodified

The same binary the x86 oracle's `just sample_plane_live_check` runs, with no
seL4-specific branch and no change of any kind. That is the load-bearing claim
of this fixture: a component written against the retired kernel's loan ABI runs
on `slime-root` because the ABI is the same one, not because the scenario was
rewritten to suit whatever the root task happened to implement.

It brings four denial arms of its own for free — a descriptor naming another
loan, a map past the loaned range, a writable map of a read-only loan, and a
second return — each of which the gate asserts.

Its ceiling is `4 / 1 / 1 / 1`: one mapping of the loaned region is all it takes.

### `console` — the unrelated holder

Takes no part in the loan and holds no loan capability, but it *is* a declared
shared-buffer holder with its own `2 / 1 / 1 / 1` ceiling — which is the point.
"Without disturbing an unrelated holder" is only checkable if the unrelated
holder actually uses its quota afterwards.

So on the message init sends after exhausting all four of its own ceilings,
console runs the startup probe's full create / map / write / seal / unmap /
release against its own budget entry and reports `shared-buffer quota live`. A
ceiling that leaked across holders reports `quota exhausted` there instead.
Receiving and printing would show only that the channel plane works.

A third holder is the minimum for this. With two components, every ceiling in
the budget belongs to a participant in the loan, and a leak would be invisible.

## The three grants

### `sample-receiver-side` — the loan's channel, and how it names its receiver

`source = "init"`, `target = "sample-receiver"`, `rights = ["send", "recv"]`,
`transferable = true`.

Bidirectional because both directions are used: init sends the descriptor, and
the receiver sends back the settled signal init waits for before reclaiming.

The name is the boot layout's label for init's end of this channel, not an
arbitrary one. `slime-root` requires an endpoint's rights to be contained in the
rights the layout declares for the slot it is placed at, so the grant name and
the layout label have to agree — `sample-receiver-side` carries `7`
(`send | recv | transfer`), which contains what this grant gives.

`transferable = true` is what lets the loan capability cross it, and it is
enforced at the **mint**: `serve_buffer_loan` refuses to create a loan at all
over a channel the generation did not mark, so an undelegated edge never has a
capability to carry. `Resource::is_transferable` is the separate question of
which resource *kinds* the root has a mechanism to move, checked at the send.
Both must hold, and only a loan satisfies the second.

This channel is also how the loan names its receiver. `shared_buffer_loan`
resolves its `receiver_slot` to a channel end and binds the loan to the task at
the other end of it. The retired kernel instead resolves a `RIGHT_SUPERVISE`
handle minted when init spawned the receiver — which does not exist here,
because there is no spawn. See `slime-root/src/main.rs::serve_buffer_loan` for
why no reading of a `supervise` *grant* would produce one either, and for what
P5.3.3 replaces this with.

### `dango-output` — the unrelated holder's channel

`source = "init"`, `target = "console"`, `rights = ["recv"]`,
`transferable = true`.

Exactly as in `sel4-channel.zti`, and for the same reason: `dango-output` is the
layout label whose rights (`send | transfer`) describe init's *send* half, while
`console-output` describes the receive half. The half a component holds and the
label it is placed under have to agree.

### `powerbox-client` — the channel a stranded loan is sent over

`source = "init"`, `target = "console"`, `rights = ["recv"]`,
`transferable = true`.

A second edge to `console`, existing for one reason: `console` loops on slot 0
alone and never reads this one. So a message sent here is queued and never
collected, deterministically — which is how `init` leaves exactly one loan
capability in flight for the root's transit reclamation to settle, rather than
racing a peer that might consume it.

That arm needs a deliberate strand because every other transfer in this fixture
completes. A fault injection removing `Transit::reclaim` passed the gate before
this grant existed: the path was untested and looked covered. It is now asserted
by the terminal `transit=0`.

## What this fixture does not declare

No `spawn` or `exec` grants, so no component starts another — that is P5.3.3.
No fabric graph and no interface schema, since C8 is P5.5.

No `bufferCreate` grant either, and that is worth stating rather than passing
over. `slime-root` does not currently check one: `serve_buffer_create` ignores
the factory slot its caller names and admits the allocation against the holder's
declared quota alone. So the quota *is* the whole bound today — which is why
`console`'s ceiling above is small but non-zero, and why a component the budget
omits allocates nothing at all.

That is a real gap against "authority is never ambient", and it predates this
slice: the retired kernel resolves a `RIGHT_BUFFER_CREATE` capability first and
this root task never has. It is recorded as **B13** in
[`roadmap/00-backlog.md`](../../../../roadmap/00-backlog.md) rather than fixed
here, because closing it means materializing non-channel grants into every
component's table — the same distribution step P5.3.3 builds for spawn — and
doing it now would rewrite P5.2's slot numbering, which
`just sel4_component_graph_check` asserts marker-for-marker.
