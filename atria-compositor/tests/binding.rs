//! What draft v0.1 can actually carry from a client to the state machine.
//!
//! These tests are as much a record of the draft's coverage as a check on the code. The
//! `PayloadUnspecified` cases are the ones to watch: each names an operation the draft numbered
//! and never described, and each should turn into a real decode the day the draft says what the
//! payload is.

use atria_compositor::{
    BindError, ClientRequest, CompositorState, ConnectionLimits, ObjectKind, Point, ServerLimits,
    interface_of, request_from_frame,
};
use atria_protocol::capability::{Capability, CapabilitySet};
use atria_protocol::message::{AttachWithFence, EncodePayload, GetRegistry, encode_message};
use atria_protocol::opcode::{Interface, Operation};
use atria_protocol::wire::{Frame, HandleIndex, HandleKind, Header};
use atria_protocol::{ObjectId, Opcode};

fn id(raw: u32) -> ObjectId {
    ObjectId::from_raw(raw)
}

/// Frames one payload into `buffer` and returns the encoded packet.
fn framed<'a, P: EncodePayload>(
    buffer: &'a mut [u8],
    object: ObjectId,
    operation: Operation,
    payload: &P,
) -> &'a [u8] {
    let opcode = operation
        .opcode()
        .unwrap_or_else(|| panic!("{operation:?} must have an assigned opcode"));
    let used = encode_message(object, opcode, 0, payload, buffer)
        .unwrap_or_else(|error| panic!("payload must encode: {error:?}"));
    &buffer[..used]
}

#[test]
fn every_object_kind_answers_exactly_one_interface() {
    // The binding chooses a decoder by interface, so a kind that mapped to the wrong one would
    // decode a payload as some other interface's message of the same number.
    assert_eq!(interface_of(ObjectKind::Display), Interface::Display);
    assert_eq!(interface_of(ObjectKind::Registry), Interface::Registry);
    assert_eq!(interface_of(ObjectKind::Seat), Interface::Seat);
    assert_eq!(interface_of(ObjectKind::Session), Interface::Session);
    assert_eq!(interface_of(ObjectKind::Surface), Interface::Surface);
    assert_eq!(interface_of(ObjectKind::Buffer), Interface::Buffer);
    assert_eq!(interface_of(ObjectKind::Fence), Interface::Fence);
    assert_eq!(
        interface_of(ObjectKind::InputStream),
        Interface::InputStream
    );
}

#[test]
fn a_get_registry_frame_becomes_a_create_registry_request() {
    let mut buffer = [0_u8; 64];
    let packet = framed(
        &mut buffer,
        ObjectId::DISPLAY,
        Operation::DisplayGetRegistry,
        &GetRegistry { new_id: id(2) },
    );
    let frame = Frame::decode(packet).expect("frame must decode");

    // The registry is a protocol singleton, so its identifier comes from the reserved range
    // rather than the client-allocated one above 256.
    assert_eq!(
        request_from_frame(ObjectKind::Display, &frame),
        Ok(ClientRequest::CreateRegistry { new_id: id(2) })
    );
}

#[test]
fn an_attach_with_fence_frame_becomes_an_attach_request() {
    let mut buffer = [0_u8; 64];
    let packet = framed(
        &mut buffer,
        id(400),
        Operation::SurfaceAttachWithFence,
        &AttachWithFence {
            buffer_id: id(401),
            fence: HandleIndex::new(0, HandleKind::Fence),
            x_offset: 12,
            y_offset: -34,
        },
    );
    let frame = Frame::decode(packet).expect("frame must decode");

    // The fence slot decodes, but binding it to a fence object needs a transport holding the
    // handle array. Until one exists the request carries no fence rather than a made-up one.
    assert_eq!(
        request_from_frame(ObjectKind::Surface, &frame),
        Ok(ClientRequest::Attach {
            surface: id(400),
            buffer: id(401),
            offset: Point { x: 12, y: -34 },
            acquire_fence: None,
        })
    );
}

