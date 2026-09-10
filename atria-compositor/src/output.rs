//! Displays, and the identity that survives one being unplugged.
//!
//! A display that comes back must come back as *the same display*. Everything a shell remembers
//! about a screen — which windows lived on it, how they were arranged — is keyed on that, so an
//! identity that changes across a disconnect is a desktop that forgets every time a cable moves.
//! That is the failure this module exists to prevent, and it is a mechanism the compositor owes
//! the shell rather than a policy the compositor should have opinions about.
//!
//! The compositor supplies identity and remembers association. Where a window actually goes is
//! the shell's to decide, from what it remembers against these identities.

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use crate::model::{Point, Rect, Size};

/// A display's durable identity.
///
/// Opaque, and derived by the backend from what the hardware reports — a manufacturer, product
/// and serial where one exists, and the connector it is attached to where one does not. How it is
/// derived is deliberately not in the protocol: naming EDID would bake one platform's discovery
/// into a wire format meant to outlive it.
///
/// The contract is what matters, and it is strict: **the same physical display, presented to the
/// same machine, yields the same identity** — across unplug and replug, across a mode change,
/// across a compositor restart, and across a reboot.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct OutputIdentity(pub u128);

/// What an identity was derived from, and therefore how much it can be trusted.
///
/// Reported rather than hidden. Two displays of one model with no serial in their descriptor are
/// genuinely indistinguishable, and an identity for them can only be derived from where they are
/// plugged in — so swapping the cables swaps their identities, and anything keyed to them follows
/// the port rather than the panel.
///
/// Systems that hide this guess, and are silently wrong on exactly that hardware. Saying which
/// kind of identity this is lets a shell decide: keep a positional arrangement if the user has
/// not moved anything, and ask rather than assume when the set of ports has changed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IdentitySource {
    /// The display asserts something unique to it — a serial, or an equivalent. Moving it to
    /// another port does not change the identity.
    Panel,
    /// Derived from the connector, because the display asserts nothing unique. Two identical
    /// panels swapped between ports exchange identities, and this is the case that says so.
    Position,
}

/// A display's current properties.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OutputInfo {
    /// Resolution in device pixels.
    pub size: Size,
    /// Physical size in millimetres, which is what a real DPI is computed from.
    pub physical_millimetres: Size,
    /// Scale as an exact ratio. A 1.5 scale is 3/2, and no integer expresses it.
    pub scale_numerator: u32,
    pub scale_denominator: u32,
    /// Refresh in millihertz, so 59.94 Hz is exact.
    pub refresh_millihertz: u32,
}

impl OutputInfo {
    /// Whether the scale is a usable ratio in lowest terms.
    ///
    /// Required in lowest terms so two scales are equal exactly when their fields are equal,
    /// which is what makes them comparable and cacheable without normalising at every use.
    #[must_use]
    pub const fn scale_is_canonical(&self) -> bool {
        if self.scale_numerator == 0 || self.scale_denominator == 0 {
            return false;
        }
        if self.scale_numerator > MAX_SCALE_TERM || self.scale_denominator > MAX_SCALE_TERM {
            return false;
        }
        greatest_common_divisor(self.scale_numerator, self.scale_denominator) == 1
    }
}

/// Bound on either term of a scale ratio.
///
/// A `u32` pair is what the wire carries; the bound is what stops a coordinate multiplied by a
/// ratio from overflowing an intermediate.
pub const MAX_SCALE_TERM: u32 = 65_536;

/// Whether two rectangles share any area.
const fn overlaps(a: Rect, b: Rect) -> bool {
    let a_right = a.x.saturating_add_unsigned(a.width);
    let a_bottom = a.y.saturating_add_unsigned(a.height);
    let b_right = b.x.saturating_add_unsigned(b.width);
    let b_bottom = b.y.saturating_add_unsigned(b.height);
    a.x < b_right && b.x < a_right && a.y < b_bottom && b.y < a_bottom
}

