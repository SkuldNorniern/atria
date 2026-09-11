//! Where input goes: which surface the pointer is over, which window holds the keys, and what a
//! shortcut takes before either of them sees it.
//!
//! A child of `state` so that it reaches the routing it changes directly. Everything here is one
//! question — given this input and this scene, who is told — and separating it from the object
//! model is what keeps that question answerable in one place.

use alloc::vec::Vec;

use atria_protocol::ObjectId;
use atria_protocol::key::{Modifiers, PhysicalKey};

use super::{CompositorState, MAX_HELD_KEYS, MAX_SHORTCUTS, ShortcutClaim, StateError};
use crate::model::{
    Chord, ConnectionId, EventKind, InteractionKind, ObjectKind, Point, Rect, SeatId, SurfaceKey,
};

impl CompositorState {
    /// Which routing world the pointer is currently in.
    #[must_use]
    pub const fn pointer_epoch(&self) -> u32 {
        self.seat.epoch
    }

    /// Break routing continuity and start a new epoch. Anything in flight from the old one
    /// carries the old epoch and is recognisable as stale.
    ///
    /// Everything the seat was holding is let go of. A key believed held that is not makes every
    /// chord after it match wrongly, and there is no way to tell which key was lost.
    pub fn reset_input(&mut self) {
        if let Some(target) = self.seat.pointer.target.take() {
            self.send_pointer_leave(target);
        }
        if let Some((connection, control)) = self.seat.pointer.shell_grab.take() {
            self.push_event(
                connection,
                control,
                EventKind::ShellGrabEnd { seat: SeatId(1) },
            );
        }
        self.seat.pointer.buttons_held = 0;
        // Keys held in the old world are not held in the new one.
        self.seat.keyboard.held.clear();
        self.seat.keyboard.consumed.clear();
        self.seat.keyboard.modifiers = Modifiers::default();
        self.seat.epoch = self.seat.epoch.wrapping_add(1);
    }

    /// Move the pointer, routing enter, leave and motion to whatever it is over. While a button
    /// is held the target does not change.
    pub fn move_pointer(&mut self, position: Point, time_ns: u64) {
        self.seat.pointer.position = position;
        if let Some((connection, control)) = self.seat.pointer.shell_grab {
            let seat = SeatId(1);
            self.push_event(
                connection,
                control,
                EventKind::ShellGrabMotion { seat, position },
            );
            return;
        }
        if self.seat.pointer.buttons_held != 0 {
            if let Some(target) = self.seat.pointer.target {
                self.send_pointer_motion(target, position, time_ns);
            }
            return;
        }

        let found = self.surface_under(position);
        if found != self.seat.pointer.target {
            if let Some(old) = self.seat.pointer.target {
                self.send_pointer_leave(old);
            }
            self.seat.pointer.target = found;
            if let Some(new) = found {
                self.send_pointer_enter(new, position);
            }
        }
        if let Some(target) = self.seat.pointer.target {
            self.send_pointer_motion(target, position, time_ns);
        }
    }

    /// Press or release a pointer button. A press takes the implicit grab; the last release
    /// gives it back. A press also tells any attached shell which window was pressed.
    pub fn pointer_button(&mut self, button: u32, pressed: bool, time_ns: u64) {
        if pressed && self.seat.pointer.buttons_held == 0 {
            self.seat.pointer.target = self.surface_under(self.seat.pointer.position);
            if let Some(target) = self.seat.pointer.target {
                self.send_pointer_enter(target, self.seat.pointer.position);
            }
        }

        // Checked before any client routing: the grab took the target away, so returning early
        // below would leave the shell holding the pointer for ever.
        if let Some((connection, control)) = self.seat.pointer.shell_grab {
            if !pressed {
                self.seat.pointer.buttons_held = self.seat.pointer.buttons_held.saturating_sub(1);
                if self.seat.pointer.buttons_held == 0 {
                    self.seat.pointer.shell_grab = None;
                    self.push_event(
                        connection,
                        control,
                        EventKind::ShellGrabEnd { seat: SeatId(1) },
                    );
                    self.seat.pointer.target = self.surface_under(self.seat.pointer.position);
                    if let Some(target) = self.seat.pointer.target {
                        self.send_pointer_enter(target, self.seat.pointer.position);
                    }
                }
            } else {
                self.seat.pointer.buttons_held = self.seat.pointer.buttons_held.saturating_add(1);
            }
            return;
        }

        let Some(target) = self.seat.pointer.target else {
            return;
        };
        self.seat.serial = self.seat.serial.wrapping_add(1);
        let serial = self.seat.serial;
        let epoch = self.seat.epoch;
        // On the pointer object, not the surface: the surface is what it is over.
        self.send_to_pointers(target.connection, |_| EventKind::PointerButton {
            serial,
            time_ns,
            button,
            pressed,
            epoch,
        });

        if pressed {
            self.seat.pointer.buttons_held = self.seat.pointer.buttons_held.saturating_add(1);
            if let Some(handle) = self.handle_of_surface(target) {
                let local = self.surface_local(target, self.seat.pointer.position);
                self.tell_shells(EventKind::ShellInteraction {
                    seat: SeatId(1),
                    handle,
                    serial,
                    kind: InteractionKind::PointerPress,
                    position: local,
                });
            }
        } else {
            self.seat.pointer.buttons_held = self.seat.pointer.buttons_held.saturating_sub(1);
            if self.seat.pointer.buttons_held == 0 {
                let under = self.surface_under(self.seat.pointer.position);
                if under != Some(target) {
                    self.send_pointer_leave(target);
                    self.seat.pointer.target = under;
                    if let Some(new) = under {
                        self.send_pointer_enter(new, self.seat.pointer.position);
                    }
                }
            }
        }
    }