#[test]
fn the_same_opcode_on_another_interface_is_not_that_interface_s_message() {
    // `display.get_registry` and `buffer.release_with_fence` are both opcode 0x0001. Only the
    // object's kind separates them, which is why the binding refuses to work from a number alone.
    let mut buffer = [0_u8; 64];
    let packet = framed(
        &mut buffer,
        ObjectId::DISPLAY,
        Operation::DisplayGetRegistry,
        &GetRegistry { new_id: id(300) },
    );
    let frame = Frame::decode(packet).expect("frame must decode");

    assert_eq!(
        request_from_frame(ObjectKind::Buffer, &frame),
        Err(BindError::UnknownOpcode {
            opcode: Opcode::from_raw(0x0001)
        })
    );
}

#[test]
fn operations_the_draft_numbered_without_a_payload_are_reported_as_unspecified() {
    // Each of these is reachable by a client today: §13 gives it a number, so a client can send
    // it, and nothing in the draft says what the bytes after the header mean. A compositor that
    // guessed would be defining the protocol by accident.
    let unspecified = [
        Operation::SurfaceAttach,
        Operation::SurfaceDamage,
        Operation::SurfaceCommit,
        Operation::SurfaceSetOpaqueRegion,
        Operation::SurfaceSetRefreshRange,
        Operation::SurfaceDestroy,
    ];

    for operation in unspecified {
        let opcode = operation
            .opcode()
            .unwrap_or_else(|| panic!("{operation:?} is numbered by the draft"));
        let header = Header::for_payload(id(400), opcode, 0, 0).expect("empty payload frames");
        let mut packet = [0_u8; 12];
        header.encode(&mut packet).expect("header encodes");
        let frame = Frame::decode(&packet).expect("frame must decode");

        assert_eq!(
            request_from_frame(ObjectKind::Surface, &frame),
            Err(BindError::PayloadUnspecified { operation }),
            "{operation:?} should be reported as unspecified rather than decoded"
        );
    }
}

#[test]
fn surface_frame_is_refused_because_the_draft_never_gave_it_a_direction() {
    // §13 numbers `surface.frame` 0x0014 and §4 does not list it at all, so nothing says whether
    // a client may send it. It is refused as an unknown method rather than accepted on the
    // strength of what the same name means in other protocols.
    let opcode = Operation::SurfaceFrame
        .opcode()
        .expect("the draft numbers surface.frame");
    let header = Header::for_payload(id(400), opcode, 0, 0).expect("empty payload frames");
    let mut packet = [0_u8; 12];
    header.encode(&mut packet).expect("header encodes");
    let frame = Frame::decode(&packet).expect("frame must decode");

    assert_eq!(
        request_from_frame(ObjectKind::Surface, &frame),
        Err(BindError::UnknownOpcode { opcode })
    );
}

#[test]
fn a_bound_request_is_accepted_by_the_state_machine() {
    // The point of the binding: bytes in, state changed, with nothing hand-built in between.
    let mut state = CompositorState::new(
        CapabilitySet::default_grants().with(Capability::SoftwareShm),
        ServerLimits::default(),
        ConnectionLimits::default(),
    );
    let connection = state
        .connect(
            CapabilitySet::default_grants().with(Capability::SoftwareShm),
            CapabilitySet::empty(),
        )
        .expect("baseline capabilities overlap");

    let mut buffer = [0_u8; 64];
    let packet = framed(
        &mut buffer,
        ObjectId::DISPLAY,
        Operation::DisplayGetRegistry,
        &GetRegistry { new_id: id(2) },
    );
    let frame = Frame::decode(packet).expect("frame must decode");

    let kind = state
        .object_kind(connection, frame.header.object_id)
        .expect("the display object is always live");
    let request = request_from_frame(kind, &frame).expect("a specified request binds");

    state
        .dispatch(connection, request)
        .expect("registry created");
    assert_eq!(
        state.object_kind(connection, id(2)),
        Some(ObjectKind::Registry)
    );
}
