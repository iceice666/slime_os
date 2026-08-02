//! Component image contract tests (`contracts/component/v1`): the kernel
//! decoder accepts well-formed images and rejects every malformed class with
//! a structured error. Images are built through the generated wire bindings
//! so the tests pin the same layout the contract owns.

#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(slime_os_kernel::test_runner)]
#![reexport_test_harness_main = "test_main"]

extern crate alloc;

use alloc::vec::Vec;

use boot_contracts::target_profile::TargetProfile;
use slime_os_kernel::component::{
    self, DEFAULT_STACK_BYTES, FORMAT_VERSION, HEADER_LEN, IMAGE_MAGIC, ImageError,
    KERNEL_ABI_VERSION, LEGACY_FORMAT_VERSION, LEGACY_HEADER_LEN, LEGACY_IMAGE_MAGIC, MAX_SEGMENTS,
    MAX_STACK_BYTES, SEGMENT_FLAG_EXEC, SEGMENT_FLAG_WRITE, WireImageHeader, WireSegmentRecord,
};
use slime_os_kernel::{gdt, interrupts, memory};

#[unsafe(no_mangle)]
pub unsafe extern "C" fn _start() -> ! {
    slime_os_kernel::limine::ensure_linked();
    unsafe { slime_os_kernel::boot::init_from_limine() };
    gdt::init();
    interrupts::init();
    memory::init();
    test_main();
    slime_os_kernel::hlt_loop()
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    slime_os_kernel::test_panic_handler(info)
}

fn header(count: u16, entry: u32, stack: u32) -> WireImageHeader {
    let profile = TargetProfile::legacy().expect("admitted profile");
    WireImageHeader {
        magic: IMAGE_MAGIC,
        format_version: FORMAT_VERSION,
        header_size: HEADER_LEN as u32,
        kernel_abi: KERNEL_ABI_VERSION,
        architecture: profile.architecture,
        abi: profile.abi,
        page_profile: profile.page_profile,
        entry_offset: entry,
        segment_count: count,
        reserved: 0,
        stack_bytes: stack,
        target_profile: profile.id,
        required_features: profile.required_features,
    }
}

fn segment(
    vaddr: u32,
    mem_len: u32,
    file_offset: u32,
    file_len: u32,
    flags: u16,
) -> WireSegmentRecord {
    WireSegmentRecord {
        vaddr_offset: vaddr,
        mem_len,
        file_offset,
        file_len,
        flags,
        reserved: 0,
    }
}

fn image(header: &WireImageHeader, segments: &[WireSegmentRecord], payload: &[u8]) -> Vec<u8> {
    let mut blob = Vec::new();
    blob.extend_from_slice(&header.encode());
    for segment in segments {
        blob.extend_from_slice(&segment.encode());
    }
    blob.extend_from_slice(payload);
    blob
}

fn legacy_image(reserved: u16) -> Vec<u8> {
    let mut blob = Vec::new();
    let mut header = [0u8; LEGACY_HEADER_LEN];
    header[..8].copy_from_slice(&LEGACY_IMAGE_MAGIC.to_le_bytes());
    header[8..12].copy_from_slice(&LEGACY_FORMAT_VERSION.to_le_bytes());
    header[12..16].copy_from_slice(&(LEGACY_HEADER_LEN as u32).to_le_bytes());
    header[16..20].copy_from_slice(&KERNEL_ABI_VERSION.to_le_bytes());
    header[20..24].copy_from_slice(&0u32.to_le_bytes());
    header[24..26].copy_from_slice(&1u16.to_le_bytes());
    header[26..28].copy_from_slice(&reserved.to_le_bytes());
    header[28..32].copy_from_slice(&DEFAULT_STACK_BYTES.to_le_bytes());
    blob.extend_from_slice(&header);
    blob.extend_from_slice(&segment(0, 0x100, 0, 0x100, SEGMENT_FLAG_EXEC).encode());
    blob.extend_from_slice(&[0x90; 0x100]);
    blob
}

#[test_case]
fn single_segment_image_decodes() {
    let blob = image(
        &header(1, 0, DEFAULT_STACK_BYTES),
        &[segment(0, 0x100, 0, 0x100, SEGMENT_FLAG_EXEC)],
        &[0x90; 0x100],
    );
    let decoded = component::decode(&blob).expect("valid image must decode");
    assert_eq!(decoded.entry_offset, 0);
    assert_eq!(decoded.stack_bytes, DEFAULT_STACK_BYTES);
    assert_eq!(decoded.segments.len(), 1);
    assert!(decoded.segments[0].executable());
    assert!(!decoded.segments[0].writable());
    assert_eq!(decoded.segment_bytes(&decoded.segments[0]).len(), 0x100);
}

