# Zenoh service client protocol

`v1/schema.zt` owns the constants and the wire layouts that a local holder and the Zenoh service share. The Zenoh service has a decision record at [`docs/decisions/zenoh-service.md`](../../docs/decisions/zenoh-service.md).

This version states constants and layouts only. It has no `gen_rust.zt`, so no encoder or decoder is generated yet. A later planning step owns them. The implementation PR for this version generates `components/proto/src/zenoh_service_client.rs` with `just zenoh_service_client_gen`. That file holds the constants.

## What a request names

A request names a grant row by its index in the holder's own list. It names no key, holder, session, link or peer. The service learns the holder from the endpoint that the request arrives on. A key string in a request would grant nothing, so the protocol has no field for one.

## Provisioning

The pattern follows [`network-service`](../network-service/README.md). The holder resolves a nontransferable control endpoint and a provisioning endpoint. At attach, the service delegates two writable shared-buffer loans on the provisioning endpoint. The first is one IO0 ring page with four slots. The second is one 4096-byte data page. A 64-byte loan descriptor accompanies each loan, and `role` tells the ring from the data page.

The attach completion reports the number of rows that the holder has. A request after that uses the IO0 ring.

## Operations

- `opAttach`, `opDetach` and `opAbort` use the control endpoint.
- `opPublish` carries the payload in the data page and the holder's own sequence number in the request. The service stamps the timestamp from its own clock. The completion reports how many local holders the sample reached. It sets `flagPeerSent` when the service sent a copy to the peer.
- `opTake` returns the holder's next item. A sample returns a 16-byte header, then the payload, in the data page. A loss returns the count of lost samples in `length`. A take on an empty ring completes with `itemNone`.

The IO0 request payload is 56 bytes and the completion payload is 24 bytes. A request is 32 bytes and a completion is 24 bytes. The payload of a sample therefore travels in the data page.

## Limits

A sample payload is at most 4080 bytes, because the header and the payload fill the data page. The grants contract allows larger samples. A holder with a larger declared sample is outside this version.

The adapter has one outstanding request, as the network service adapter has. A missing or malformed completion poisons the session.

## Judge

`scripts/lib/zenoh_client_exam.py` reads this schema as text and derives every fact. It checks the layouts against `contracts/io-queue/v1` and `contracts/zenoh-service-grants/v1`. Run it with `just zenoh_service_client_check`.
