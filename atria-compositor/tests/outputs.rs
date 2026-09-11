//! What a desktop needs from its displays, and what most systems get wrong.

use atria_compositor::{
    ClientRequest, CompositorState, ConnectionLimits, EventKind, IdentitySource, MAX_SCALE_TERM,
    OutputIdentity, OutputInfo, OutputSet, Point, Rect, ServerLimits, Size,
};
use atria_protocol::ObjectId;
use atria_protocol::capability::CapabilitySet;
use atria_protocol::interface::Interface;

fn id(raw: u32) -> ObjectId {
    ObjectId::from_raw(raw)
}

fn identity(raw: u128) -> OutputIdentity {
    OutputIdentity(raw)
}

fn info(width: u32, height: u32) -> OutputInfo {
    OutputInfo {
        size: Size { width, height },
        physical_millimetres: Size {
            width: 600,
            height: 340,
        },
        scale_numerator: 1,
        scale_denominator: 1,
        refresh_millihertz: 60_000,
    }
}

/// The failure this whole module exists to prevent: a display that comes back must come back as
/// the same display, so everything keyed to it is still keyed to it.
#[test]
fn a_display_that_returns_is_recognised_rather_than_treated_as_new() {
    let mut outputs = OutputSet::new();
    let panel = identity(0xaa);

    let first = outputs.apply(&[(panel, IdentitySource::Panel, info(2560, 1440))]);
    assert_eq!(first.arrived, vec![panel]);

    let unplugged = outputs.apply(&[]);
    assert_eq!(unplugged.departed, vec![panel]);
    assert!(!outputs.is_present(panel));
    assert!(
        outputs.is_known(panel),
        "an absent display is remembered, not forgotten"
    );
    assert_eq!(
        outputs.info(panel),
        Some(info(2560, 1440)),
        "what it last reported survives its absence"
    );

    let replugged = outputs.apply(&[(panel, IdentitySource::Panel, info(2560, 1440))]);
    assert_eq!(
        replugged.returned,
        vec![panel],
        "it returns, and is not reported as a new arrival"
    );
    assert!(replugged.arrived.is_empty());
    assert_eq!(outputs.len(), 1, "and it is still one display, not two");
}

/// A dock carrying three displays is one change. Reported as three, a shell rearranges three
/// times and the user watches their windows move three times.
#[test]
fn a_dock_arriving_is_one_topology_change() {
    let mut outputs = OutputSet::new();
    let built_in = identity(1);
    outputs.apply(&[(built_in, IdentitySource::Panel, info(2560, 1600))]);

    let docked = outputs.apply(&[
        (built_in, IdentitySource::Panel, info(2560, 1600)),
        (identity(2), IdentitySource::Panel, info(3840, 2160)),
        (identity(3), IdentitySource::Panel, info(3840, 2160)),
        (identity(4), IdentitySource::Position, info(1920, 1080)),
    ]);

    assert_eq!(docked.arrived.len(), 3, "three displays, one change");
    assert!(docked.departed.is_empty());
    assert!(
        docked.reconfigured.is_empty(),
        "the display that did not change is not reported as changed"
    );
}

/// Changing a mode is a reconfiguration of a display that never left. Detaching and reattaching
/// it would discard everything keyed to it, which is why changing a resolution elsewhere
/// rearranges the whole desktop.
#[test]
fn a_mode_change_does_not_detach_the_display() {
    let mut outputs = OutputSet::new();
    let panel = identity(7);
    outputs.apply(&[(panel, IdentitySource::Panel, info(3840, 2160))]);

    let changed = outputs.apply(&[(panel, IdentitySource::Panel, info(2560, 1440))]);

    assert_eq!(changed.reconfigured, vec![panel]);
    assert!(changed.departed.is_empty(), "it never went away");
    assert!(changed.returned.is_empty(), "so it never came back");
    assert_eq!(outputs.info(panel), Some(info(2560, 1440)));
}

/// Two identical panels with nothing unique to report can only be told apart by where they are
/// plugged in. Saying so lets a shell decide; guessing silently is wrong on exactly that
/// hardware.
#[test]
fn an_identity_says_whether_it_came_from_the_panel_or_the_port() {
    let mut outputs = OutputSet::new();
    let serialled = identity(0x1111);
    let anonymous = identity(0x2222);

    outputs.apply(&[
        (serialled, IdentitySource::Panel, info(2560, 1440)),
        (anonymous, IdentitySource::Position, info(2560, 1440)),
    ]);

    assert_eq!(outputs.source(serialled), Some(IdentitySource::Panel));
    assert_eq!(
        outputs.source(anonymous),
        Some(IdentitySource::Position),
        "a shell can tell that swapping cables would swap this one"
    );
}

/// Nothing a user owns may end up somewhere they cannot reach it.
#[test]
fn a_window_left_beyond_every_display_is_brought_back_to_the_nearest_one() {
    let mut outputs = OutputSet::new();
    let left = identity(1);
    let right = identity(2);
    outputs.apply(&[
        (left, IdentitySource::Panel, info(1920, 1080)),
        (right, IdentitySource::Panel, info(1920, 1080)),
    ]);
    outputs.place(left, Point { x: 0, y: 0 });
    outputs.place(right, Point { x: 1920, y: 0 });

    let on_screen = Rect {
        x: 2000,
        y: 100,
        width: 400,
        height: 300,
    };
    assert!(outputs.is_reachable(on_screen));

    // The right-hand display goes away, leaving the window past the edge of everything left.
    outputs.apply(&[(left, IdentitySource::Panel, info(1920, 1080))]);
    assert!(
        !outputs.is_reachable(on_screen),
        "the window is now beyond every attached display"
    );

    let rescued = outputs
        .nearest_reachable(on_screen)
        .expect("a display is attached to move it onto");
    assert_eq!(
        rescued,
        Point { x: 1520, y: 100 },
        "it moves the shortest distance back onto the remaining display, keeping its row"
    );
}

