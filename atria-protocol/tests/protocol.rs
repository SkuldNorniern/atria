use std::panic::{AssertUnwindSafe, catch_unwind};

use atria_protocol::capability::{Capability, CapabilitySet};
use atria_protocol::error::{ErrorCategory, ErrorCode};
use atria_protocol::message::{
    AttachWithFence, ConnectRequest, DisplayError, EncodePayload, RegistryGlobal, encode_message,
};
use atria_protocol::opcode::{Interface, MessageKind, Opcode, Operation, decode_operation};
use atria_protocol::version::VersionRange;
use atria_protocol::wire::{
    Decoder, Encoder, FdIndex, Frame, HEADER_SIZE, MAX_MESSAGE_SIZE, MAX_PAYLOAD_SIZE, encode_frame,
};
use atria_protocol::{DecodeError, EncodeError, ObjectId};

#[test]
fn frame_round_trips_across_many_values_and_payload_sizes() {
    let mut packet = [0_u8; 512];
    let mut payload = [0_u8; 256];
    for (index, byte) in payload.iter_mut().enumerate() {
        *byte = index as u8;
    }

    for payload_len in (0..=payload.len()).step_by(4) {
        for seed in [0_u32, 1, 255, 256, 0x1234_5678, u32::MAX] {
            let object_id = ObjectId::from_raw(seed);
            let opcode = Opcode::from_raw(seed as u16);
            let sequence = seed.rotate_left(13);
            let size = encode_frame(
                object_id,
                opcode,
                sequence,
                &payload[..payload_len],
                &mut packet,
            )
            .expect("valid frame must encode");
            let decoded = Frame::decode(&packet[..size]).expect("encoded frame must decode");
            assert_eq!(decoded.header.object_id, object_id);
            assert_eq!(decoded.header.opcode, opcode);
            assert_eq!(decoded.header.sequence_num, sequence);
            assert_eq!(decoded.payload, &payload[..payload_len]);
        }
    }
}

#[test]
fn string_codec_round_trips_lengths_and_unicode() {
    for value in ["", "a", "ab", "abc", "abcd", "atria_seat", "héllø 世界"] {
        let mut bytes = [0xaa_u8; 128];
        let used = {
            let mut encoder = Encoder::new(&mut bytes);
            encoder.write_string(value).expect("string must fit");
            encoder.position()
        };
        assert_eq!(used % 4, 0);
        let mut decoder = Decoder::new(&bytes[..used]);
        assert_eq!(decoder.read_string().expect("string must decode"), value);
        decoder.finish().expect("all bytes consumed");
    }
}

#[test]
fn specified_messages_round_trip() {
    let attach = AttachWithFence {
        buffer_id: ObjectId::from_raw(700),
        fence_fd: FdIndex::new(255),
        x_offset: -17,
        y_offset: i32::MAX,
    };
    let mut payload = [0_u8; 64];
    let used = encode_payload(&attach, &mut payload);
    assert_eq!(AttachWithFence::decode(&payload[..used]), Ok(attach));

    let global = RegistryGlobal {
        name: 42,
        interface: "atria_seat",
        version: 1,
    };
    let used = encode_payload(&global, &mut payload);
    assert_eq!(RegistryGlobal::decode(&payload[..used]), Ok(global));

    let error = DisplayError {
        object_id: ObjectId::from_raw(11),
        code: ErrorCode::INVALID_OBJECT,
        message: "object destroyed",
    };
    let used = encode_payload(&error, &mut payload);
    assert_eq!(DisplayError::decode(&payload[..used]), Ok(error));
}

#[test]
fn typed_message_encoder_includes_checked_header() {
    let request = ConnectRequest {
        min_version: 1,
        max_version: 3,
    };
    let mut packet = [0_u8; 32];
    let size = encode_message(
        ObjectId::DISPLAY,
        Opcode::from_raw(9),
        77,
        &request,
        &mut packet,
    )
    .expect("message must encode");
    let frame = Frame::decode(&packet[..size]).expect("message must decode");
    assert_eq!(ConnectRequest::decode(frame.payload), Ok(request));
}

#[test]
fn truncated_headers_and_messages_are_rejected() {
    for length in 0..HEADER_SIZE {
        assert!(matches!(
            Frame::decode(&[0_u8; HEADER_SIZE][..length]),
            Err(DecodeError::Truncated { .. })
        ));
    }

    let mut packet = [0_u8; 16];
    packet[6..8].copy_from_slice(&20_u16.to_le_bytes());
    assert_eq!(
        Frame::decode(&packet),
        Err(DecodeError::Truncated {
            needed: 20,
            available: 16,
        })
    );
}

#[test]
fn oversized_declared_length_is_rejected() {
    let mut packet = [0_u8; HEADER_SIZE];
    packet[6..8].copy_from_slice(&u16::MAX.to_le_bytes());
    assert_eq!(
        Frame::decode(&packet),
        Err(DecodeError::MessageTooLarge {
            size: usize::from(u16::MAX),
            maximum: MAX_MESSAGE_SIZE,
        })
    );
}

#[test]
fn misaligned_declared_and_encoded_sizes_are_rejected() {
    let mut packet = [0_u8; 14];
    packet[6..8].copy_from_slice(&14_u16.to_le_bytes());
    assert_eq!(
        Frame::decode(&packet),
        Err(DecodeError::MisalignedSize { size: 14 })
    );
    assert_eq!(
        encode_frame(
            ObjectId::DISPLAY,
            Opcode::from_raw(0),
            0,
            &[0_u8; 1],
            &mut packet,
        ),
        Err(EncodeError::MisalignedSize { size: 13 })
    );
}

