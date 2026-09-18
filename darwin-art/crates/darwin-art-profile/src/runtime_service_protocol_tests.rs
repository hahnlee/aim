use super::*;
use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::{OsStrExt, OsStringExt};

#[test]
fn exact_payload_limit_and_fixed_message_envelopes() {
    let mut request = sample_request();
    request.environment.clear();
    request.arguments = vec![OsString::new()];
    let overhead = request.encode().unwrap().len();
    request.arguments[0] = OsString::from_vec(vec![b'x'; MAX_PAYLOAD - overhead]);
    let encoded = request.encode().unwrap();
    assert_eq!(encoded.len(), MAX_PAYLOAD);
    assert_eq!(StartRuntimeRequest::decode(&encoded).unwrap(), request);
    request.arguments[0].push("x");
    assert!(request.encode().is_err());
    assert!(StartRuntimeRequest::decode(&vec![0; MAX_PAYLOAD + 1]).is_err());
    let ready = ReadyRequest {
        token: InstanceToken([1; 16]),
        readiness: ReadinessMask::BINDER,
    };
    let response = StartRuntimeResponse {
        pid: 42,
        token: ready.token,
    };
    for (mut payload, is_ready) in [
        (ready.encode().unwrap(), true),
        (response.encode().unwrap(), false),
    ] {
        for length in 0..payload.len() {
            assert!(if is_ready {
                ReadyRequest::decode(&payload[..length]).is_err()
            } else {
                StartRuntimeResponse::decode(&payload[..length]).is_err()
            });
        }
        payload[0] = 2;
        assert!(if is_ready {
            ReadyRequest::decode(&payload).is_err()
        } else {
            StartRuntimeResponse::decode(&payload).is_err()
        });
        payload[0] = 1;
        payload.push(0);
        assert!(if is_ready {
            ReadyRequest::decode(&payload).is_err()
        } else {
            StartRuntimeResponse::decode(&payload).is_err()
        });
    }
}

fn sample_request() -> StartRuntimeRequest {
    StartRuntimeRequest {
        package: "org.example.runtime".into(),
        key: RuntimeKey([0xabu8; 32]),
        required: ReadinessMask::BINDER.union(ReadinessMask::COMPOSITOR),
        arguments: vec![
            OsString::from_vec(vec![b'a', 0x80, b'b']),
            OsString::from_vec(vec![0xff, b'=', b'x']),
        ],
        environment: vec![
            (
                OsString::from_vec(vec![b'A', b'_', 0x81]),
                OsString::from_vec(vec![b'v', 0xfe]),
            ),
            (OsString::from("LANG"), OsString::from("C")),
        ],
    }
}

#[test]
fn runtime_key_accepts_uniform_hex_and_displays_lowercase() {
    let lower = "0123456789abcdef".repeat(4);
    let upper = lower.to_uppercase();
    assert_eq!(RuntimeKey::from_hex(&lower).unwrap().to_string(), lower);
    assert_eq!(RuntimeKey::from_hex(&upper).unwrap().to_string(), lower);
    assert!(RuntimeKey::from_hex(&format!("{}A", &lower[..63])).is_err());
    assert!(RuntimeKey::from_hex(&format!("{}A", &lower[..62])).is_err());
    assert!(RuntimeKey::from_hex(&format!("{}a", &upper[..63])).is_err());
}

#[test]
fn request_roundtrip_preserves_non_utf8_osstrings() {
    let request = sample_request();
    let encoded = request.encode().unwrap();
    assert_eq!(StartRuntimeRequest::decode(&encoded).unwrap(), request);
}

#[test]
fn startup_requires_loss_aware_daemon_without_changing_ready_wire() {
    let mut encoded = sample_request().encode().unwrap();
    assert_eq!(encoded[0], 2);
    // Old daemons check VERSION=1 before interpreting or launching a request.
    assert!(Reader::new(&encoded).unwrap().version().is_err());
    encoded[0] = 1;
    assert!(StartRuntimeRequest::decode(&encoded).is_err());
    let ready = ReadyRequest {
        token: InstanceToken([1; 16]),
        readiness: ReadinessMask::BINDER,
    };
    assert_eq!(ready.encode().unwrap()[0], 1);
}

#[test]
fn fixed_codecs_roundtrip() {
    let ready = ReadyRequest {
        token: InstanceToken([0x5a; 16]),
        readiness: ReadinessMask::COMPOSITOR,
    };
    assert_eq!(
        ReadyRequest::decode(&ready.encode().unwrap()).unwrap(),
        ready
    );
    let response = StartRuntimeResponse {
        pid: i32::MAX as u32,
        token: InstanceToken([0xa5; 16]),
    };
    assert_eq!(
        StartRuntimeResponse::decode(&response.encode().unwrap()).unwrap(),
        response
    );
}

