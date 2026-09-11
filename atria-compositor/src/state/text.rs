//! Turning keys into text: which field is being composed into, and by whom.
//!
//! A child of `state` because composition is routing — text goes where the keys would have gone.
//! Kept apart from the input path because the questions differ: that one asks who is told, this
//! one asks what the keys became.

use atria_protocol::ObjectId;

use super::CompositorState;
use crate::model::{EventKind, Rect, TextBuffer};

impl CompositorState {
    /// Where the caret of the field being composed into is, for a shell placing a candidate
    /// window. Nothing is being composed into means there is nowhere to put one.
    #[must_use]
    pub fn composing_caret(&self) -> Option<Rect> {
        self.composing.map(|field| field.caret)
    }

    /// Whatever the method should be composing into now, told to both sides.
    ///
    /// The field the method composes into follows keyboard focus: text goes where the keys would
    /// have gone. A password field is never composed into, because an input method sees every key
    /// of whatever it is composing for, and that is not a thing to point at a password.
    pub(super) fn refresh_composition(&mut self) {
        let wanted = self.keyboard_focus.and_then(|surface| {
            self.fields
                .values()
                .find(|field| field.connection == surface.connection)
                .copied()
                .filter(|field| field.purpose.admits_composition())
        });

        let same = match (self.composing, wanted) {
            (Some(was), Some(now)) => was.connection == now.connection && was.object == now.object,
            (None, None) => true,
            _ => false,
        };
        if same {
            return;
        }

        if let Some(was) = self.composing.take() {
            let surface = self
                .keyboard_focus
                .map_or(ObjectId::NULL, |key| key.object_id);
            // The preedit is cleared before the field is let go of. Text being composed is a
            // proposal, and a proposal nobody is going to finish must not be left on the screen.
            self.push_event(
                was.connection,
                was.object,
                EventKind::TextPreedit {
                    composition: self.composition,
                    text: TextBuffer::default(),
                    cursor_begin: -1,
                    cursor_end: -1,
                },
            );
            self.push_event(was.connection, was.object, EventKind::TextLeave { surface });
            if let Some((connection, object)) = self.method {
                self.push_event(connection, object, EventKind::MethodDeactivated);
            }
        }
        if let Some(now) = wanted
            && let Some((connection, object)) = self.method
        {
            let surface = self
                .keyboard_focus
                .map_or(ObjectId::NULL, |key| key.object_id);
            self.composition = self.composition.wrapping_add(1);
            self.composing = Some(now);
            self.push_event(now.connection, now.object, EventKind::TextEnter { surface });
            self.push_event(
                connection,
                object,
                EventKind::MethodActivated {
                    surface,
                    purpose: now.purpose,
                    composition: self.composition,
                },
            );
        }
    }

    /// Send to the field the method is composing into, if this is still that composition.
    ///
    /// Text naming a composition that has ended is dropped, not delivered. Without this a commit
    /// that crossed with a focus change lands in whatever field is there now — which is how a
    /// password ends up in a chat window.
    pub(super) fn tell_field(&mut self, composition: u32, event: EventKind) {
        if composition != self.composition {
            return;
        }
        let Some(field) = self.composing else {
            return;
        };
        self.push_event(field.connection, field.object, event);
    }
}
