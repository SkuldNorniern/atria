//! What a viewer did, kept in the order it happened.
//!
//! A pointer position is state, so a run of motion collapses to its last. A key or a button is a
//! transition, and dropping one leaves the compositor believing something is held that is not.

use core::mem::take;

/// Where the pointer is.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PointerInput {
    pub x: i32,
    pub y: i32,
}

/// A pointer button changing state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ButtonInput {
    /// Which button, counting from one, as RFB numbers them.
    pub button: u32,
    pub pressed: bool,
}

/// A key changing state, by physical position.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct KeyInput {
    /// A USB HID Keyboard/Keypad usage.
    pub usage: u32,
    pub pressed: bool,
}

/// What a viewer did.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Input {
    /// Where the pointer is. The newest carries everything an older one did.
    Pointer(PointerInput),
    /// A button changed state. A transition, so it may not be dropped.
    Button(ButtonInput),
    /// A key changed state. A transition, so it may not be dropped.
    Key(KeyInput),
}

/// How many entries are kept between reads. Past this the routing world is restarted.
const MAX_PENDING: usize = 256;

/// Input waiting to be read.
#[derive(Debug, Default)]
pub struct InputQueue {
    events: Vec<Input>,
    /// The button mask this viewer last reported, so a report becomes transitions.
    buttons: u8,
    /// Set when a transition could not be kept.
    overflowed: bool,
    /// Set when what is held can no longer be trusted.
    stale: bool,
}

impl InputQueue {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Queue a `PointerEvent` body: a button mask, then x and y.
    ///
    /// RFB reports the whole mask each time; the transitions are derived here.
    pub fn pointer(&mut self, body: &[u8]) {
        if body.len() < 5 {
            return;
        }
        let mask = body[0];
        let x = i32::from(u16::from_be_bytes([body[1], body[2]]));
        let y = i32::from(u16::from_be_bytes([body[3], body[4]]));
        self.push(Input::Pointer(PointerInput { x, y }));

        let changed = self.buttons ^ mask;
        self.buttons = mask;
        for bit in 0..8_u32 {
            if changed & (1 << bit) == 0 {
                continue;
            }
            self.push(Input::Button(ButtonInput {
                button: bit + 1,
                pressed: mask & (1 << bit) != 0,
            }));
        }
    }

    /// Queue a key at a physical position.
    pub fn key(&mut self, usage: u32, pressed: bool) {
        self.push(Input::Key(KeyInput { usage, pressed }));
    }

    /// Take everything waiting, in order.
    #[must_use]
    pub fn take(&mut self) -> Vec<Input> {
        take(&mut self.events)
    }

    /// Say that what is held can no longer be trusted.
    ///
    /// A viewer that loses focus mid-chord never sends the release: the operating system it runs
    /// on takes the key and the viewer hears no more about it. The compositor would go on
    /// believing that modifier is down for ever, and every chord after it would match wrongly.
    /// This is the moment to let go of everything rather than guess which key was lost.
    pub fn forget_held(&mut self) {
        self.stale = true;
        self.buttons = 0;
    }

    /// Whether what is held should be let go of.
    #[must_use]
    pub fn is_stale(&mut self) -> bool {
        take(&mut self.stale)
    }

    /// Whether a transition was lost. The answer is a new routing epoch, not a guess.
    #[must_use]
    pub fn overflowed(&mut self) -> bool {
        take(&mut self.overflowed)
    }

    fn push(&mut self, input: Input) {
        if matches!(input, Input::Pointer(_))
            && matches!(self.events.last(), Some(Input::Pointer(_)))
        {
            // A newer position carries everything the one it replaces did.
            self.events.pop();
            self.events.push(input);
            return;
        }
        if self.events.len() >= MAX_PENDING {
            // Half a transition is worse than none. Say so, and let the compositor start a
            // routing world that nothing from this one belongs to.
            self.overflowed = true;
            return;
        }
        self.events.push(input);
    }
}
