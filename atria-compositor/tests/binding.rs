//! What the wire can carry into the state machine.
//!
//! The `Unmodelled` cases are the ones to watch: each names an operation the interface table
//! defines completely and the compositor has nowhere to put yet. Each becomes a real decode when
//! the state machine grows a request to match, and the test that pins the gap becomes the test
//! that proves the message.

use atria_compositor::{
    BindError, ClientRequest, CompositorState, ConnectionLimits, DecodedRequest, HandleResolver,
    ObjectKind, ResolveError, ServerLimits, SharedMemory, StateError, decode, interface_of,
    resolve,
};
use atria_protocol::capability::{Capability, CapabilitySet};
use atria_protocol::interface::{INTERFACES, Interface, MessageKind, Operation};
use atria_protocol::message::{CreatePool, EncodePayload, GetRegistry, encode_message};
use atria_protocol::wire::{Frame, HandleIndex, HandleKind, Header};
use atria_protocol::{ObjectId, Opcode};

fn id(raw: u32) -> ObjectId {
    ObjectId::from_raw(raw)
}

/// A transport carrying nothing, for requests that name no handles.
struct NoHandles;

impl HandleResolver for NoHandles {
    fn shared_memory(&self, slot: HandleIndex) -> Result<SharedMemory, ResolveError> {
        Err(ResolveError::SlotEmpty { slot: slot.slot() })
    }
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
        decode(ObjectKind::Seat, &frame),
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
        decode(ObjectKind::Display, &frame),
        Ok(DecodedRequest::CreateRegistry { new_id: id(2) })
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
        decode(ObjectKind::Buffer, &frame),
        Err(BindError::UnknownOpcode {
            opcode: Opcode::from_raw(Operation::DisplayGetRegistry.opcode())
        })
    );
}