/// The position that puts `area` wholly inside `screen`, or as far in as it fits.
const fn clamp_into(screen: Rect, area: Rect) -> Point {
    let max_x = screen
        .x
        .saturating_add_unsigned(screen.width.saturating_sub(area.width));
    let max_y = screen
        .y
        .saturating_add_unsigned(screen.height.saturating_sub(area.height));
    let x = if area.x < screen.x {
        screen.x
    } else if area.x > max_x {
        max_x
    } else {
        area.x
    };
    let y = if area.y < screen.y {
        screen.y
    } else if area.y > max_y {
        max_y
    } else {
        area.y
    };
    Point { x, y }
}

const fn greatest_common_divisor(a: u32, b: u32) -> u32 {
    let (mut a, mut b) = (a, b);
    while b != 0 {
        let next = a % b;
        a = b;
        b = next;
    }
    a
}

/// What the compositor knows about one display, present or not.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Output {
    info: OutputInfo,
    source: IdentitySource,
    present: bool,
    /// Where the shell has placed this display in the scene. `None` until it says.
    position: Option<Point>,
}

/// What one atomic topology change did.
///
/// A dock carrying three displays is one change, not three. Reporting it as three makes a shell
/// rearrange three times, and a user watch their windows move three times — which is what
/// happens on every system that treats each connector as its own event.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TopologyDelta {
    /// Displays that were not present and now are, and were never seen before.
    pub arrived: Vec<OutputIdentity>,
    /// Displays that were not present and now are, and are recognised from before.
    pub returned: Vec<OutputIdentity>,
    /// Displays that were present and now are not. Their memory is kept.
    pub departed: Vec<OutputIdentity>,
    /// Displays that stayed present and changed what they report.
    pub reconfigured: Vec<OutputIdentity>,
}

impl TopologyDelta {
    /// Whether anything changed at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.arrived.is_empty()
            && self.returned.is_empty()
            && self.departed.is_empty()
            && self.reconfigured.is_empty()
    }
}

/// Every display this compositor has seen, and what it last knew about each.
///
/// An absent display is kept rather than dropped. That is the whole point: dropping it is what
/// makes a replug look like a new screen and loses everything keyed to the old one.
#[derive(Clone, Debug, Default)]
pub struct OutputSet {
    outputs: BTreeMap<OutputIdentity, Output>,
}

impl OutputSet {
    #[must_use]
    pub fn new() -> Self {
        Self {
            outputs: BTreeMap::new(),
        }
    }

    /// Apply one whole topology change and report what it did.
    ///
    /// `present` is the complete set of attached displays afterwards, with what each reports.
    /// Anything known and not listed has gone. Taking the whole set rather than one display at a
    /// time is what makes a dock arriving one change: the shell rearranges once, from one
    /// picture, instead of once per connector while the picture is still assembling.
    ///
    /// A display in the set that was already present and reports something different is
    /// reconfigured, never detached and reattached. Removing and re-adding a display to change
    /// its mode destroys everything keyed to it, and is why changing a resolution elsewhere
    /// rearranges every window on the desktop.
    pub fn apply(
        &mut self,
        present: &[(OutputIdentity, IdentitySource, OutputInfo)],
    ) -> TopologyDelta {
        let mut delta = TopologyDelta::default();

        for (identity, source, info) in present {
            match self.outputs.get_mut(identity) {
                Some(output) if output.present => {
                    if output.info != *info {
                        output.info = *info;
                        delta.reconfigured.push(*identity);
                    }
                    output.source = *source;
                }
                Some(output) => {
                    // Known, and absent until now: a reconnection. It keeps its identity, its
                    // remembered position, and anything the shell keyed to it.
                    output.info = *info;
                    output.source = *source;
                    output.present = true;
                    delta.returned.push(*identity);
                }
                None => {
                    self.outputs.insert(
                        *identity,
                        Output {
                            info: *info,
                            source: *source,
                            present: true,
                            position: None,
                        },
                    );
                    delta.arrived.push(*identity);
                }
            }
        }

        for (identity, output) in &mut self.outputs {
            if output.present && !present.iter().any(|(listed, _, _)| listed == identity) {
                output.present = false;
                delta.departed.push(*identity);
            }
        }

        delta
    }

