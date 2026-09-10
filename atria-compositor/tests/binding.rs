//! What the wire can carry into the state machine.
//!
//! The `Unmodelled` cases are the ones to watch: each names an operation the interface table
//! defines completely and the compositor has nowhere to put yet. Each becomes a real decode when
//! the state machine grows a request to match, and the test that pins the gap becomes the test
//! that proves the message.

use atria_compositor::{
    BindError, ClientRequest, CompositorState, ConnectionLimits, ObjectKind, Point, ServerLimits,
    interface_of, request_from_frame,
};
use atria_protocol::capability::{Capability, CapabilitySet};
use atria_protocol::interface::{INTERFACES, Interface, MessageKind, Operation};
use atria_protocol::message::{AttachWithFence, EncodePayload, GetRegistry, encode_message};
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
    let opcode = Opcode::from_raw(operation.opcode());
    let used = encode_message(object, opcode, 0, payload, buffer)
        .unwrap_or_else(|error| panic!("payload must encode: {error:?}"));
    &buffer[..used]
}

/// An empty-payload frame, for operations whose arguments the state machine cannot yet accept.
fn bare(packet: &mut [u8; 12], object: ObjectId, operation: Operation) {
    Header::for_payload(object, Opcode::from_raw(operation.opcode()), 0, 0)
        .unwrap_or_else(|error| panic!("an empty payload frames: {error:?}"))
        .encode(packet)
        .unwrap_or_else(|error| panic!("header encodes: {error:?}"));
}

#[test]
fn an_object_kind_answers_at_most_one_interface() {
    assert_eq!(interface_of(ObjectKind::Display), Some(Interface::Display));
    assert_eq!(
        interface_of(ObjectKind::Registry),
        Some(Interface::Registry)
    );
    assert_eq!(interface_of(ObjectKind::Surface), Some(Interface::Surface));
    assert_eq!(interface_of(ObjectKind::Buffer), Some(Interface::Buffer));

    // Input and explicit synchronization are outside the draw path, so these kinds exist in the
    // compositor and cannot be addressed from the wire at all. Mapping them to some interface and
    // refusing every opcode would report the wrong thing.
    for kind in [
        ObjectKind::Seat,
        ObjectKind::Session,
        ObjectKind::Fence,
        ObjectKind::InputStream,
    ] {
        assert_eq!(
            interface_of(kind),
            None,
            "{kind:?} has no assigned interface"
        );
    }
}

#[test]
fn a_frame_to_an_unassigned_kind_names_the_kind() {
    let mut packet = [0_u8; 12];
    bare(&mut packet, id(400), Operation::SurfaceDestroy);
    let frame = Frame::decode(&packet).expect("frame must decode");

    assert_eq!(
        request_from_frame(ObjectKind::Seat, &frame),
        Err(BindError::InterfaceUnassigned {
            kind: ObjectKind::Seat
        })
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

    assert_eq!(
        request_from_frame(ObjectKind::Display, &frame),
        Ok(ClientRequest::CreateRegistry { new_id: id(2) })
    );
}

#[test]
fn an_attach_frame_becomes_an_attach_request() {
    let mut buffer = [0_u8; 64];
    let packet = framed(
        &mut buffer,
        id(400),
        Operation::SurfaceAttach,
        &AttachWithFence {
            buffer_id: id(401),
            fence: HandleIndex::new(0, HandleKind::Fence),
            x_offset: 12,
            y_offset: -34,
        },
    );
    let frame = Frame::decode(packet).expect("frame must decode");

    // The fence slot decodes, but binding it to a fence needs a transport holding the handle
    // array. Until one exists the request carries no fence rather than a made-up one.
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
fn an_opcode_is_read_against_the_addressed_object_s_interface() {
    // `display.get_registry` is method 1. The buffer interface defines one method, so the same
    // number addressed to a buffer is not a message at all — only the object's kind separates
    // them, which is why the binding refuses to work from a number alone.
    let mut buffer = [0_u8; 64];
    let packet = framed(
        &mut buffer,
        ObjectId::DISPLAY,
        Operation::DisplayGetRegistry,
        &GetRegistry { new_id: id(2) },
    );
    let frame = Frame::decode(packet).expect("frame must decode");

    assert_eq!(
        request_from_frame(ObjectKind::Buffer, &frame),
        Err(BindError::UnknownOpcode {
            opcode: Opcode::from_raw(Operation::DisplayGetRegistry.opcode())
        })
    );
}

#[test]
fn operations_the_wire_defines_and_the_state_machine_does_not_model_are_reported() {
    // Every method the draw path assigns, minus the two the compositor already accepts. Each is
    // fully specified on the wire and has nowhere to go yet.
    let modelled = [Operation::DisplayGetRegistry, Operation::SurfaceAttach];

    let mut checked = 0;
    for interface in INTERFACES {
        let Some(kind) = [
            ObjectKind::Display,
            ObjectKind::Registry,
            ObjectKind::Surface,
            ObjectKind::Buffer,
        ]
        .into_iter()
        .find(|kind| interface_of(*kind) == Some(*interface)) else {
            continue;
        };

        for operation in interface.methods() {
            if modelled.contains(operation) {
                continue;
            }
            let mut packet = [0_u8; 12];
            bare(&mut packet, id(400), *operation);
            let frame = Frame::decode(&packet).expect("frame must decode");

            assert_eq!(
                request_from_frame(kind, &frame),
                Err(BindError::Unmodelled {
                    operation: *operation
                }),
                "{} should be reported unmodelled rather than decoded",
                operation.spec().name
            );
            checked += 1;
        }
    }
    assert!(checked > 0, "the sweep must actually reach some operations");
}

#[test]
fn an_event_opcode_is_never_bound_as_a_method() {
    // `surface.enter` is event 0 and `surface.destroy` is method 0. A frame from a client is
    // read as a method, so the number resolves to destroy — never to the event that shares it.
    assert_eq!(Operation::SurfaceEnter.opcode(), 0);
    assert_eq!(Operation::SurfaceEnter.kind(), MessageKind::Event);
    assert_eq!(Operation::SurfaceDestroy.opcode(), 0);

    let mut packet = [0_u8; 12];
    bare(&mut packet, id(400), Operation::SurfaceEnter);
    let frame = Frame::decode(&packet).expect("frame must decode");

    assert_eq!(
        request_from_frame(ObjectKind::Surface, &frame),
        Err(BindError::Unmodelled {
            operation: Operation::SurfaceDestroy
        })
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
    let request = request_from_frame(kind, &frame).expect("a modelled request binds");

    state
        .dispatch(connection, request)
        .expect("registry created");
    assert_eq!(
        state.object_kind(connection, id(2)),
        Some(ObjectKind::Registry)
    );
}
