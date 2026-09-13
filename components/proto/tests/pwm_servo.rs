use slime_proto::{
    pwm_servo::{
        FORMAT_VERSION, MAX_CHANNEL, MAX_PERIOD_US, MAX_PULSE_US, MIN_PERIOD_US, MIN_PULSE_US,
        PULSE_DISABLE, PWM_SERVO_MAGIC, REPLY_LEN, REQUEST_LEN, STATUS_BAD_CHANNEL,
        STATUS_BAD_PERIOD, STATUS_BAD_PULSE, STATUS_DEVICE_ERROR, STATUS_MALFORMED,
        STATUS_NO_DEVICE, STATUS_OK, WirePwmServoReply, WirePwmServoRequest,
    },
    valid_pwm_servo_reply, valid_pwm_servo_request,
};

/// The one request vector both encoders pin: `(pwm 0 1600)` at the default
/// 50 Hz frame. `components/slisp/host_main.c` encodes the same bytes through
/// the generated C header, so the two sides of the endpoint agree by test.
const PINNED_REQUEST: [u8; REQUEST_LEN] = [
    b'P', b'W', b'M', b'S', // magic
    1, 0, 0, 0, // version
    0, 0, 0, 0, // channel 0
    0x20, 0x4e, 0, 0, // period 20000 us
    0x40, 0x06, 0, 0, // pulse 1600 us
    0, 0, 0, 0, // flags
    0, 0, 0, 0, 0, 0, 0, 0, // reserved
];

#[test]
fn the_pinned_request_vector_is_the_generated_encoding() {
    let request = WirePwmServoRequest {
        magic: PWM_SERVO_MAGIC,
        version: FORMAT_VERSION,
        channel: 0,
        period_us: 20_000,
        pulse_us: 1_600,
        flags: 0,
        reserved: [0; 8],
    };
    assert_eq!(request.encode(), PINNED_REQUEST);
    assert_eq!(WirePwmServoRequest::decode(&PINNED_REQUEST), Some(request));
    assert!(valid_pwm_servo_request(&request));
    assert!(WirePwmServoRequest::decode(&PINNED_REQUEST[..REQUEST_LEN - 1]).is_none());
}

#[test]
fn structural_validation_refuses_identity_flags_and_reserved_but_not_bounds() {
    let base = WirePwmServoRequest {
        magic: PWM_SERVO_MAGIC,
        version: FORMAT_VERSION,
        channel: MAX_CHANNEL,
        period_us: MIN_PERIOD_US,
        pulse_us: PULSE_DISABLE,
        flags: 0,
        reserved: [0; 8],
    };
    assert!(valid_pwm_servo_request(&base));
    assert!(!valid_pwm_servo_request(&WirePwmServoRequest {
        magic: PWM_SERVO_MAGIC + 1,
        ..base
    }));
    assert!(!valid_pwm_servo_request(&WirePwmServoRequest {
        version: FORMAT_VERSION + 1,
        ..base
    }));
    assert!(!valid_pwm_servo_request(&WirePwmServoRequest {
        flags: 1,
        ..base
    }));
    assert!(!valid_pwm_servo_request(&WirePwmServoRequest {
        reserved: [0, 0, 0, 0, 0, 0, 0, 1],
        ..base
    }));
    // Out-of-bound values are the driver's to refuse with a named status.
    assert!(valid_pwm_servo_request(&WirePwmServoRequest {
        channel: MAX_CHANNEL + 1,
        period_us: MAX_PERIOD_US + 1,
        pulse_us: MAX_PULSE_US + 1,
        ..base
    }));
}

#[test]
fn the_bounds_nest_and_every_status_is_distinct() {
    assert!(PULSE_DISABLE < MIN_PULSE_US);
    assert!(MIN_PULSE_US < MAX_PULSE_US);
    assert!(MAX_PULSE_US <= MIN_PERIOD_US);
    assert!(MIN_PERIOD_US < MAX_PERIOD_US);
    let statuses = [
        STATUS_OK,
        STATUS_BAD_CHANNEL,
        STATUS_BAD_PERIOD,
        STATUS_BAD_PULSE,
        STATUS_NO_DEVICE,
        STATUS_DEVICE_ERROR,
        STATUS_MALFORMED,
    ];
    assert_eq!(STATUS_OK, 0);
    for (index, status) in statuses.iter().enumerate() {
        assert!(index == 0 || *status < 0);
        assert!(statuses[index + 1..].iter().all(|other| other != status));
    }
}

#[test]
fn a_reply_round_trips_and_carries_only_a_named_status() {
    let reply = WirePwmServoReply {
        magic: PWM_SERVO_MAGIC,
        version: FORMAT_VERSION,
        status: STATUS_NO_DEVICE,
        detail: 0x0020_4000,
    };
    let encoded = reply.encode();
    assert_eq!(WirePwmServoReply::decode(&encoded), Some(reply));
    assert!(WirePwmServoReply::decode(&encoded[..REPLY_LEN - 1]).is_none());
    assert!(valid_pwm_servo_reply(&reply));
    assert!(!valid_pwm_servo_reply(&WirePwmServoReply {
        status: -7,
        ..reply
    }));
    assert!(!valid_pwm_servo_reply(&WirePwmServoReply {
        status: 1,
        ..reply
    }));
    assert!(!valid_pwm_servo_reply(&WirePwmServoReply {
        magic: 0,
        ..reply
    }));
}
