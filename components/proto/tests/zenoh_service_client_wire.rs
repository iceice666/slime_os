//! The wire records of `contracts/zenoh-service-client/v1`, encoded and decoded.
//!
//! The test prints one `[zenoh-exam] clientwire ...` line for each thing it did. The judge in
//! `scripts/lib/zenoh_client_wire_exam.py` writes the same bytes with Python's `struct` from the schema's layout, and
//! requires these lines to equal them. The values below are the judge's: every field holds a distinct nonzero pattern.

use slime_proto::zenoh_service_client_wire::{
    COMPLETION_LEN, LOAN_LEN, REQUEST_LEN, SAMPLE_HEADER_LEN, WireSampleHeader,
    WireServiceCompletion, WireServiceLoan, WireServiceRequest,
};

const MAGIC: u32 = 0x4356_535a;

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn line(text: &str) {
    println!("[zenoh-exam] clientwire {text}");
}

fn bytes(from: u8, count: usize) -> Vec<u8> {
    (0..count)
        .map(|index| from.wrapping_add(index as u8))
        .collect()
}

#[test]
fn every_record_encodes_to_the_layout_and_decodes_back() {
    let request = WireServiceRequest {
        magic: MAGIC,
        version: 1,
        op: 2,
        flags: 0x40,
        row: 0x0201,
        reserved: [0xa1, 0xa2],
        sequence: -2,
        reserved2: bytes(0xb0, 12).try_into().unwrap(),
    };
    let completion = WireServiceCompletion {
        magic: MAGIC,
        version: 1,
        op: 3,
        item: 1,
        status: -4,
        row: 0x0102,
        reserved: [0xc1, 0xc2],
        length: 0x1234,
        flags: 1,
    };
    let header = WireSampleHeader {
        sequence: 0x1112_1314_1516_1718,
        timestamp: -1000,
    };
    let loan = WireServiceLoan {
        magic: MAGIC,
        version: 1,
        role: 2,
        reserved0: [0xd1],
        buffer: 0x2122_2324_2526_2728,
        lease: 0x3132_3334_3536_3738,
        length: 4096,
        reserved: bytes(0xe0, 32).try_into().unwrap(),
    };

    let encoded: [(&str, Vec<u8>); 4] = [
        ("request", request.encode().to_vec()),
        ("completion", completion.encode().to_vec()),
        ("header", header.encode().to_vec()),
        ("loan", loan.encode().to_vec()),
    ];
    for (name, bytes) in &encoded {
        line(&format!("size {name} {}", bytes.len()));
    }
    for (name, bytes) in &encoded {
        line(&format!("encode {name} {}", hex(bytes)));
    }
    assert_eq!(
        [
            encoded[0].1.len(),
            encoded[1].1.len(),
            encoded[2].1.len(),
            encoded[3].1.len()
        ],
        [REQUEST_LEN, COMPLETION_LEN, SAMPLE_HEADER_LEN, LOAN_LEN]
    );

    assert_eq!(WireServiceRequest::decode(&encoded[0].1), Some(request));
    line("roundtrip request ok");
    assert_eq!(
        WireServiceCompletion::decode(&encoded[1].1),
        Some(completion)
    );
    line("roundtrip completion ok");
    assert_eq!(WireSampleHeader::decode(&encoded[2].1), Some(header));
    line("roundtrip header ok");
    assert_eq!(WireServiceLoan::decode(&encoded[3].1), Some(loan));
    line("roundtrip loan ok");

    // A buffer one byte short is refused, and a longer one is read from its start.
    assert!(WireServiceRequest::decode(&encoded[0].1[..REQUEST_LEN - 1]).is_none());
    line("short request refused");
    assert!(WireServiceCompletion::decode(&encoded[1].1[..COMPLETION_LEN - 1]).is_none());
    line("short completion refused");
    assert!(WireSampleHeader::decode(&encoded[2].1[..SAMPLE_HEADER_LEN - 1]).is_none());
    line("short header refused");
    assert!(WireServiceLoan::decode(&encoded[3].1[..LOAN_LEN - 1]).is_none());
    line("short loan refused");
    let mut longer = encoded[0].1.clone();
    longer.push(0xff);
    assert_eq!(WireServiceRequest::decode(&longer), Some(request));

    line("ok every-record-encodes-to-the-layout-and-decodes-back");
}
