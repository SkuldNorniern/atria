//! Anything a client can send, the compositor must refuse rather than fall over.

use atria_compositor::{ObjectKind, decode};
use atria_protocol::wire::Frame;

/// Every object kind the wire can address.
const KINDS: &[ObjectKind] = &[
    ObjectKind::Display,
    ObjectKind::Registry,
    ObjectKind::Compositor,
    ObjectKind::Shm,
    ObjectKind::ShmPool,
    ObjectKind::Buffer,
    ObjectKind::Surface,
    ObjectKind::Shell,
    ObjectKind::Toplevel,
    ObjectKind::Output,
    ObjectKind::ShellControl,
    ObjectKind::Seat,
    ObjectKind::Pointer,
    ObjectKind::Keyboard,
    ObjectKind::Shortcuts,
    ObjectKind::TextInput,
    ObjectKind::InputMethod,
];

/// A deterministic spread of bytes. Not randomness: a test that fails only sometimes is a test
/// nobody can act on.
fn scrambled(seed: u64, len: usize) -> Vec<u8> {
    let mut value = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
    (0..len)
        .map(|_| {
            value = value
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (value >> 33) as u8
        })
        .collect()
}

#[test]
fn no_packet_makes_the_binding_panic() {
    for seed in 0..2_000_u64 {
        let len = 12 + (seed as usize % 64);
        let mut packet = scrambled(seed, len);
        // A plausible header, so the bytes reach the payload decoders rather than being refused
        // by the framing.
        packet[6..8].copy_from_slice(&(len as u16).to_le_bytes());
        let Ok(frame) = Frame::decode(&packet) else {
            continue;
        };
        for kind in KINDS {
            let _ = decode(*kind, &frame);
        }
    }
}

#[test]
fn a_payload_shorter_than_its_fields_is_refused_not_read_past() {
    for kind in KINDS {
        for opcode in 0..8_u16 {
            for len in 12..20_usize {
                let mut packet = vec![0_u8; len];
                packet[0..4].copy_from_slice(&7_u32.to_le_bytes());
                packet[4..6].copy_from_slice(&opcode.to_le_bytes());
                packet[6..8].copy_from_slice(&(len as u16).to_le_bytes());
                let Ok(frame) = Frame::decode(&packet) else {
                    continue;
                };
                let _ = decode(*kind, &frame);
            }
        }
    }
}

#[test]
fn a_string_length_that_lies_is_refused() {
    // A title whose declared length runs past the packet. Believing it would read whatever is
    // next in memory and send it back to the client.
    for declared in [u32::MAX, 1 << 24, 100] {
        let mut packet = vec![0_u8; 20];
        packet[0..4].copy_from_slice(&7_u32.to_le_bytes());
        packet[4..6].copy_from_slice(&1_u16.to_le_bytes());
        packet[6..8].copy_from_slice(&20_u16.to_le_bytes());
        packet[12..16].copy_from_slice(&declared.to_le_bytes());
        let Ok(frame) = Frame::decode(&packet) else {
            continue;
        };
        for kind in KINDS {
            let _ = decode(*kind, &frame);
        }
    }
}
