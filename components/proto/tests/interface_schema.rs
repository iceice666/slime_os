use core::mem::size_of;

use slime_proto::interface_schema::{
    BoundedSequence, Call, Operation, Stream, navigation_operation, parameter_call,
    telemetry_stream,
};
use slime_proto::sample_descriptor::{
    CAPABILITY_KIND_LOAN, FLAG_LAST, FORMAT_VERSION, SAMPLE_DESCRIPTOR_MAGIC, WireSampleDescriptor,
};
use slime_proto::valid_sample_descriptor;

#[test]
fn native_contract_markers_are_zero_sized_and_sequences_stay_bounded() {
    assert_eq!(size_of::<Stream<telemetry_stream::TelemetrySample>>(), 0);
    assert_eq!(
        size_of::<Call<parameter_call::ParameterRequest, parameter_call::ParameterReply>>(),
        0
    );
    assert_eq!(
        size_of::<
            Operation<
                navigation_operation::NavigationGoal,
                navigation_operation::NavigationFeedback,
                navigation_operation::NavigationResult,
            >,
        >(),
        0
    );

    let readings = BoundedSequence::<i32, 8>::new(3, [1, 2, 3, 0, 0, 0, 0, 0])
        .expect("declared length is in bounds");
    assert_eq!(readings.len(), 3);
    assert_eq!(readings.as_slice(), &[1, 2, 3]);
    assert!(BoundedSequence::<i32, 8>::new(9, [0; 8]).is_none());
}

#[test]
fn generated_identities_are_full_and_tags_bind_the_retained_descriptor() {
    assert_ne!(telemetry_stream::INTERFACE_IDENTITY, [0; 32]);
    assert_ne!(parameter_call::INTERFACE_IDENTITY, [0; 32]);
    assert_ne!(navigation_operation::INTERFACE_IDENTITY, [0; 32]);
    assert_ne!(
        telemetry_stream::INTERFACE_IDENTITY,
        parameter_call::INTERFACE_IDENTITY
    );
    assert_ne!(telemetry_stream::TYPE_TAG, 0);

    let descriptor = WireSampleDescriptor {
        magic: SAMPLE_DESCRIPTOR_MAGIC,
        version: FORMAT_VERSION,
        flags: FLAG_LAST,
        capability_kind: CAPABILITY_KIND_LOAN,
        loan_id: 7,
        offset: 0,
        length: 4096,
        type_identity: telemetry_stream::TYPE_TAG,
        sequence: 1,
        reserved: [0; 8],
    };
    assert!(valid_sample_descriptor(
        &descriptor,
        7,
        telemetry_stream::TYPE_TAG,
        4096
    ));
    assert!(!valid_sample_descriptor(
        &descriptor,
        7,
        parameter_call::TYPE_TAG,
        4096
    ));
}

#[test]
fn generated_maximum_encoded_sizes_cover_declared_messages() {
    assert_eq!(telemetry_stream::MAX_ENCODED_BYTES, 64);
    assert_eq!(parameter_call::MAX_ENCODED_BYTES, 40);
    assert_eq!(navigation_operation::MAX_ENCODED_BYTES, 16);
}
