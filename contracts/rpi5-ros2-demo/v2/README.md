# Raspberry Pi 5 ROS 2 demo contract format 2

This directory defines RP0's target-qualified acceptance contract for the first Raspberry Pi 5 ROS 2 two-node demo. `schema.zt` is the normative cross-boundary shape. `fixtures/valid.zti` freezes the exact board, firmware, media, serial path, ROS profile, transport profile, two-node workload, bounds, capabilities, evidence trace, and operator markers that later milestones must implement.

## Why format 2 exists

Format 1 made one transport family part of the contract's own shape: a `DdsProfile` record with `rtpsVersion`, `writerEntityId`, and `requiredSubmessages` fields, `maxRtpsMessageBytes`/`maxXcdrBytes` bounds, and `malformedRtps`/`malformedXcdr` markers. A userspace transport must be replaceable without a schema migration, so format 2 carries the family as data:

- `transport` is a closed discriminator naming one admitted family;
- `zenoh?` and `ddsiRtps?` are optional profile records, exactly one of which is present, and it is the one `transport` names;
- payload representation, the ROSIDL importer boundary, and the golden byte fixtures live in `ros`/`workload` rather than in either profile, because `rmw_zenoh_cpp` puts the same classic CDR on the wire that `rmw_fastrtps_cpp` does (`rmw_zenoh_cpp/src/detail/cdr.cpp` forces `CdrVersion::DDS_CDR`).

Swapping the transport is then a change of generation data. Both families remain expressible, because R1's peer target may name either and a family that cannot be expressed cannot be compared.

Format 1 and its gate are retained unchanged as the historical record of what RP0 and RP1 admitted.

## What format 2 selects

- Raspberry Pi 5 Model B revision 1.0 boards with official revision codes `c04170` or `d04170`;
- the Raspberry Pi 5 EEPROM boot flow from removable microSD, using `kernel8.img`, `config.txt`, and `bcm2712-rpi-5-b.dtb`;
- the firmware-final device tree as the memory-map/device source, the device-tree-described Arm GIC-400 and Armv8 generic timer, and UART10 on the dedicated debug header at 115200 baud;
- ROS 2 **Kilted Kaiju** semantics implemented as Slime-native ROS-compatible components through a bounded Slime RMW Profile 0 over Zenoh. Kilted rather than Jazzy because REP-2000 lists `rmw_zenoh_cpp` Tier 1 under Kilted Kaiju and Rolling and omits it from the Jazzy Jalisco middleware table;
- Zenoh wire protocol version `0x09`, one `peer`-mode session over a single TCP link with a 2-byte little-endian batch length, the four-message `INIT`/`OPEN` handshake plus `FRAME` and `CLOSE`, and `DECLARE_SUBSCRIBER`/`UNDECLARE_SUBSCRIBER`/`PUSH_PUT` as the entire declaration and data surface;
- static generation-declared discovery: no router, no multicast scouting, no gossip, and no liveliness tokens;
- the `rmw_zenoh` data key expression `<domainId>/<topic>/<typeNameOnWire>/<typeHash>`, with the DDS-mangled type name and the RIHS01 type hash, so a peer built to this contract addresses the same Zenoh resource an `rmw_zenoh` peer does;
- the fixed 33-byte per-message attachment: 8-byte little-endian sequence number, 8-byte little-endian source timestamp, and a LEB128-prefixed 16-byte source GID;
- a four-sample `slime_demo_msgs/msg/Counter` workload with the exact CDR bytes of every sample;
- a versioned semantic/wire trace whose required success sequence reaches the success marker only after all samples are decoded and validated and the sessions close.

## Validation levels

Zutai decoding validates the closed record shape. `scripts/check/check-rpi5-ros2-demo-contract-v2.py` owns semantic admission.

Every wire-level constant is **derived and compared**, never merely transcribed:

- the RIHS01 type hash is recomputed from the message's field types through the same hashable-JSON rendering `rcl_type_description_to_hashable_json` emits, and that implementation is itself first validated against the `sensor_msgs/msg/PointCloud2` hash copied from generated code into `rcl/test/rcl/test_type_hash.cpp`. This matters because REP-2011 was never merged: `ros2/rcl`'s implementation is the only specification of the hash, so a hand-copied digest would be unverifiable;
- the data key expression is recomposed the way `liveliness::TopicInfo::TopicInfo` composes it, including the DDS name mangling `_create_type_name` applies;
- each sample's CDR bytes are re-encoded from the classic `DDS_CDR` encapsulation;
- the attachment length is recomputed from `zenoh::ext::Serializer`'s rules, where a fixed-size array still carries a LEB128 length prefix.

The gate also enforces:

- `transport` names an admitted family, that family's profile is present, and no other family's profile is;
- `formatVersion`, contract id, target, board, revision set, firmware, ROS profile, and transport profile are exact closed values;
- every count, string, payload, attachment, key expression, fragment, queue, history, retry, trace, log, and capability list is bounded before activation, and the transport framing ceiling covers payload plus attachment plus key expression;
- the capability inventory covers both nodes, the transport runtime, board services, removable storage, exact stream endpoints, clocks, logs, and the trace sink without wildcard or ambient authority, and without any router, gossip, multicast, scouting, or discovery grant;
- success/failure markers are exact and distinct for denial, timeout, wrong board, wrong target, wrong transport, malformed wire framing, malformed payload, and malformed generation;
- unknown board, firmware, ROS, transport, and target identifiers are rejected independently by one-delta negative fixtures generated by the check.

No format-2 reader may select a nearby target, firmware, or profile. Unknown required values require a new contract version or a separately admitted exact profile.

## Scope boundary

RP0 fixes the demo contract; it does not claim that the kernel, transport runtime, AArch64 QEMU path, or physical board path exists yet. RP1-RP8 and R0 implement and observe those claims.

Outside Profile 0: Zenoh routers and gossip, multicast scouting, liveliness tokens and ROS graph introspection, `rmw_zenoh` peer interoperability, Zenoh queryables and the `zenoh::ext` advanced publisher/subscriber, DDS Security, wildcard key expressions, wildcard destinations, services/actions, arbitrary ROS packages, Python, filesystems, parameters, Wi-Fi, GPU, and internal-storage writes.

Interoperability with an actual `rmw_zenoh` peer is R1's target, not RP0's. The key expression, type hash, and attachment layout are frozen in the `rmw_zenoh` format so that R1 is reachable without redefining the contract, but R0 declares both ends in the generation and runs no discovery.

Run the focused gate with:

```sh
just rpi5_ros2_demo_contract_v2_check
```
