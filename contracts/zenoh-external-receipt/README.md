# Zenoh external-listener receipt

`v1/schema.zt` owns the record a stock `eclipse-zenoh==1.0.0` listener on the host writes
about what its subscriber delivered. `scripts/lib/zenoh_listener_peer.py` runs the listener
and writes the record; `scripts/lib/zenoh_capture.py` reads it back through this schema's
decoder, and `scripts/check/check-sel4-io-network-plane.py --arm zenoh-external` holds it
against the packet capture and the guest's report.

The record is a container only. The wire is eclipse-zenoh's, so the contract declares no
layout for it. It fixes:

- `formatVersion` 1 and `peerVersion` `"1.0.0"`;
- `listen`, the endpoint the peer bound, and `maxRemotePeers`, the most remote peers it saw
  (1 to 8; the guest is one);
- `samples`: exactly four, each a `key` of 1 to 129 bytes, a `payloadHex` of at most 12 bytes
  and an `attachmentHex` of exactly the 33-byte rmw_zenoh attachment, all as lowercase
  hexadecimal of even length.

A record with another version, another number of samples, an uppercase or odd-length hex
string, a value over a bound, or a field the contract does not name, at the top level or in a
sample, is refused: the decoder closes the field set itself, because the derived `decode`
ignores a field it does not know.

The scope is `01a138a3-e794-7a45-8e41-646ad90c966a`. The decision is
[`docs/decisions/zenoh-external-listener.md`](../../docs/decisions/zenoh-external-listener.md).
