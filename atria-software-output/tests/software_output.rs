use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use atria_compositor::{
    BufferDescriptor, BufferState, BufferTransport, ClientRequest, CompositorState, ConnectionId,
    ConnectionLimits, EventKind, Point, Rect, Size, SurfaceKey,
};
use atria_protocol::ObjectId;
use atria_protocol::capability::Capability;
use atria_software_output::{
    BufferKey, BufferStore, ComposeError, FileSink, HeadlessSink, PixelLayout, SoftwareBuffer,
    SoftwareOutput, ValidationError, capabilities,
};

fn id(raw: u32) -> ObjectId {
    ObjectId::from_raw(raw)
}

fn layout() -> PixelLayout {
    PixelLayout::new(4).expect("test layout is valid")
}

fn descriptor(width: u32, height: u32, stride: u32, byte_len: u64) -> BufferDescriptor {
    BufferDescriptor {
        transport: BufferTransport::SoftwareShm,
        size: Size { width, height },
        stride,
        byte_len,
    }
}

fn packed_descriptor(width: u32, height: u32) -> BufferDescriptor {
    descriptor(
        width,
        height,
        width * 4,
        u64::from(width) * u64::from(height) * 4,
    )
}

fn setup() -> (CompositorState, ConnectionId) {
    let mut state = CompositorState::new(capabilities(), ConnectionLimits::default());
    let connection = state
        .connect(capabilities(), Default::default())
        .expect("software profile negotiates");
    state
        .create_session(connection, id(256), None, true)
        .expect("session is valid");
    (state, connection)
}

fn create_surface(state: &mut CompositorState, connection: ConnectionId, surface: u32) {
    state
        .dispatch(
            connection,
            ClientRequest::CreateSurface {
                session: id(256),
                new_id: id(surface),
            },
        )
        .expect("surface is valid");
    state
        .place_surface(
            SurfaceKey {
                connection,
                object_id: id(surface),
            },
            Point::default(),
        )
        .expect("surface placement is valid");
}

fn import_attach_commit(
    state: &mut CompositorState,
    connection: ConnectionId,
    surface: u32,
    buffer: u32,
    descriptor: BufferDescriptor,
    damage: Option<Rect>,
) {
    state
        .dispatch(
            connection,
            ClientRequest::ImportBuffer {
                new_id: id(buffer),
                descriptor,
            },
        )
        .expect("buffer import is valid");
    state
        .dispatch(
            connection,
            ClientRequest::Attach {
                surface: id(surface),
                buffer: id(buffer),
                offset: Point::default(),
                acquire_fence: None,
            },
        )
        .expect("attachment is valid");
    if let Some(rect) = damage {
        state
            .dispatch(
                connection,
                ClientRequest::Damage {
                    surface: id(surface),
                    rect,
                },
            )
            .expect("nonzero damage is accepted by protocol state");
    }
    state
        .dispatch(
            connection,
            ClientRequest::Commit {
                surface: id(surface),
            },
        )
        .expect("commit is valid");
}

fn store_buffer(
    store: &mut BufferStore,
    connection: ConnectionId,
    raw: u32,
    descriptor: BufferDescriptor,
    bytes: Vec<u8>,
) {
    store.insert(
        BufferKey {
            connection,
            object_id: id(raw),
        },
        SoftwareBuffer::new(descriptor, layout(), bytes).expect("test buffer is valid"),
    );
}

#[test]
fn damage_rectangle_outside_surface_is_rejected() {
    let (mut state, connection) = setup();
    create_surface(&mut state, connection, 257);
    let descriptor = packed_descriptor(2, 2);
    import_attach_commit(
        &mut state,
        connection,
        257,
        258,
        descriptor,
        Some(Rect {
            x: 1,
            y: 0,
            width: 2,
            height: 1,
        }),
    );
    let mut store = BufferStore::new();
    store_buffer(&mut store, connection, 258, descriptor, vec![1; 16]);
    let mut output = SoftwareOutput::new(
        Size {
            width: 2,
            height: 2,
        },
        layout(),
    )
    .unwrap();

    let error = output.compose(&mut state, &store, 1).unwrap_err();

    assert!(matches!(
        error,
        ComposeError::DamageRectangleOutsideSurface { .. }
    ));
    assert!(matches!(
        state.buffer_state(connection, id(258)),
        Some(BufferState::CompositorHeld { .. })
    ));
}

#[test]
fn stride_smaller_than_width_is_rejected() {
    let descriptor = descriptor(4, 1, 3, 3);

    assert_eq!(
        SoftwareBuffer::new(descriptor, layout(), vec![0; 3]),
        Err(ValidationError::StrideSmallerThanWidth {
            stride: 3,
            width: 4
        })
    );
}