/// Moving to the nearest display rather than the primary one keeps a window near where its user
/// last saw it, instead of collecting everything onto one screen.
#[test]
fn a_window_returns_to_the_nearest_display_not_the_first_one() {
    let mut outputs = OutputSet::new();
    let first = identity(1);
    let far = identity(2);
    outputs.apply(&[
        (first, IdentitySource::Panel, info(1920, 1080)),
        (far, IdentitySource::Panel, info(1920, 1080)),
    ]);
    outputs.place(first, Point { x: 0, y: 0 });
    outputs.place(far, Point { x: 4000, y: 0 });

    // Sitting in the gap between them, closer to the far one.
    let stranded = Rect {
        x: 3600,
        y: 0,
        width: 200,
        height: 200,
    };
    assert!(!outputs.is_reachable(stranded));

    assert_eq!(
        outputs.nearest_reachable(stranded),
        Some(Point { x: 4000, y: 0 }),
        "it goes to the display it was next to"
    );
}

/// A scale must be an exact ratio in lowest terms, so two scales are equal exactly when their
/// fields are, and so multiplying a coordinate by one cannot overflow.
#[test]
fn a_scale_must_be_an_exact_ratio_in_lowest_terms() {
    let canonical = OutputInfo {
        scale_numerator: 3,
        scale_denominator: 2,
        ..info(3840, 2160)
    };
    assert!(canonical.scale_is_canonical());

    for (numerator, denominator) in [(6, 4), (300, 200), (0, 1), (1, 0), (MAX_SCALE_TERM + 1, 1)] {
        let refused = OutputInfo {
            scale_numerator: numerator,
            scale_denominator: denominator,
            ..info(3840, 2160)
        };
        assert!(
            !refused.scale_is_canonical(),
            "{numerator}/{denominator} must be refused"
        );
    }
}

#[test]
fn a_display_is_a_global_a_client_binds_and_is_described_once() {
    let mut state = CompositorState::new(
        CapabilitySet::empty(),
        ServerLimits::default(),
        ConnectionLimits::default(),
    );
    let client = state
        .connect(CapabilitySet::empty(), CapabilitySet::empty())
        .expect("the connection is admitted");

    let delta = state.apply_topology(&[(identity(1), IdentitySource::Panel, info(2560, 1440))], 1);
    assert_eq!(delta.arrived.len(), 1);
    state.place_output(identity(1), Point { x: 0, y: 0 });

    state
        .dispatch(client, ClientRequest::CreateRegistry { new_id: id(2) })
        .expect("a registry");
    let announced: Vec<_> = state
        .take_events()
        .into_iter()
        .filter_map(|event| match event.kind {
            EventKind::Global {
                name, interface, ..
            } => Some((name, interface)),
            _ => None,
        })
        .collect();
    let output = announced
        .iter()
        .find(|(_, interface)| *interface == Interface::Output)
        .map(|(name, _)| *name)
        .expect("the display is announced as a global like anything else");

    state
        .dispatch(
            client,
            ClientRequest::Bind {
                name: output,
                version: 1,
                new_id: id(3),
            },
        )
        .expect("the client binds the display");

    let described: Vec<_> = state
        .take_events()
        .into_iter()
        .filter(|event| event.object_id == id(3))
        .map(|event| event.kind)
        .collect();

    assert_eq!(
        described,
        vec![
            EventKind::OutputIdentity { identity: 1 },
            EventKind::OutputGeometry {
                position: Point { x: 0, y: 0 },
                physical_millimetres: Size {
                    width: 600,
                    height: 340
                },
                identity_source: IdentitySource::Panel,
            },
            EventKind::OutputMode {
                size: Size {
                    width: 2560,
                    height: 1440
                },
                refresh_millihertz: 60_000,
            },
            EventKind::OutputScale {
                numerator: 1,
                denominator: 1,
            },
            EventKind::OutputDone,
        ],
        "a bound display describes itself whole, and says where the description ends"
    );
}

#[test]
fn a_display_departing_withdraws_the_global_it_was_offered_as() {
    let mut state = CompositorState::new(
        CapabilitySet::empty(),
        ServerLimits::default(),
        ConnectionLimits::default(),
    );
    let client = state
        .connect(CapabilitySet::empty(), CapabilitySet::empty())
        .expect("the connection is admitted");
    state
        .dispatch(client, ClientRequest::CreateRegistry { new_id: id(2) })
        .expect("a registry");

    state.apply_topology(&[(identity(1), IdentitySource::Panel, info(1920, 1080))], 1);
    let _ = state.take_events();

    state.apply_topology(&[], 1);
    let withdrawn: Vec<_> = state
        .take_events()
        .into_iter()
        .filter_map(|event| match event.kind {
            EventKind::GlobalRemove { name } => Some(name),
            _ => None,
        })
        .collect();
    assert_eq!(
        withdrawn.len(),
        1,
        "a display that is gone stops being offered, the way any global does"
    );

    // And plugging it back in offers it again, under the identity it always had.
    let delta = state.apply_topology(&[(identity(1), IdentitySource::Panel, info(1920, 1080))], 1);
    assert_eq!(
        delta.returned,
        vec![identity(1)],
        "the same display returning is the same display"
    );
    let offered = state
        .take_events()
        .into_iter()
        .any(|event| matches!(event.kind, EventKind::Global { interface, .. } if interface == Interface::Output));
    assert!(offered, "and it is offered again without the client asking");
}
