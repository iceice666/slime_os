---
schema: work-item/v2
id: 01a17dad-fa21-739d-a55e-ef59fa3ce0ea
kind: task
state: active
created: 2026-10-27T14:05:16+00:00
updated: 2026-10-28T04:22:42+00:00
parent: 01a0f5fa-f2c4-71cf-962b-c62cbea2ef68
depends:
  - 01a15d1f-26d2-7653-924b-a3f113a718e6
devloop:
  schema: dev-spec/v1
  recordSchema: devloop-consumer/v1
  bodyProfile: fenced-zti/v1
  helper: devloop-helpers/v1:93745c5aa610e8859ed5f5d937e47d55d36ac612cc60b3a22141fa6763fe1c23
  requirementsDigest: 3026946fa200c9a51dcbfd07c5c9d66ca605e517e5d16e76baec5d8d90185fa2
  admission: slime-zenoh-service-client-20261027d
  epoch: e345bf9a-b123-4acb-8afa-74a1e4b63208
  evidence:
  - acceptance: cases-observed
    gate: just-observations
    mode: automated
    identity:
      requirements: 3026946fa200c9a51dcbfd07c5c9d66ca605e517e5d16e76baec5d8d90185fa2
      helper: devloop-helpers/v1:93745c5aa610e8859ed5f5d937e47d55d36ac612cc60b3a22141fa6763fe1c23
      code: cf1483825f759f472d346036acb14f5a4f6ad6ccc0a8db112c11ea5d5d34d591
      policy: 7e24d538c6090cd8b45edf0615526980d9ddf09df9abc03f29855b2e92c4bd29
      inputs: e19f235eda94f63be2eae450fff17588b290d1ef11f9a4795ca501beb64b0b1d
      target: x86_64-linux
      image: zenoh-client-main-e817e3e4abbd
      epoch: e345bf9a-b123-4acb-8afa-74a1e4b63208
    observedAt: 1793161389
    expiresAt: 1793247789
    passed: true
    observer: argv:["python3", "scripts/check/devloop-gate.py", "just-observations"]
    observations:
    - id: casesObserved
      value:
        kind: int
        intValue: 24
        boolValue: false
        textValue: ''
    - id: negativeControlsRefused
      value:
        kind: int
        intValue: 80
        boolValue: false
        textValue: ''
    - id: passed
      value:
        kind: bool
        intValue: 0
        boolValue: true
        textValue: ''
    - id: transcriptDigest
      value:
        kind: text
        intValue: 0
        boolValue: false
        textValue: b106c5b31c1c47f23b41d62d4eadbe34d270fa43db36440839cec2feb8552f3e
  - acceptance: controls-refused
    gate: just-observations
    mode: automated
    identity:
      requirements: 3026946fa200c9a51dcbfd07c5c9d66ca605e517e5d16e76baec5d8d90185fa2
      helper: devloop-helpers/v1:93745c5aa610e8859ed5f5d937e47d55d36ac612cc60b3a22141fa6763fe1c23
      code: cf1483825f759f472d346036acb14f5a4f6ad6ccc0a8db112c11ea5d5d34d591
      policy: 7e24d538c6090cd8b45edf0615526980d9ddf09df9abc03f29855b2e92c4bd29
      inputs: e19f235eda94f63be2eae450fff17588b290d1ef11f9a4795ca501beb64b0b1d
      target: x86_64-linux
      image: zenoh-client-main-e817e3e4abbd
      epoch: e345bf9a-b123-4acb-8afa-74a1e4b63208
    observedAt: 1793161405
    expiresAt: 1793247805
    passed: true
    observer: argv:["python3", "scripts/check/devloop-gate.py", "just-observations"]
    observations:
    - id: casesObserved
      value:
        kind: int
        intValue: 24
        boolValue: false
        textValue: ''
    - id: negativeControlsRefused
      value:
        kind: int
        intValue: 80
        boolValue: false
        textValue: ''
    - id: passed
      value:
        kind: bool
        intValue: 0
        boolValue: true
        textValue: ''
    - id: transcriptDigest
      value:
        kind: text
        intValue: 0
        boolValue: false
        textValue: b106c5b31c1c47f23b41d62d4eadbe34d270fa43db36440839cec2feb8552f3e
  - acceptance: recipe-passed
    gate: just-observations
    mode: automated
    identity:
      requirements: 3026946fa200c9a51dcbfd07c5c9d66ca605e517e5d16e76baec5d8d90185fa2
      helper: devloop-helpers/v1:93745c5aa610e8859ed5f5d937e47d55d36ac612cc60b3a22141fa6763fe1c23
      code: cf1483825f759f472d346036acb14f5a4f6ad6ccc0a8db112c11ea5d5d34d591
      policy: 7e24d538c6090cd8b45edf0615526980d9ddf09df9abc03f29855b2e92c4bd29
      inputs: e19f235eda94f63be2eae450fff17588b290d1ef11f9a4795ca501beb64b0b1d
      target: x86_64-linux
      image: zenoh-client-main-e817e3e4abbd
      epoch: e345bf9a-b123-4acb-8afa-74a1e4b63208
    observedAt: 1793161418
    expiresAt: 1793247818
    passed: true
    observer: argv:["python3", "scripts/check/devloop-gate.py", "just-observations"]
    observations:
    - id: casesObserved
      value:
        kind: int
        intValue: 24
        boolValue: false
        textValue: ''
    - id: negativeControlsRefused
      value:
        kind: int
        intValue: 80
        boolValue: false
        textValue: ''
    - id: passed
      value:
        kind: bool
        intValue: 0
        boolValue: true
        textValue: ''
    - id: transcriptDigest
      value:
        kind: text
        intValue: 0
        boolValue: false
        textValue: b106c5b31c1c47f23b41d62d4eadbe34d270fa43db36440839cec2feb8552f3e
