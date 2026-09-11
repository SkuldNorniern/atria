//! Turning what a viewer says about keys into what Atria says about keys.
//!
//! RFB carries keysyms, which name what a key means under some layout. Atria carries physical
//! positions. Translating is guesswork in general; this covers the part that is not.

use atria_protocol::key::usage;
use atria_vnc::protocol::usage_of_keysym;

#[test]
fn a_letter_is_the_same_position_whichever_case_it_arrives_in() {
    assert_eq!(usage_of_keysym(0x61), Some(usage::A), "lower case a");
    assert_eq!(usage_of_keysym(0x41), Some(usage::A), "upper case A");
    assert_eq!(
        usage_of_keysym(0x71),
        usage_of_keysym(0x51),
        "shift changes what a key means, not where it is"
    );
    assert_eq!(usage_of_keysym(0x71), Some(usage::Q));
    assert_eq!(usage_of_keysym(0x7a), Some(usage::Z));
}

#[test]
fn the_digits_follow_the_row_and_not_the_number() {
    // HID puts zero after nine, because that is where it sits on the row.
    assert_eq!(usage_of_keysym(0x31), Some(usage::ONE));
    assert_eq!(usage_of_keysym(0x39), Some(usage::ONE + 8));
    assert_eq!(usage_of_keysym(0x30), Some(usage::ONE + 9));
}

#[test]
fn the_named_keys_and_modifiers_are_known() {
    assert_eq!(usage_of_keysym(0xff0d), Some(usage::ENTER));
    assert_eq!(usage_of_keysym(0xff1b), Some(usage::ESCAPE));
    assert_eq!(usage_of_keysym(0x20), Some(usage::SPACE));
    assert_eq!(usage_of_keysym(0xff52), Some(usage::UP));
    assert_eq!(usage_of_keysym(0xffe1), Some(usage::LEFT_SHIFT));
    assert_eq!(usage_of_keysym(0xffec), Some(usage::RIGHT_META));
    // Super, Meta and Hyper are one physical key under three names, and which one a viewer sends
    // is its keymap's business rather than the key's.
    for left in [0xffeb_u32, 0xffe7, 0xffed] {
        assert_eq!(
            usage_of_keysym(left),
            Some(usage::LEFT_META),
            "keysym {left:#06x} is the same key"
        );
    }
    for right in [0xffec_u32, 0xffe8, 0xffee] {
        assert_eq!(usage_of_keysym(right), Some(usage::RIGHT_META));
    }
    assert_eq!(usage_of_keysym(0xffbe), Some(usage::F1));
}

#[test]
fn a_key_this_server_cannot_place_is_dropped_rather_than_guessed() {
    // A keysym whose position depends on the layout. Sending some position for it would put a
    // key under the client's hand that the person did not press.
    assert_eq!(usage_of_keysym(0x00b5), None, "micro sign");
    assert_eq!(usage_of_keysym(0x3131), None, "hangul letter");
    assert_eq!(usage_of_keysym(0), None);
}