    /// Which modifiers are held on the seat.
    #[must_use]
    pub const fn modifiers(&self) -> Modifiers {
        self.seat.keyboard.modifiers
    }

    /// Press or release a key, routing it to whatever holds focus. The key is a physical
    /// position; what it means depends on a layout the compositor does not own.
    pub fn key(&mut self, key: PhysicalKey, pressed: bool, time_ns: u64) {
        let changed = if pressed {
            self.seat.keyboard.held.len() < MAX_HELD_KEYS && self.seat.keyboard.held.insert(key)
        } else {
            self.seat.keyboard.held.remove(&key)
        };
        if !changed {
            // Already down, or not down. Neither is a state change.
            return;
        }
        if let Some(modifier) = Modifiers::of(key) {
            self.seat.keyboard.modifiers = if pressed {
                self.seat.keyboard.modifiers.with(modifier)
            } else {
                self.seat.keyboard.modifiers.without(modifier)
            };
        }

        // Matched before anything is routed. A chord the shell claimed must not also reach the
        // application: a window that saw the Q of Super+Q would act on a key nobody meant for it.
        if pressed && let Some(claim) = self.claimed(key) {
            self.seat.keyboard.consumed.insert(key);
            self.seat.serial = self.seat.serial.wrapping_add(1);
            let serial = self.seat.serial;
            let epoch = self.seat.epoch;
            self.push_event(
                claim.holder,
                claim.manager,
                EventKind::ShortcutTriggered {
                    shortcut: claim.shortcut,
                    seat: SeatId(1),
                    serial,
                    time_ns,
                    epoch,
                },
            );
            return;
        }
        // A press that was swallowed takes its release with it. Half a transition is not a state
        // a client can make sense of.
        if !pressed && self.seat.keyboard.consumed.remove(&key) {
            return;
        }

        // While an input method is composing, the keys are its raw material. The field is sent
        // what they became, not what they were: for Korean or Japanese there is no useful
        // correspondence between the two.
        if self.composing.is_some()
            && let Some((connection, object)) = self.method
        {
            self.seat.serial = self.seat.serial.wrapping_add(1);
            let serial = self.seat.serial;
            let epoch = self.seat.epoch;
            self.push_event(
                connection,
                object,
                EventKind::Key {
                    serial,
                    time_ns,
                    key,
                    pressed,
                    epoch,
                },
            );
            return;
        }

        let Some(target) = self.keyboard_focus else {
            return;
        };
        self.seat.serial = self.seat.serial.wrapping_add(1);
        let serial = self.seat.serial;
        let epoch = self.seat.epoch;
        let modifiers = self.seat.keyboard.modifiers;
        self.send_to_keyboards(
            target.connection,
            EventKind::Key {
                serial,
                time_ns,
                key,
                pressed,
                epoch,
            },
        );
        if Modifiers::of(key).is_some() {
            self.send_to_keyboards(
                target.connection,
                EventKind::KeyModifiers { modifiers, epoch },
            );
        }
    }