#[test]
fn operations_the_wire_defines_and_the_state_machine_does_not_model_are_reported() {
    // Every method the draw path assigns, minus the two the compositor already accepts. Each is
    // fully specified on the wire and has nowhere to go yet.
    let modelled = [Operation::DisplayGetRegistry, Operation::ShmCreatePool];

    let mut checked = 0;
    for interface in INTERFACES {
        let Some(kind) = [
            ObjectKind::Display,
            ObjectKind::Registry,
            ObjectKind::Surface,
            ObjectKind::Buffer,
            ObjectKind::Shm,
            ObjectKind::ShmPool,
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
                decode(kind, &frame),
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
        decode(ObjectKind::Surface, &frame),
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
    let decoded = decode(kind, &frame).expect("a modelled request decodes");
    let request =
        resolve(decoded, &NoHandles).expect("a request naming no handles cannot fail resolution");

    state
        .dispatch(connection, request)
        .expect("registry created");
    assert_eq!(
        state.object_kind(connection, id(2)),
        Some(ObjectKind::Registry)
    );
}

/// A transport holding one shared-memory resource, in one slot.
struct OneRegion {
    slot: u8,
    memory: SharedMemory,
}

impl HandleResolver for OneRegion {
    fn shared_memory(&self, slot: HandleIndex) -> Result<SharedMemory, ResolveError> {
        if slot.slot() == self.slot {
            Ok(self.memory)
        } else {
            Err(ResolveError::SlotEmpty { slot: slot.slot() })
        }
    }
}

fn pool_frame(buffer: &mut [u8], slot: u8, size: u32) -> &[u8] {
    framed(
        buffer,
        id(300),
        Operation::ShmCreatePool,
        &CreatePool {
            new_id: id(301),
            memory: HandleIndex::new(slot, HandleKind::SharedMemory),
            size,
        },
    )
}

/// The three stages fail for three different reasons, and each says which. Collapsing them would
/// report a client whose transport dropped a handle as though it had sent a malformed message.
#[test]
fn each_stage_reports_its_own_kind_of_failure() {
    let mut buffer = [0_u8; 64];

    // Decode: the bytes are not a message this interface defines.
    let mut malformed = [0_u8; 12];
    bare(&mut malformed, id(300), Operation::ShmCreatePool);
    let frame = Frame::decode(&malformed).expect("the header alone is well formed");
    assert!(
        matches!(decode(ObjectKind::Shm, &frame), Err(BindError::Payload(_))),
        "a create_pool with no payload is a decode failure"
    );

    // Resolve: the message is well formed and the transport carried nothing in the slot.
    let packet = pool_frame(&mut buffer, 3, 4096);
    let frame = Frame::decode(packet).expect("frame must decode");
    let decoded = decode(ObjectKind::Shm, &frame).expect("the message is well formed");
    assert_eq!(
        resolve(decoded, &NoHandles),
        Err(ResolveError::SlotEmpty { slot: 3 })
    );

    // Resolve: the slot holds memory, and less of it than the message claims. Believing the
    // message would let every buffer carved from this pool be checked against a size that was
    // never true.
    let carrying = OneRegion {
        slot: 3,
        memory: SharedMemory::new(77, 1024),
    };
    assert_eq!(
        resolve(decoded, &carrying),
        Err(ResolveError::TooSmall {
            slot: 3,
            needed: 4096,
            actual: 1024,
        })
    );

    // Resolve succeeds when the resource is large enough, and the pool records the size the
    // message asked for rather than the whole resource.
    let packet = pool_frame(&mut buffer, 3, 512);
    let frame = Frame::decode(packet).expect("frame must decode");
    let decoded = decode(ObjectKind::Shm, &frame).expect("the message is well formed");
    assert_eq!(
        resolve(decoded, &carrying),
        Ok(ClientRequest::CreatePool {
            new_id: id(301),
            memory: SharedMemory::new(77, 512),
        })
    );
}

/// Dispatch is the third domain: a well-formed, fully resolved request the compositor's state
/// does not allow.
#[test]
fn a_resolved_pool_is_refused_without_the_capability_to_import_one() {
    let mut state = CompositorState::new(
        CapabilitySet::empty(),
        ServerLimits::default(),
        ConnectionLimits::default(),
    );
    let connection = state
        .connect(CapabilitySet::empty(), CapabilitySet::empty())
        .expect("an empty requirement always overlaps");
    state
        .revoke_capability(connection, Capability::BufferImport)
        .expect("the connection exists");

    let refused = state.dispatch(
        connection,
        ClientRequest::CreatePool {
            new_id: id(301),
            memory: SharedMemory::new(77, 512),
        },
    );
    assert!(
        matches!(refused, Err(StateError::UnsupportedCapability { .. })),
        "a pool needs the import capability, and the refusal is a dispatch failure"
    );
}

/// The compositor is handed a validated resource, never a pointer. That is what lets an Artery
/// memory capability and a Unix shared-memory descriptor sit behind one contract.
#[test]
fn a_pool_records_the_resource_without_mapping_it() {
    let mut state = CompositorState::new(
        CapabilitySet::default_grants(),
        ServerLimits::default(),
        ConnectionLimits::default(),
    );
    let connection = state
        .connect(CapabilitySet::default_grants(), CapabilitySet::empty())
        .expect("baseline capabilities overlap");

    state
        .dispatch(
            connection,
            ClientRequest::CreatePool {
                new_id: id(301),
                memory: SharedMemory::new(77, 512),
            },
        )
        .expect("importing a pool succeeds with the default grants");

    assert_eq!(
        state.object_kind(connection, id(301)),
        Some(ObjectKind::ShmPool)
    );
    // The size recorded is the one resolution settled on, and it is all the compositor holds:
    // an identity and a length, with no pointer anywhere.
    let memory = state
        .pool_memory(connection, id(301))
        .expect("the pool records its memory");
    assert_eq!(memory.id(), 77);
    assert_eq!(memory.size(), 512);
}
