//! What may be lost when a viewer sends faster than the compositor reads, and what may not.
//!
//! A pointer position is state: a newer one carries everything an older one did, so keeping only
//! the newest loses nothing. A key or a button is a transition. Dropping one leaves the
//! compositor believing a key is down that is up, and nothing later corrects it.

use atria_vnc::{Input, InputQueue};

fn sink() -> InputQueue {
    InputQueue::new()
}

/// RFB's `PointerEvent`: a button mask, then x and y.
fn pointer(mask: u8, x: u16, y: u16) -> Vec<u8> {
    let mut body = vec![mask];
    body.extend_from_slice(&x.to_be_bytes());
    body.extend_from_slice(&y.to_be_bytes());
    body
}

#[test]
fn a_run_of_motion_keeps_only_where_the_pointer_ended() {
    let mut queue = sink();
    for step in 0..500_u16 {
        queue.pointer(&pointer(0, step, step));
    }

    let taken = queue.take();
    assert_eq!(
        taken.len(),
        1,
        "five hundred positions say one thing: where the pointer is"
    );
    assert!(
        matches!(taken[0], Input::Pointer(at) if at.x == 499 && at.y == 499),
        "and it is the newest, not the oldest"
    );
    assert!(!queue.overflowed(), "nothing was lost");
}

#[test]
fn a_button_mask_becomes_the_transitions_it_implies() {
    let mut queue = sink();
    queue.pointer(&pointer(0, 10, 10));
    queue.pointer(&pointer(1, 10, 10));
    queue.pointer(&pointer(1, 20, 20));
    queue.pointer(&pointer(0, 20, 20));

    let taken = queue.take();
    let buttons: Vec<_> = taken
        .iter()
        .filter_map(|input| match input {
            Input::Button(button) => Some(button.pressed),
            _ => None,
        })
        .collect();
    assert_eq!(
        buttons,
        vec![true, false],
        "a mask that did not change is not a press, and one that did is exactly one"
    );
}

#[test]
fn a_press_is_never_dropped_to_make_room() {
    let mut queue = sink();
    // Far more transitions than the queue holds.
    for _ in 0..500 {
        queue.key(0x04, true);
        queue.key(0x04, false);
    }

    assert!(
        queue.overflowed(),
        "losing a transition is reported, because the compositor's idea of what is held is now wrong"
    );
    let taken = queue.take();
    let downs = taken
        .iter()
        .filter(|input| matches!(input, Input::Key(key) if key.pressed))
        .count();
    let ups = taken
        .iter()
        .filter(|input| matches!(input, Input::Key(key) if !key.pressed))
        .count();
    assert_eq!(
        downs, ups,
        "and what did survive is whole pairs, not half a transition"
    );
}

#[test]
fn overflow_is_reported_once_and_then_cleared() {
    let mut queue = sink();
    for _ in 0..500 {
        queue.key(0x04, true);
    }
    assert!(queue.overflowed());
    assert!(
        !queue.overflowed(),
        "asking twice does not mean it happened twice"
    );
}

#[test]
fn letting_go_forgets_the_buttons_a_viewer_left_held() {
    let mut queue = sink();
    queue.pointer(&pointer(1, 10, 10));
    let _ = queue.take();

    // The viewer went away holding the button, and comes back holding nothing.
    queue.forget_held();
    assert!(queue.is_stale(), "the compositor is told to let go");
    assert!(!queue.is_stale(), "and told once, not for ever");

    // The same mask again is a fresh press, not a repeat of one believed still down.
    queue.pointer(&pointer(1, 10, 10));
    let pressed = queue
        .take()
        .into_iter()
        .filter(|input| matches!(input, Input::Button(button) if button.pressed))
        .count();
    assert_eq!(
        pressed, 1,
        "a viewer that comes back pressing is pressing, not continuing"
    );
}
