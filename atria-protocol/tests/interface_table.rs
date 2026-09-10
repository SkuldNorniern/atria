//! The table's own invariants. These are what make it authoritative rather than merely tidy.

use atria_protocol::interface::{Field, INTERFACES, Interface, MessageKind, Operation};
use atria_protocol::wire::{HEADER_SIZE, MAX_MESSAGE_SIZE};

/// §12.2: an opcode is a position, not a choice. Anything else lets two operations claim one
/// number, or leaves a gap that implies a meaning the protocol does not have.
#[test]
fn every_opcode_is_its_position_in_its_interface() {
    for interface in INTERFACES {
        for (position, operation) in interface.methods().iter().enumerate() {
            let spec = operation.spec();
            assert_eq!(
                usize::from(spec.opcode),
                position,
                "{}.{} is filed at position {position} and claims opcode {}",
                interface.name(),
                spec.name,
                spec.opcode
            );
            assert_eq!(
                spec.kind,
                MessageKind::Method,
                "{} is not a method",
                spec.name
            );
        }
        for (position, operation) in interface.events().iter().enumerate() {
            let spec = operation.spec();
            assert_eq!(
                usize::from(spec.opcode),
                position,
                "{}.{} is filed at position {position} and claims opcode {}",
                interface.name(),
                spec.name,
                spec.opcode
            );
            assert_eq!(
                spec.kind,
                MessageKind::Event,
                "{} is not an event",
                spec.name
            );
        }
    }
}

/// A row filed under one interface must not describe another, or the lists and the specs
/// disagree about where an operation lives while both look correct in isolation.
#[test]
fn every_operation_is_filed_under_the_interface_it_names() {
    for interface in INTERFACES {
        for operation in interface.methods().iter().chain(interface.events()) {
            assert_eq!(
                operation.spec().interface,
                *interface,
                "{} is listed by {} and claims another interface",
                operation.spec().name,
                interface.name()
            );
        }
    }
}

/// Names are how a trace or a generated binding refers to an operation, so two operations of one
/// interface and kind sharing one would be ambiguous wherever the number is not shown.
#[test]
fn operation_names_are_unique_within_an_interface_and_kind() {
    for interface in INTERFACES {
        for list in [interface.methods(), interface.events()] {
            for (index, operation) in list.iter().enumerate() {
                for other in &list[index + 1..] {
                    assert_ne!(
                        operation.spec().name,
                        other.spec().name,
                        "{} declares {} twice",
                        interface.name(),
                        operation.spec().name
                    );
                }
            }
        }
    }
}

/// Every message carries the twelve-byte header, is a whole number of four-byte words, and fits
/// the permanent ceiling §12.9 fixes. A payload that broke any of these would be undecodable by
/// the framing the header describes.
#[test]
fn every_fixed_message_is_word_aligned_and_within_the_ceiling() {
    for interface in INTERFACES {
        for operation in interface.methods().iter().chain(interface.events()) {
            let spec = operation.spec();
            let Some(size) = spec.message_size() else {
                assert!(
                    spec.payload.contains(&Field::String),
                    "{} has no size but no variable-length field",
                    spec.name
                );
                continue;
            };
            assert!(
                size >= HEADER_SIZE,
                "{} is smaller than a header",
                spec.name
            );
            assert_eq!(
                size % 4,
                0,
                "{} is {size} bytes, not word aligned",
                spec.name
            );
            assert!(
                size <= MAX_MESSAGE_SIZE,
                "{} exceeds the message ceiling",
                spec.name
            );
        }
    }
}

/// The sizes the specification prints, checked against the sizes the table computes. This is the
/// drift the table exists to prevent: a message whose documented size and real layout disagree.
#[test]
fn the_documented_sizes_are_the_computed_sizes() {
    let documented = [
        (Operation::DisplaySync, 0x10),
        (Operation::DisplayGetRegistry, 0x10),
        (Operation::DisplayDeleteId, 0x10),
        (Operation::DisplaySyncDone, 0x10),
        (Operation::RegistryBind, 0x18),
        (Operation::RegistryGlobalRemove, 0x10),
        (Operation::CompositorCreateSurface, 0x10),
        (Operation::ShmCreatePool, 0x18),
        (Operation::DisplayGetRegistry, 0x10),
        (Operation::ShmFormat, 0x10),
        (Operation::ShmPoolCreateBuffer, 0x24),
        (Operation::ShmPoolResize, 0x10),
        (Operation::ShmPoolDestroy, 0x0c),
        (Operation::BufferDestroy, 0x0c),
        (Operation::BufferRelease, 0x0c),
        (Operation::SurfaceDestroy, 0x0c),
        (Operation::SurfaceAttach, 0x18),
        (Operation::SurfaceDamageBuffer, 0x1c),
        (Operation::SurfaceCommit, 0x18),
        (Operation::SurfaceFrame, 0x10),
        (Operation::SurfaceEnter, 0x10),
        (Operation::SurfaceLeave, 0x10),
        (Operation::SurfaceFrameDone, 0x18),
        (Operation::ShellGetToplevel, 0x14),
        (Operation::ToplevelDestroy, 0x0c),
        (Operation::ToplevelSetMinSize, 0x14),
        (Operation::ToplevelSetMaxSize, 0x14),
        (Operation::ToplevelConfigure, 0x1c),
        (Operation::ToplevelClose, 0x0c),
        (Operation::OutputIdentity, 0x1c),
        (Operation::OutputGeometry, 0x18),
        (Operation::OutputMode, 0x1c),
        (Operation::OutputScale, 0x14),
        (Operation::OutputDone, 0x0c),
    ];

    for (operation, size) in documented {
        assert_eq!(
            operation.spec().message_size(),
            Some(size),
            "{} does not have its documented size",
            operation.spec().name
        );
    }
}

/// The three variable-length messages, named so that adding a fourth is a deliberate act rather
/// than something a reader discovers from a `None`.
#[test]
fn only_the_string_carrying_messages_are_variable_length() {
    let variable = [
        Operation::DisplayError,
        Operation::RegistryGlobal,
        Operation::ToplevelSetTitle,
    ];

    for interface in INTERFACES {
        for operation in interface.methods().iter().chain(interface.events()) {
            let expected = variable.contains(operation);
            assert_eq!(
                operation.spec().message_size().is_none(),
                expected,
                "{} disagrees with the variable-length list",
                operation.spec().name
            );
        }
    }
}

/// Every interface the draw path names is in the table exactly once.
#[test]
fn the_table_holds_ten_interfaces() {
    assert_eq!(INTERFACES.len(), 10);
    for (index, interface) in INTERFACES.iter().enumerate() {
        for other in &INTERFACES[index + 1..] {
            assert_ne!(interface.name(), other.name(), "duplicate interface name");
        }
    }
    assert_eq!(Interface::Surface.name(), "atria_surface");
}