#[test]
fn zero_area_surface_is_rejected() {
    let descriptor = descriptor(0, 1, 4, 4);

    assert_eq!(
        SoftwareBuffer::new(descriptor, layout(), vec![0; 4]),
        Err(ValidationError::ZeroAreaSurface)
    );
}

#[test]
fn buffer_too_small_for_declared_geometry_is_rejected() {
    let descriptor = descriptor(2, 2, 8, 15);

    assert_eq!(
        SoftwareBuffer::new(descriptor, layout(), vec![0; 15]),
        Err(ValidationError::BufferTooSmallForDeclaredGeometry {
            available: 15,
            required: 16
        })
    );
}

#[test]
fn stacking_damage_and_release_ownership_are_respected() {
    let (mut state, connection) = setup();
    create_surface(&mut state, connection, 257);
    create_surface(&mut state, connection, 258);
    state
        .place_surface(
            SurfaceKey {
                connection,
                object_id: id(258),
            },
            Point { x: 1, y: 0 },
        )
        .unwrap();
    let descriptor = packed_descriptor(2, 1);
    import_attach_commit(&mut state, connection, 257, 259, descriptor, None);
    import_attach_commit(&mut state, connection, 258, 260, descriptor, None);
    let mut store = BufferStore::new();
    store_buffer(&mut store, connection, 259, descriptor, vec![1; 8]);
    store_buffer(&mut store, connection, 260, descriptor, vec![2; 8]);
    let mut output = SoftwareOutput::new(
        Size {
            width: 3,
            height: 1,
        },
        layout(),
    )
    .unwrap();

    output.compose(&mut state, &store, 10).unwrap();

    assert_eq!(
        output.frame().bytes(),
        &[1, 1, 1, 1, 2, 2, 2, 2, 2, 2, 2, 2]
    );
    assert_eq!(
        state.buffer_state(connection, id(259)),
        Some(BufferState::Available)
    );
    assert_eq!(
        state.buffer_state(connection, id(260)),
        Some(BufferState::Available)
    );
    assert_eq!(
        state
            .take_events()
            .iter()
            .filter(|event| matches!(event.kind, EventKind::BufferRelease))
            .count(),
        2
    );

    // A new backing replaces only the damaged right pixel of the lower surface. Its left
    // pixel remains cached, and the released old buffer is never read again.
    import_attach_commit(
        &mut state,
        connection,
        257,
        261,
        descriptor,
        Some(Rect {
            x: 1,
            y: 0,
            width: 1,
            height: 1,
        }),
    );
    store_buffer(&mut store, connection, 261, descriptor, vec![9; 8]);
    output.compose(&mut state, &store, 11).unwrap();
    assert_eq!(
        output.frame().bytes(),
        &[1, 1, 1, 1, 2, 2, 2, 2, 2, 2, 2, 2]
    );
}

#[test]
fn software_profile_does_not_advertise_gpu_fences_or_seat_tokens() {
    let available = capabilities();

    assert!(available.contains(Capability::SoftwareShm));
    assert!(!available.contains(Capability::GpuPrime));
    assert!(!available.contains(Capability::ExplicitGpuFence));
    assert!(!available.contains(Capability::RevocableSeat));
}

#[test]
fn headless_and_file_sinks_report_and_store_complete_frames() {
    let (mut state, connection) = setup();
    create_surface(&mut state, connection, 257);
    let descriptor = packed_descriptor(1, 1);
    import_attach_commit(&mut state, connection, 257, 258, descriptor, None);
    let mut store = BufferStore::new();
    store_buffer(&mut store, connection, 258, descriptor, vec![3, 4, 5, 6]);
    let mut output = SoftwareOutput::new(
        Size {
            width: 1,
            height: 1,
        },
        layout(),
    )
    .unwrap();
    let mut headless = HeadlessSink::new();

    let report = output
        .present_to(&mut state, &store, 42, &mut headless)
        .unwrap();

    assert_eq!(headless.frames_presented(), 1);
    assert_eq!(headless.last_report(), Some(report));
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    let path = std::env::temp_dir().join(format!("atria-frame-{}-{nonce}.raw", std::process::id()));
    let mut file = FileSink::create(&path).unwrap();
    output
        .present_to(&mut state, &store, 43, &mut file)
        .unwrap();
    assert_eq!(fs::read(&path).unwrap(), vec![3, 4, 5, 6]);
    fs::remove_file(path).unwrap();
}

#[test]
fn gpu_buffers_are_rejected_instead_of_imported() {
    let mut descriptor = packed_descriptor(1, 1);
    descriptor.transport = BufferTransport::GpuPrime;

    assert_eq!(
        SoftwareBuffer::new(descriptor, layout(), vec![0; 4]),
        Err(ValidationError::UnsupportedTransport(
            BufferTransport::GpuPrime
        ))
    );
}
