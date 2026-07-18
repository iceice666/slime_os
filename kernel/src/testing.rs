use slime_os_kernel::{generation, limine, sha256};

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
    let bytes = limine::generation_module();
    let decoded = generation::decode(bytes).expect("generation must decode");
    assert_eq!(decoded.number, 1);
    assert_eq!(decoded.components[decoded.bootstrap].name, "init");
    for name in ["init", "console", "dango", "sysinfo", "echo-agent"] {
        assert!(decoded.component_bytes(name).is_some());
    }
    for name in [
        "console-output",
        "system-information",
        "echo-request",
        "echo-reply",
    ] {
        assert!(decoded.grant(name).is_some());
    }
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