#[test_case]
fn multi_segment_image_decodes_with_bss_tail() {
    let mut payload = Vec::new();
    payload.extend_from_slice(&[0xcc; 0x100]);
    payload.extend_from_slice(&[0xaa; 0x80]);
    payload.extend_from_slice(&[0xbb; 0x40]);
    let blob = image(
        &header(3, 0, DEFAULT_STACK_BYTES),
        &[
            segment(0, 0x100, 0, 0x100, SEGMENT_FLAG_EXEC),
            segment(0x2000, 0x80, 0x100, 0x80, 0),
            segment(0x4000, 0x200, 0x180, 0x40, SEGMENT_FLAG_WRITE),
        ],
        &payload,
    );
    let decoded = component::decode(&blob).expect("valid image must decode");
    assert_eq!(decoded.segments.len(), 3);
    let bss = &decoded.segments[2];
    assert!(bss.writable());
    assert!(!bss.executable());
    assert_eq!(bss.mem_len, 0x200);
    assert_eq!(decoded.segment_bytes(bss).len(), 0x40);
}

#[test_case]
fn rejects_truncated_inputs() {
    assert!(matches!(component::decode(&[]), Err(ImageError::Truncated)));
    // Shorter than the smallest header any accepted revision uses, so the
    // revision cannot even be established.
    assert!(matches!(
        component::decode(&[0u8; LEGACY_HEADER_LEN - 1]),
        Err(ImageError::Truncated)
    ));
    // A well-formed header identifying the current revision, cut short of that
    // revision's own header length.
    let full = header(1, 0, DEFAULT_STACK_BYTES).encode();
    assert!(matches!(
        component::decode(&full[..HEADER_LEN - 1]),
        Err(ImageError::Truncated)
    ));
    // Two declared segments but only one record present.
    let blob = image(
        &header(2, 0, DEFAULT_STACK_BYTES),
        &[segment(0, 0x100, 0, 0, SEGMENT_FLAG_EXEC)],
        &[],
    );
    assert!(matches!(
        component::decode(&blob),
        Err(ImageError::Truncated)
    ));
}

#[test_case]
fn rejects_bad_magic() {
    let mut header = header(1, 0, DEFAULT_STACK_BYTES);
    header.magic ^= 1;
    let blob = image(
        &header,
        &[segment(0, 0x100, 0, 0x100, SEGMENT_FLAG_EXEC)],
        &[0x90; 0x100],
    );
    assert!(matches!(
        component::decode(&blob),
        Err(ImageError::BadMagic)
    ));
}

#[test_case]
fn rejects_unsupported_version() {
    // A known magic paired with a version that revision never used, in both
    // directions, so neither revision's tag can smuggle the other's layout.
    for (magic, version) in [
        (IMAGE_MAGIC, FORMAT_VERSION + 1),
        (IMAGE_MAGIC, LEGACY_FORMAT_VERSION),
        (LEGACY_IMAGE_MAGIC, FORMAT_VERSION),
    ] {
        let mut header = header(1, 0, DEFAULT_STACK_BYTES);
        header.magic = magic;
        header.format_version = version;
        let blob = image(
            &header,
            &[segment(0, 0x100, 0, 0x100, SEGMENT_FLAG_EXEC)],
            &[0x90; 0x100],
        );
        assert!(matches!(
            component::decode(&blob),
            Err(ImageError::UnsupportedVersion)
        ));
    }
}

#[test_case]
fn rejects_header_size_disagreeing_with_the_revision() {
    let mut header = header(1, 0, DEFAULT_STACK_BYTES);
    header.header_size = LEGACY_HEADER_LEN as u32;
    let blob = image(
        &header,
        &[segment(0, 0x100, 0, 0x100, SEGMENT_FLAG_EXEC)],
        &[0x90; 0x100],
    );
    assert!(matches!(
        component::decode(&blob),
        Err(ImageError::BadHeader)
    ));
}

#[test_case]
fn rejects_abi_mismatch() {
    let mut header = header(1, 0, DEFAULT_STACK_BYTES);
    header.kernel_abi = KERNEL_ABI_VERSION + 1;
    let blob = image(
        &header,
        &[segment(0, 0x100, 0, 0x100, SEGMENT_FLAG_EXEC)],
        &[0x90; 0x100],
    );
    assert!(matches!(
        component::decode(&blob),
        Err(ImageError::AbiMismatch)
    ));
}

#[test_case]
fn retained_v1_reserved_field_must_be_zero() {
    assert!(component::decode(&legacy_image(0)).is_ok());
    assert!(matches!(
        component::decode(&legacy_image(1)),
        Err(ImageError::BadFlags)
    ));
}

#[test_case]
fn rejects_segment_count_out_of_bounds() {
    for count in [0, MAX_SEGMENTS + 1] {
        let blob = image(&header(count, 0, DEFAULT_STACK_BYTES), &[], &[]);
        assert!(matches!(
            component::decode(&blob),
            Err(ImageError::BadSegmentCount)
        ));
    }
}

