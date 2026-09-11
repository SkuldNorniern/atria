//! What an update costs, in bytes and in time.
//!
//! Latency over anything slower than loopback is almost entirely how much is sent, so this
//! measures that directly rather than guessing from a frame rate. Run it after changing how
//! updates are built.
//!
//! Run with: cargo run --release -p atria-vnc --example measure

use std::hint::black_box;
use std::time::Instant;

use atria_vnc::PixelFormat;
use atria_vnc::protocol::{Region, changed_regions, write_update};

const WIDTH: u16 = 1280;
const HEIGHT: u16 = 720;
const ROUNDS: u32 = 50;

fn main() {
    let whole = [Region {
        x: 0,
        y: 0,
        width: WIDTH,
        height: HEIGHT,
    }];
    let size = usize::from(WIDTH) * usize::from(HEIGHT) * 4;
    let previous = vec![0_u8; size];
    let mut current = previous.clone();
    // Two windows' worth of change, which is what dragging one produces: where it was, and where
    // it now is.
    for (left, top) in [(32_usize, 32_usize), (400, 300)] {
        for row in 0..220 {
            let start = ((top + row) * usize::from(WIDTH) + left) * 4;
            current[start..start + 320 * 4].fill(0x7f);
        }
    }

    let started = Instant::now();
    for _ in 0..ROUNDS {
        black_box(changed_regions(
            Some(&previous),
            &current,
            WIDTH,
            HEIGHT,
            &whole,
        ));
    }
    let regions = changed_regions(Some(&previous), &current, WIDTH, HEIGHT, &whole);
    println!(
        "compare:  {:?} per frame, {} rectangles",
        started.elapsed() / ROUNDS,
        regions.len()
    );

    let covered: usize = regions
        .iter()
        .map(|region| usize::from(region.width) * usize::from(region.height))
        .sum();
    println!(
        "covered:  {:.2} MB of {:.2} MB",
        covered as f64 * 4.0 / 1e6,
        size as f64 / 1e6
    );

    for (name, hextile) in [("raw", false), ("hextile", true)] {
        let started = Instant::now();
        let mut bytes = 0;
        for _ in 0..ROUNDS {
            let mut sink = Vec::with_capacity(size);
            write_update(
                &mut sink,
                &regions,
                &current,
                WIDTH,
                PixelFormat::declared(),
                hextile,
            )
            .unwrap_or_else(|error| panic!("an update must encode: {error}"));
            bytes = sink.len();
            black_box(sink);
        }
        println!(
            "{name:8}: {:?} per frame, {:.3} MB on the wire",
            started.elapsed() / ROUNDS,
            bytes as f64 / 1e6
        );
    }
}
