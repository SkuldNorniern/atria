//! What a viewer is sent, and how much of it.
//!
//! Sending the whole framebuffer for every change is correct and unusable: at 1280x720 it is
//! three and a half megabytes a frame, which is fine over loopback and is the entire latency over
//! anything else. These are the two things that make the difference — sending only what changed,
//! and not spending a thousand bytes on a square that is one colour.

use atria_vnc::PixelFormat;
use atria_vnc::protocol::{Region, changed_regions, write_update};

const WIDTH: u16 = 256;
const HEIGHT: u16 = 128;

fn blank() -> Vec<u8> {
    vec![0_u8; usize::from(WIDTH) * usize::from(HEIGHT) * 4]
}

/// Paint a filled rectangle, the way a client's window lands in a composed frame.
fn paint(frame: &mut [u8], x: usize, y: usize, width: usize, height: usize, colour: [u8; 4]) {
    for row in 0..height {
        for column in 0..width {
            let at = ((y + row) * usize::from(WIDTH) + x + column) * 4;
            frame[at..at + 4].copy_from_slice(&colour);
        }
    }
}

#[test]
fn a_first_frame_is_sent_whole_because_there_is_nothing_to_compare_it_to() {
    let frame = blank();
    let regions = changed_regions(None, &frame, WIDTH, HEIGHT);
    assert_eq!(
        regions,
        vec![Region {
            x: 0,
            y: 0,
            width: WIDTH,
            height: HEIGHT
        }],
        "a viewer that has seen nothing must be sent everything"
    );
}

#[test]
fn an_unchanged_frame_is_not_sent_at_all() {
    let frame = blank();
    assert!(
        changed_regions(Some(&frame), &frame, WIDTH, HEIGHT).is_empty(),
        "a frame that changed nothing is a frame with nothing to say"
    );
}

#[test]
fn only_the_part_that_changed_is_described() {
    let previous = blank();
    let mut current = previous.clone();
    paint(&mut current, 64, 32, 32, 32, [0x2f, 0x9e, 0xd8, 0xff]);

    let regions = changed_regions(Some(&previous), &current, WIDTH, HEIGHT);
    let covered: usize = regions
        .iter()
        .map(|region| usize::from(region.width) * usize::from(region.height))
        .sum();
    assert!(
        covered < usize::from(WIDTH) * usize::from(HEIGHT) / 4,
        "a small change costs a small update, not the screen: covered {covered}"
    );
    for region in &regions {
        assert!(
            region.x <= 64 && region.y <= 32,
            "and every rectangle is around what actually changed"
        );
    }
}

#[test]
fn a_change_over_most_of_the_frame_is_sent_as_one_rectangle() {
    let previous = blank();
    let mut current = previous.clone();
    paint(
        &mut current,
        0,
        0,
        usize::from(WIDTH),
        usize::from(HEIGHT) - 8,
        [0x11, 0x22, 0x33, 0xff],
    );

    let regions = changed_regions(Some(&previous), &current, WIDTH, HEIGHT);
    assert_eq!(
        regions.len(),
        1,
        "past a point the comparison has stopped paying for itself"
    );
    assert_eq!(regions[0].width, WIDTH);
    assert_eq!(regions[0].height, HEIGHT);
}

#[test]
fn a_square_of_one_colour_costs_a_colour_rather_than_a_square() {
    let mut frame = blank();
    paint(
        &mut frame,
        0,
        0,
        usize::from(WIDTH),
        usize::from(HEIGHT),
        [0x2f, 0x9e, 0xd8, 0xff],
    );
    let whole = [Region {
        x: 0,
        y: 0,
        width: WIDTH,
        height: HEIGHT,
    }];

    let mut raw = Vec::new();
    write_update(
        &mut raw,
        &whole,
        &frame,
        WIDTH,
        PixelFormat::declared(),
        false,
    )
    .unwrap_or_else(|error| panic!("raw must encode: {error}"));

    let mut hextile = Vec::new();
    write_update(
        &mut hextile,
        &whole,
        &frame,
        WIDTH,
        PixelFormat::declared(),
        true,
    )
    .unwrap_or_else(|error| panic!("hextile must encode: {error}"));

    assert!(
        hextile.len() * 50 < raw.len(),
        "flat colour is where this pays: {} against {}",
        hextile.len(),
        raw.len()
    );
}

#[test]
fn a_square_that_is_not_one_colour_costs_one_byte_more_than_raw() {
    // The worst case for hextile: every pixel different from its neighbour.
    let mut frame = blank();
    for (index, pixel) in frame.chunks_exact_mut(4).enumerate() {
        pixel[0] = (index % 251) as u8;
        pixel[1] = (index % 253) as u8;
        pixel[2] = (index % 257) as u8;
    }
    let whole = [Region {
        x: 0,
        y: 0,
        width: WIDTH,
        height: HEIGHT,
    }];

    let mut raw = Vec::new();
    write_update(
        &mut raw,
        &whole,
        &frame,
        WIDTH,
        PixelFormat::declared(),
        false,
    )
    .unwrap_or_else(|error| panic!("raw must encode: {error}"));
    let mut hextile = Vec::new();
    write_update(
        &mut hextile,
        &whole,
        &frame,
        WIDTH,
        PixelFormat::declared(),
        true,
    )
    .unwrap_or_else(|error| panic!("hextile must encode: {error}"));

    let squares = (usize::from(WIDTH) / 16) * (usize::from(HEIGHT) / 16);
    assert_eq!(
        hextile.len(),
        raw.len() + squares,
        "one subencoding byte a square, and never more than that"
    );
}