    /// Claim a chord for a holder that may claim chords.
    ///
    /// One holder per chord per seat. Last-registration-wins would let anything that bound the
    /// manager silently take a chord out from under the shell, so a second claim is refused and
    /// the holder is told.
    pub(super) fn register_shortcut(
        &mut self,
        connection: ConnectionId,
        manager: ObjectId,
        shortcut: u32,
        seat: SeatId,
        chord: Chord,
    ) -> Result<(), StateError> {
        self.expect_kind(connection, manager, ObjectKind::Shortcuts)?;
        if self.shortcuts.len() >= MAX_SHORTCUTS {
            return Err(StateError::QuotaExceeded { object_id: manager });
        }
        if self.shortcuts.contains_key(&(seat, chord)) {
            return Err(StateError::InvalidState { object_id: manager });
        }
        self.shortcuts.insert(
            (seat, chord),
            ShortcutClaim {
                holder: connection,
                manager,
                shortcut,
            },
        );
        Ok(())
    }

    /// The chord this key completes, if somebody has claimed it.
    pub(super) fn claimed(&self, key: PhysicalKey) -> Option<ShortcutClaim> {
        let held = self.seat.keyboard.modifiers;
        self.shortcuts
            .iter()
            .find(|((seat, chord), _)| {
                *seat == SeatId(1) && chord.trigger == key && chord.accepts(held)
            })
            .map(|(_, claim)| *claim)
    }

    /// Send to every keyboard object a connection holds.
    pub(super) fn send_to_keyboards(&mut self, connection: ConnectionId, event: EventKind) {
        let keyboards: Vec<_> = self
            .connections
            .get(&connection)
            .map(|client| {
                client
                    .registry
                    .ids()
                    .filter(|id| client.registry.kind_of(*id) == Some(ObjectKind::Keyboard))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        for keyboard in keyboards {
            self.push_event(connection, keyboard, event.clone());
        }
    }

    /// The topmost surface containing a point.
    pub(super) fn surface_under(&self, point: Point) -> Option<SurfaceKey> {
        self.scene.stack.iter().rev().copied().find(|surface| {
            self.surface_area(*surface)
                .is_some_and(|area| area.contains(point))
        })
    }

    /// Where a surface is on screen, and how big.
    pub(super) fn surface_area(&self, surface: SurfaceKey) -> Option<Rect> {
        let position = self.scene.positions.get(&surface)?;
        let state = self.surface(surface.connection, surface.object_id).ok()?;
        let current = state.current.as_ref()?;
        let buffer = self
            .buffer(surface.connection, current.snapshot.buffer)
            .ok()?;
        Some(Rect {
            x: position.x.checked_add(current.snapshot.offset.x)?,
            y: position.y.checked_add(current.snapshot.offset.y)?,
            width: buffer.descriptor.size.width,
            height: buffer.descriptor.size.height,
        })
    }

    /// A point in a surface's own coordinates.
    pub(super) fn surface_local(&self, surface: SurfaceKey, point: Point) -> Point {
        self.surface_area(surface).map_or(point, |area| Point {
            x: point.x.saturating_sub(area.x),
            y: point.y.saturating_sub(area.y),
        })
    }

    /// Send to every pointer object the connection holds; each is a view of the same seat.
    pub(super) fn send_to_pointers(
        &mut self,
        connection: ConnectionId,
        make: impl Fn(ObjectId) -> EventKind,
    ) {
        let pointers: Vec<_> = self
            .connections
            .get(&connection)
            .map(|client| {
                client
                    .registry
                    .ids()
                    .filter(|id| client.registry.kind_of(*id) == Some(ObjectKind::Pointer))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        for pointer in pointers {
            let event = make(pointer);
            self.push_event(connection, pointer, event);
        }
    }

    pub(super) fn send_pointer_enter(&mut self, target: SurfaceKey, position: Point) {
        self.seat.serial = self.seat.serial.wrapping_add(1);
        let serial = self.seat.serial;
        let epoch = self.seat.epoch;
        let local = self.surface_local(target, position);
        self.send_to_pointers(target.connection, |_| EventKind::PointerEnter {
            serial,
            surface: target.object_id,
            position: local,
            epoch,
        });
    }

    pub(super) fn send_pointer_leave(&mut self, target: SurfaceKey) {
        self.seat.serial = self.seat.serial.wrapping_add(1);
        let serial = self.seat.serial;
        let epoch = self.seat.epoch;
        self.send_to_pointers(target.connection, |_| EventKind::PointerLeave {
            serial,
            surface: target.object_id,
            epoch,
        });
    }

    pub(super) fn send_pointer_motion(
        &mut self,
        target: SurfaceKey,
        position: Point,
        time_ns: u64,
    ) {
        let epoch = self.seat.epoch;
        let local = self.surface_local(target, position);
        self.send_to_pointers(target.connection, |_| EventKind::PointerMotion {
            time_ns,
            position: local,
            epoch,
        });
    }
}