#[test_case]
fn rejects_bad_stack_sizes() {
    for stack in [0, 4095, MAX_STACK_BYTES + 4096] {
        let blob = image(
            &header(1, 0, stack),
            &[segment(0, 0x100, 0, 0x100, SEGMENT_FLAG_EXEC)],
            &[0x90; 0x100],
        );
        assert!(matches!(
            component::decode(&blob),
            Err(ImageError::BadStack)
        ));
    }
}

#[test_case]
fn rejects_writable_executable_and_unknown_flags() {
    for flags in [SEGMENT_FLAG_WRITE | SEGMENT_FLAG_EXEC, 0x4, 0x8000] {
        let blob = image(
            &header(1, 0, DEFAULT_STACK_BYTES),
            &[segment(0, 0x100, 0, 0x100, flags)],
            &[0x90; 0x100],
        );
        assert!(matches!(
            component::decode(&blob),
            Err(ImageError::BadFlags)
        ));
    }
}

#[test_case]
fn rejects_malformed_segments() {
    // Page-misaligned load offset.
    let blob = image(
        &header(1, 0x100, DEFAULT_STACK_BYTES),
        &[segment(0x100, 0x100, 0, 0x100, SEGMENT_FLAG_EXEC)],
        &[0x90; 0x100],
    );
    assert!(matches!(
        component::decode(&blob),
        Err(ImageError::BadSegment)
    ));
    // Zero-length memory range.
    let blob = image(
        &header(1, 0, DEFAULT_STACK_BYTES),
        &[segment(0, 0, 0, 0, SEGMENT_FLAG_EXEC)],
        &[],
    );
    assert!(matches!(
        component::decode(&blob),
        Err(ImageError::BadSegment)
    ));
    // File bytes longer than the memory range.
    let blob = image(
        &header(1, 0, DEFAULT_STACK_BYTES),
        &[segment(0, 0x40, 0, 0x80, SEGMENT_FLAG_EXEC)],
        &[0x90; 0x80],
    );
    assert!(matches!(
        component::decode(&blob),
        Err(ImageError::BadSegment)
    ));
    // Unsorted load offsets.
    let reversed = image(
        &header(2, 0, DEFAULT_STACK_BYTES),
        &[
            segment(0x2000, 0x100, 0, 0, SEGMENT_FLAG_EXEC),
            segment(0, 0x100, 0, 0, SEGMENT_FLAG_EXEC),
        ],
        &[],
    );
    assert!(matches!(
        component::decode(&reversed),
        Err(ImageError::BadSegment)
    ));
    // Sorted but overlapping memory ranges.
    let blob = image(
        &header(2, 0, DEFAULT_STACK_BYTES),
        &[
            segment(0, 0x3000, 0, 0, SEGMENT_FLAG_EXEC),
            segment(0x2000, 0x100, 0, 0, SEGMENT_FLAG_EXEC),
        ],
        &[],
    );
    assert!(matches!(
        component::decode(&blob),
        Err(ImageError::BadSegment)
    ));
}

#[test_case]
fn rejects_file_range_outside_blob() {
    let blob = image(
        &header(1, 0, DEFAULT_STACK_BYTES),
        &[segment(0, 0x100, 0x10, 0x100, SEGMENT_FLAG_EXEC)],
        &[0x90; 0x80],
    );
    assert!(matches!(
        component::decode(&blob),
        Err(ImageError::BadFileRange)
    ));
}

#[test_case]
fn rejects_entry_outside_executable_segment() {
    // Past every segment.
    let blob = image(
        &header(1, 0x2000, DEFAULT_STACK_BYTES),
        &[segment(0, 0x100, 0, 0x100, SEGMENT_FLAG_EXEC)],
        &[0x90; 0x100],
    );
    assert!(matches!(
        component::decode(&blob),
        Err(ImageError::BadEntry)
    ));
    // Inside a non-executable segment.
    let blob = image(
        &header(2, 0x100, DEFAULT_STACK_BYTES),
        &[
            segment(0, 0x2000, 0, 0, 0),
            segment(0x2000, 0x100, 0, 0x100, SEGMENT_FLAG_EXEC),
        ],
        &[0x90; 0x100],
    );
    assert!(matches!(
        component::decode(&blob),
        Err(ImageError::BadEntry)
    ));
}

#[test_case]
fn rejects_image_above_footprint_bound() {
    let blob = image(
        &header(1, 0, DEFAULT_STACK_BYTES),
        &[segment(
            0,
            (slime_os_kernel::component::MAX_IMAGE_BYTES + 4096) as u32,
            0,
            0,
            SEGMENT_FLAG_EXEC,
        )],
        &[],
    );
    assert!(matches!(
        component::decode(&blob),
        Err(ImageError::ImageTooLarge)
    ));
}