---

# Fix the Zenoh service client protocol

```zti
{
  schema = "dev-spec/v1";
  problem = "A local holder has no way to speak to the Zenoh service. No contract says what a request names, what a completion reports, or what a take returns. Four rules are undecided. A request names a grant row by its index and never a key, because a key string in a request grants nothing. The service learns the holder from the endpoint the request arrives on, so no field names a holder. A request and a completion must fit the IO0 payloads of 56 and 24 bytes. A sample header and its payload must fit the data page. This item fixes those rules as a Zutai schema and judges it before any client library or component exists.";
  scope = [
    "Add `contracts/zenoh-service-client/v1/schema.zt` and its README. The schema states constants and four wire layouts. They are the request, the completion, the sample header and the loan descriptor.";
    "A request is 32 bytes and a completion is 24 bytes. The layouts add up to those sizes and list the fields of their record types in order.";
    "The operations are attach, publish, take, detach and abort. A take returns none, a sample or a loss count. A publish completion may set one flag, which reports that the service sent a copy to the peer.";
    "The sample header is 16 bytes and the largest payload is 4080 bytes. Together they fill the 4096-byte data page. A holder with a larger declared sample is outside this version.";
    "Add `scripts/lib/zenoh_client_exam.py`. It reads the schema as text and derives every fact. It checks the layouts against `contracts/io-queue/v1` and the sample size against `contracts/zenoh-service-grants/v1`.";
    "Add `scripts/generate/generate-zenoh-service-client-bindings.py` and the recipe `just zenoh_service_client_gen`. They render every integer constant of the schema as `components/proto/src/zenoh_service_client.rs`. The exam requires the committed file to equal that text byte for byte.";
    "Add `check-zenoh-profile0.py --slice client`, run as `just zenoh_service_client_check`.";
    "The implementation PR owns the generated Rust file.";
  ];
  nonGoals = [
    "Generated encoders and decoders for the layouts. The schema has no `gen_rust.zt`, which is product code. A later planning step owns the wire records and their bindings.";
    "The client library, the grants path, the service component and a booted composition. Later items own them.";
    "A holder with a sample larger than 4080 bytes. The grants contract allows larger samples and this version does not serve them.";
    "A request that names a key, a topic or a type.";
    "Any product change in the planning PR.";
  ];
  requirements = [
    {
      id = "constants-are-pinned";
      statement = "The schema states format version 1 and the magic for `ZSVC`, as a little-endian integer. It states four queue slots, one ring page and one data page of 4096 bytes each. A success status is zero.";
      acceptance = [
        "cases-observed";
      ];
    };
    {
      id = "layouts-add-up";
      statement = "The request, completion, sample header and loan layouts add up to 32, 24, 16 and 64 bytes. Each layout equals a field list that the judge holds apart from the schema. Each layout lists the fields of its record type in order. Every record field has the type `Int`. A commented field is not a field.";
      acceptance = [
        "cases-observed";
      ];
    };
    {
      id = "a-request-and-a-completion-fit-io0";
      statement = "A request of 32 bytes is within the IO0 request payload of 56. A completion of 24 bytes is within the IO0 completion payload of 24.";
      acceptance = [
        "cases-observed";
      ];
    };
    {
      id = "no-field-names-what-the-endpoint-decides";
      statement = "No field name contains key, holder, session, peer, link, topic or name. A request names a grant row by its index.";
      acceptance = [
        "cases-observed";
      ];
    };
    {
      id = "signs-and-reserved-bytes";
      statement = "The sequence, the timestamp and the status are signed. A reserved field is an unsigned byte array and a named field is not a byte array.";
      acceptance = [
        "cases-observed";
      ];
    };
    {
      id = "the-row-index-is-sized";
      statement = "The request and the completion carry the row in two bytes, and `maxRows` is 65535.";
      acceptance = [
        "cases-observed";
      ];
    };
    {
      id = "the-data-page-is-filled";
      statement = "The sample header of 16 bytes and the largest payload of 4080 bytes fill the data page of 4096 bytes. The largest payload is at most the grants contract's sample limit.";
      acceptance = [
        "cases-observed";
      ];
    };
    {
      id = "values-are-distinct-and-signed";
      statement = "The five operations, the six statuses, the three items and the two loan roles are distinct within their group. Operations are positive and failures are negative. The item none is zero. Each value fits the width and sign of the field that carries it.";
      acceptance = [
        "cases-observed";
      ];
    };
    {
      id = "flags-are-known";
      statement = "The known completion flags equal the sum of the flags, and each flag is one bit.";
      acceptance = [
        "cases-observed";
      ];
    };
    {
      id = "the-honest-holder-is-served";
      statement = "A holder with five rows and samples of 12 bytes is served by this protocol. A sample of 4081 bytes, a sample of no bytes, a holder of no rows and a holder of 65536 rows are each refused.";
      acceptance = [
        "cases-observed";
      ];
    };
    {
      id = "the-generated-file-is-exact";
      statement = "The committed Rust constants file equals the generator's output byte for byte. Ten edits are each refused. They change a value, drop or add a constant, swap two, or change a type. They also drop the header, append a comment, use CRLF, drop the final newline or empty the file.";
      acceptance = [
        "cases-observed";
      ];
    };
    {
      id = "judge-holds";
      statement = "The judge refuses 80 corruptions before it checks the generated file. They are 66 edits of the schema, 4 holders that the protocol cannot serve and 10 edits of the generated file.";
      acceptance = [
        "controls-refused";
      ];
    };
    {
      id = "recipe-passes";
      statement = "`just zenoh_service_client_check` exits zero and reports its observations through the gate adapter.";
      acceptance = [
        "recipe-passed";
      ];
    };
  ];
  predicates = [
    {
      id = "all-cases-observed";
      kind = "equal";
      observation = "casesObserved";
      expected = {
        kind = "int";
        intValue = 24;
        boolValue = false;
        textValue = "";
      };
      lower = 0;
      upper = 0;
      lowerInclusive = false;
      upperInclusive = false;
      members = [];
    };
    {
      id = "exactly-80-controls";
      kind = "equal";
      observation = "negativeControlsRefused";
      expected = {
        kind = "int";
        intValue = 80;
        boolValue = false;
        textValue = "";
      };
      lower = 0;
      upper = 0;
      lowerInclusive = false;
      upperInclusive = false;
      members = [];
    };
    {
      id = "recipe-passed";
      kind = "equal";
      observation = "passed";
      expected = {
        kind = "bool";
        intValue = 0;
        boolValue = true;
        textValue = "";
      };
      lower = 0;
      upper = 0;
      lowerInclusive = false;
      upperInclusive = false;
      members = [];
    };
    {
      id = "observed-and-passed";
      kind = "all";
      observation = "";
      expected = {
        kind = "bool";
        intValue = 0;
        boolValue = false;
        textValue = "";
      };
      lower = 0;
      upper = 0;
      lowerInclusive = false;
      upperInclusive = false;
      members = [
        "all-cases-observed";
        "recipe-passed";
      ];
    };
  ];
  acceptance = [
    {
      id = "cases-observed";
      rationale = "The checker counts 24 facts that the judge derives from the schema text. They are constants, layouts, IO0 fit, signs, the row index, the data page, distinct values, flags and a served holder. The count is exact. The judge reads no Rust, so these facts come from the schema and the two contracts it is checked against.";
      gate = "just-observations";
      mode = "automated";
      mandatory = true;
      predicate = "observed-and-passed";
    };
    {
      id = "controls-refused";
      rationale = "The checker refuses exactly 80 corruptions before it checks the generated file. They are 66 edits of the schema, 4 holders the protocol cannot serve and 10 edits of the generated file. A dropped or invented control changes the count and re-admits the item.";
      gate = "just-observations";
      mode = "automated";
      mandatory = true;
      predicate = "exactly-80-controls";
    };
    {
      id = "recipe-passed";
      rationale = "The gate adapter reports the recipe's exit code, so the checker cannot vouch for itself. The recipe fails until the generated constants file lands.";
      gate = "just-observations";
      mode = "automated";
      mandatory = true;
      predicate = "recipe-passed";
    };
  ];
}
```
