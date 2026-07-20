use slime_os_kernel::{acpi, boot, generation, sha256};

#[test_case]
fn trivial_assertion() {
    assert_eq!([1, 1].len(), 2);
}

#[test_case]
fn nonzero_assertion() {
    assert_ne!(1, 0);
}

#[test_case]
fn generation_decodes_and_resolves_vertical_slice() {
    let bytes = boot::generation();
    let decoded = generation::decode(bytes).expect("generation must decode");
    assert_eq!(decoded.number, 1);
    assert_eq!(decoded.target, "x86_64-qemu-virtio");
    assert!(decoded.parent.is_some());
    assert_eq!(decoded.component(decoded.bootstrap).unwrap().name, "init");
    for name in ["init", "console", "dango", "sysinfo", "echo-agent"] {
        assert!(decoded.component_bytes(name).is_some());
    }
    for name in [
        "console-output",
        "system-information",
        "echo-request",
        "echo-reply",
    ] {
        assert!(decoded.grant_named(name).is_some());
    }
    assert_eq!(decoded.state_count(), 5);
    let policies: alloc::vec::Vec<u32> = (0..decoded.state_count())
        .map(|index| decoded.state(index).unwrap().policy)
        .collect();
    assert_eq!(
        policies,
        alloc::vec![
            boot_contracts::generation::POLICY_PRESERVE,
            boot_contracts::generation::POLICY_EPHEMERAL,
            boot_contracts::generation::POLICY_IMMUTABLE,
            boot_contracts::generation::POLICY_DISCARD_ON_ROLLBACK,
            boot_contracts::generation::POLICY_SNAPSHOT_BEFORE_UPGRADE,
        ],
    );
    assert_eq!(decoded.boot_attempts, 3);
    assert_eq!(decoded.health_count(), 5);
}

#[test_case]
fn sha256_matches_standard_vector() {
    assert_eq!(
        sha256::digest(b"abc"),
        [
            0xba, 0x78, 0x16, 0xbf, 0x8f, 0x01, 0xcf, 0xea, 0x41, 0x41, 0x40, 0xde, 0x5d, 0xae,
            0x22, 0x23, 0xb0, 0x03, 0x61, 0xa3, 0x96, 0x17, 0x7a, 0x9c, 0xb4, 0x10, 0xff, 0x61,
            0xf2, 0x00, 0x15, 0xad,
        ],
    );
}

#[test_case]
fn acpi_rsdp_v2_checksums_and_addresses() {
    let mut rsdp = [0u8; 36];
    rsdp[..8].copy_from_slice(b"RSD PTR ");
    rsdp[9..15].copy_from_slice(b"SLIME ");
    rsdp[15] = 2;
    rsdp[16..20].copy_from_slice(&0x1234_5000u32.to_le_bytes());
    let rsdp_len = rsdp.len() as u32;
    rsdp[20..24].copy_from_slice(&rsdp_len.to_le_bytes());
    rsdp[24..32].copy_from_slice(&0x1234_5678_9000u64.to_le_bytes());
    let legacy_sum = rsdp[..20]
        .iter()
        .fold(0u8, |sum, byte| sum.wrapping_add(*byte));
    rsdp[8] = 0u8.wrapping_sub(legacy_sum);
    let extended_sum = rsdp.iter().fold(0u8, |sum, byte| sum.wrapping_add(*byte));
    rsdp[32] = 0u8.wrapping_sub(extended_sum);

    let parsed = acpi::parse_rsdp(&rsdp).expect("valid RSDP must parse");
    assert_eq!(parsed.revision, 2);
    assert_eq!(parsed.rsdt_address, 0x1234_5000);
    assert_eq!(parsed.xsdt_address, 0x1234_5678_9000);

    rsdp[24] ^= 1;
    assert_eq!(
        acpi::parse_rsdp(&rsdp),
        Err(acpi::AcpiError::InvalidChecksum)
    );
}

#[test_case]
fn acpi_s5_package_decodes() {
    let aml = [
        0x08, b'\\', b'_', b'S', b'5', b'_', 0x12, 0x06, 0x02, 0x0a, 0x05, 0x0a, 0x05,
    ];
    assert_eq!(acpi::parse_s5_aml(&aml), Some((5, 5)));
}