#[test]
fn maximum_payload_boundary_is_checked() {
    let payload = vec![0_u8; MAX_PAYLOAD_SIZE];
    let mut packet = vec![0_u8; MAX_MESSAGE_SIZE];
    assert_eq!(
        encode_frame(
            ObjectId::DISPLAY,
            Opcode::from_raw(0),
            1,
            &payload,
            &mut packet,
        ),
        Ok(MAX_MESSAGE_SIZE)
    );
    assert!(Frame::decode(&packet).is_ok());

    let oversized_payload = vec![0_u8; MAX_PAYLOAD_SIZE + 4];
    assert_eq!(
        encode_frame(
            ObjectId::DISPLAY,
            Opcode::from_raw(0),
            1,
            &oversized_payload,
            &mut packet,
        ),
        Err(EncodeError::MessageTooLarge {
            size: MAX_MESSAGE_SIZE + 4,
            maximum: MAX_MESSAGE_SIZE,
        })
    );
}

#[test]
fn malformed_strings_fds_and_trailing_payload_are_rejected() {
    let mut invalid_utf8 = [0_u8; 8];
    invalid_utf8[..4].copy_from_slice(&1_u32.to_le_bytes());
    invalid_utf8[4] = 0xff;
    assert_eq!(
        Decoder::new(&invalid_utf8).read_string(),
        Err(DecodeError::InvalidUtf8)
    );

    let mut plain_integer = [0_u8; 4];
    plain_integer.copy_from_slice(&7_u32.to_le_bytes());
    assert_eq!(
        Decoder::new(&plain_integer).read_fd(),
        Err(DecodeError::InvalidFdPlaceholder { value: 7 })
    );

    let request_with_extra_field = [1_u8, 0, 0, 0, 2, 0, 0, 0, 3, 0, 0, 0];
    assert_eq!(
        ConnectRequest::decode(&request_with_extra_field),
        Err(DecodeError::TrailingBytes {
            declared: 8,
            actual: 12,
        })
    );
}

#[test]
fn unknown_and_forbidden_opcodes_are_typed_errors() {
    let unknown = Opcode::from_raw(0x00ff);
    assert_eq!(
        decode_operation(Interface::Display, MessageKind::Method, unknown),
        Err(DecodeError::UnknownOpcode {
            interface: Interface::Display,
            kind: MessageKind::Method,
            opcode: unknown,
        })
    );
    assert_eq!(
        Opcode::from_raw(0xf123).validate_from_untrusted_client(),
        Err(DecodeError::ForbiddenClientOpcode {
            opcode: Opcode::from_raw(0xf123),
        })
    );
    assert_eq!(
        decode_operation(
            Interface::Surface,
            MessageKind::Method,
            Opcode::from_raw(0x0011)
        ),
        Ok(Operation::SurfaceAttachWithFence)
    );
}

#[test]
fn malformed_byte_sequences_never_panic() {
    let mut state = 0x6a09_e667_f3bc_c909_u64;
    let mut bytes = [0_u8; 96];
    for length in 0..=bytes.len() {
        for _ in 0..16 {
            for byte in &mut bytes[..length] {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                *byte = state as u8;
            }
            let result = catch_unwind(AssertUnwindSafe(|| {
                let _ = Frame::decode(&bytes[..length]);
                let mut decoder = Decoder::new(&bytes[..length]);
                let _ = decoder.read_string();
            }));
            assert!(result.is_ok(), "decoder panicked for length {length}");
        }
    }
}

#[test]
fn error_ranges_versions_objects_and_capabilities_follow_policy() {
    assert_eq!(
        ErrorCode::from_raw(1).category(),
        Some(ErrorCategory::Protocol)
    );
    assert_eq!(
        ErrorCode::from_raw(0x100).category(),
        Some(ErrorCategory::Object)
    );
    assert_eq!(
        ErrorCode::from_raw(0x200).category(),
        Some(ErrorCategory::Resource)
    );
    assert_eq!(
        ErrorCode::from_raw(0x300).category(),
        Some(ErrorCategory::Compositor)
    );
    assert_eq!(ErrorCode::from_raw(0).category(), None);

    let client = VersionRange::new(1, 3).expect("ordered range");
    let server = VersionRange::new(2, 4).expect("ordered range");
    assert_eq!(client.overlap(server), VersionRange::new(2, 3));
    assert_eq!(
        client.overlap(VersionRange::new(4, 5).expect("ordered range")),
        None
    );

    assert!(ObjectId::from_raw(1).is_reserved());
    assert!(!ObjectId::from_raw(0).is_reserved());
    assert!(ObjectId::from_raw(256).is_client_allocatable());

    let freebsd = CapabilitySet::default_grants().with(Capability::SoftwareShm);
    assert!(freebsd.contains(Capability::SoftwareShm));
    assert!(!freebsd.contains(Capability::ExplicitGpuFence));
    assert!(!freebsd.contains(Capability::RevocableSeat));
    assert!(!freebsd.satisfies(CapabilitySet::empty().with(Capability::GpuPrime)));
}

fn encode_payload(payload: &impl EncodePayload, output: &mut [u8]) -> usize {
    let declared = payload.encoded_len().expect("length must compute");
    let mut encoder = Encoder::new(output);
    payload.encode(&mut encoder).expect("payload must fit");
    assert_eq!(encoder.position(), declared);
    declared
}