    /// Place a display in the scene. Where displays sit relative to one another is the shell's
    /// arrangement, and the compositor needs it to answer whether anything is reachable.
    pub fn place(&mut self, identity: OutputIdentity, position: Point) -> bool {
        match self.outputs.get_mut(&identity) {
            Some(output) => {
                output.position = Some(output.position.map_or(position, |_| position));
                true
            }
            None => false,
        }
    }

    /// The area a display occupies in the scene, if it is present and placed.
    #[must_use]
    pub fn area(&self, identity: OutputIdentity) -> Option<Rect> {
        let output = self.outputs.get(&identity)?;
        if !output.present {
            return None;
        }
        let position = output.position?;
        Some(Rect {
            x: position.x,
            y: position.y,
            width: output.info.size.width,
            height: output.info.size.height,
        })
    }

    /// What an identity was derived from, and therefore how far it can be trusted.
    #[must_use]
    pub fn source(&self, identity: OutputIdentity) -> Option<IdentitySource> {
        self.outputs.get(&identity).map(|output| output.source)
    }

    /// Whether a rectangle overlaps any attached, placed display.
    ///
    /// The invariant a desktop needs and few deliver: nothing a user owns may end up somewhere
    /// they cannot reach it. A window on a display that has gone, or one left beyond the edge
    /// after a display shrank, is lost without a menu item to fetch it back.
    #[must_use]
    pub fn is_reachable(&self, area: Rect) -> bool {
        self.outputs
            .keys()
            .filter_map(|identity| self.area(*identity))
            .any(|screen| overlaps(screen, area))
    }

    /// Move `area` the shortest distance that makes it reachable, or `None` when no display is
    /// attached and placed to move it onto.
    ///
    /// Returns the position it should take. Choosing the nearest display rather than the primary
    /// one keeps a window near where its user last saw it, instead of collecting everything onto
    /// one screen the way a system that only knows a primary display must.
    #[must_use]
    pub fn nearest_reachable(&self, area: Rect) -> Option<Point> {
        if self.is_reachable(area) {
            return Some(Point {
                x: area.x,
                y: area.y,
            });
        }
        self.outputs
            .keys()
            .filter_map(|identity| self.area(*identity))
            .map(|screen| clamp_into(screen, area))
            .min_by_key(|candidate| {
                let dx = i64::from(candidate.x) - i64::from(area.x);
                let dy = i64::from(candidate.y) - i64::from(area.y);
                dx * dx + dy * dy
            })
    }

    /// Whether this display is currently attached.
    #[must_use]
    pub fn is_present(&self, identity: OutputIdentity) -> bool {
        self.outputs
            .get(&identity)
            .is_some_and(|output| output.present)
    }

    /// Whether this display has ever been seen, present or not.
    #[must_use]
    pub fn is_known(&self, identity: OutputIdentity) -> bool {
        self.outputs.contains_key(&identity)
    }

    /// What a display last reported, whether or not it is attached now.
    #[must_use]
    pub fn info(&self, identity: OutputIdentity) -> Option<OutputInfo> {
        self.outputs.get(&identity).map(|output| output.info)
    }

    /// Every display currently attached.
    #[must_use]
    pub fn present(&self) -> Vec<OutputIdentity> {
        self.outputs
            .iter()
            .filter(|(_, output)| output.present)
            .map(|(identity, _)| *identity)
            .collect()
    }

    /// How many displays are remembered, attached or not.
    #[must_use]
    pub fn len(&self) -> usize {
        self.outputs.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.outputs.is_empty()
    }

    /// Forget a display entirely.
    ///
    /// Deliberately separate from [`Self::detach`], and not something unplugging does. Anything
    /// keyed to this identity is lost, so it is for a caller that means to discard the memory —
    /// a user removing a display from their arrangement, not a cable coming out.
    pub fn forget(&mut self, identity: OutputIdentity) -> bool {
        self.outputs.remove(&identity).is_some()
    }
}
