# `sel4-crossing.zti` — direct endpoint delegation crossing

The crossing generation uses two preinstalled native Endpoint paths. The peer
holds carrier slot 0 and gate slot 1; init holds their paired endpoint roles at
declared slots 2 and 3. Spawn carries no endpoint role because runtime installs
all declared and minted bindings automatically.

Init delegates its gate authority over the carrier with
`capability_delegate`, using the typed 64-byte capability-transfer descriptor
and a `send`-only rights mask. The runtime narrows and copies the logical
authority before sending it, so exactly one capability accompanies the direct
message. The peer proves the received authority cannot receive, can send, and
uses it to return `survived`; init's successful receive proves copy semantics
left the sender's original authority usable.

The peers then complete forty blocking native rendezvous request/reply
exchanges, more than the retired logical channel lifetime bound of thirty-two.
The causal markers cover narrowed one-cap delegation, retained sender
authority, peer use of the received endpoint, and completion beyond the old
bound. No runtime endpoint creation, logical transit state, or endpoint wait
source participates.