#[test]
fn readiness_loss_codec_is_fixed_versioned_and_bounded() {
    let loss = LossRequest {
        token: InstanceToken([0x3c; 16]),
        lost: ReadinessMask::COMPOSITOR,
    };
    let encoded = loss.encode().unwrap();
    assert_eq!(LossRequest::decode(&encoded).unwrap(), loss);
    for length in 0..encoded.len() {
        assert!(LossRequest::decode(&encoded[..length]).is_err());
    }
    let mut trailing = encoded.clone();
    trailing.push(0);
    assert!(LossRequest::decode(&trailing).is_err());
    let mut unknown_mask = encoded;
    unknown_mask[17..21].copy_from_slice(&4_u32.to_le_bytes());
    assert!(LossRequest::decode(&unknown_mask).is_err());
    assert!(ReadinessMask::new(0).is_err());
}

#[test]
fn malformed_truncated_trailing_and_unknown_mask_are_rejected() {
    let encoded = sample_request().encode().unwrap();
    for length in 0..encoded.len() {
        assert!(
            StartRuntimeRequest::decode(&encoded[..length]).is_err(),
            "length={length}"
        );
    }
    let mut trailing = encoded.clone();
    trailing.push(0);
    assert!(StartRuntimeRequest::decode(&trailing).is_err());

    let mut unknown_mask = encoded;
    unknown_mask[33..37].copy_from_slice(&4_u32.to_le_bytes());
    assert!(StartRuntimeRequest::decode(&unknown_mask).is_err());
    assert!(ReadinessMask::new(0).is_err());
    assert!(ReadinessMask::new(4).is_err());
}

#[test]
fn duplicate_environment_keys_and_invalid_fields_are_rejected() {
    let mut request = sample_request();
    request.environment[1].0 = request.environment[0].0.clone();
    assert!(request.encode().is_err());

    request = sample_request();
    request.environment[0].0 = OsString::new();
    assert!(request.encode().is_err());
    request = sample_request();
    request.environment[0].0 = OsString::from("BAD=KEY");
    assert!(request.encode().is_err());
    request = sample_request();
    request.arguments[0] = OsString::from_vec(vec![b'x', 0]);
    assert!(request.encode().is_err());
}

#[test]
fn bounds_and_pid_contract_are_enforced() {
    let mut request = sample_request();
    request.arguments = vec![OsString::from("x"); MAX_ARGC + 1];
    assert!(request.encode().is_err());
    request.arguments = vec![OsString::from("x")];
    request.environment = vec![(OsString::from("K"), OsString::from("v")); MAX_ENVC + 1];
    assert!(request.encode().is_err());
    assert!(StartRuntimeResponse {
        pid: 0,
        token: InstanceToken([0; 16]),
    }
    .encode()
    .is_err());
    assert!(StartRuntimeResponse {
        pid: i32::MAX as u32 + 1,
        token: InstanceToken([0; 16]),
    }
    .encode()
    .is_err());
}

#[test]
fn readiness_union_and_contains_are_bounded() {
    let both = ReadinessMask::BINDER.union(ReadinessMask::COMPOSITOR);
    assert_eq!(both.bits(), 3);
    assert!(both.contains(ReadinessMask::BINDER));
    assert!(both.contains(ReadinessMask::COMPOSITOR));
    assert!(!ReadinessMask::BINDER.contains(ReadinessMask::COMPOSITOR));
}

#[test]
fn malformed_environment_payload_is_rejected_after_decoding() {
    let mut encoded = sample_request().encode().unwrap();
    // Locate the environment key after version, key, mask, package, argc and
    // the two argv fields, then make its length zero.
    let mut cursor = 1 + 32 + 4;
    let package_len = u32::from_le_bytes(encoded[cursor..cursor + 4].try_into().unwrap()) as usize;
    cursor += 4 + package_len + 2;
    for _ in 0..2 {
        let argument_len =
            u32::from_le_bytes(encoded[cursor..cursor + 4].try_into().unwrap()) as usize;
        cursor += 4 + argument_len;
    }
    cursor += 2;
    encoded[cursor..cursor + 4].copy_from_slice(&0_u32.to_le_bytes());
    assert!(StartRuntimeRequest::decode(&encoded).is_err());

    // The codec preserves arbitrary bytes rather than accidentally applying
    // a lossy UTF-8 conversion to argv/environment values.
    assert_eq!(OsStr::from_bytes(&[0x80, 0xff]).as_bytes(), &[0x80, 0xff]);
}
