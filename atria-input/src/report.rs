use std::mem;

use crate::InputError;

pub(crate) const EV_SYN: u16 = 0;
pub(crate) const EV_KEY: u16 = 1;
pub(crate) const SYN_REPORT: u16 = 0;
pub(crate) const SYN_DROPPED: u16 = 3;
const MAX_REPORT_EVENTS: usize = 256;

/// One architecture-independent evdev record used by the pure report pipeline.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RawInputEvent {
    pub seconds: i64,
    pub microseconds: i64,
    pub event_type: u16,
    pub code: u16,
    pub value: i32,
}

impl RawInputEvent {
    #[must_use]
    pub const fn new(
        seconds: i64,
        microseconds: i64,
        event_type: u16,
        code: u16,
        value: i32,
    ) -> Self {
        Self {
            seconds,
            microseconds,
            event_type,
            code,
            value,
        }
    }
}

/// A complete evdev report with its synchronization marker removed.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RawReport {
    events: Vec<RawInputEvent>,
}

impl RawReport {
    #[must_use]
    pub fn events(&self) -> &[RawInputEvent] {
        &self.events
    }
}

/// Preserves the kernel's report boundary across arbitrary read boundaries.
#[derive(Debug, Default)]
pub struct ReportAccumulator {
    pending: Vec<RawInputEvent>,
    dropping: bool,
}

/// Progress made while assembling one evdev report.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReportStatus {
    Pending,
    Complete(RawReport),
    Discontinuity,
}

impl ReportAccumulator {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            pending: Vec::new(),
            dropping: false,
        }
    }

    /// Accepts one record and returns a batch only when `SYN_REPORT` closes it.
    pub fn push(&mut self, event: RawInputEvent) -> Result<ReportStatus, InputError> {
        if event.event_type == EV_SYN && event.code == SYN_DROPPED {
            self.pending.clear();
            self.dropping = true;
            return Ok(ReportStatus::Pending);
        }
        if event.event_type == EV_SYN && event.code == SYN_REPORT {
            if self.dropping {
                self.dropping = false;
                return Ok(ReportStatus::Discontinuity);
            }
            return Ok(ReportStatus::Complete(RawReport {
                events: mem::take(&mut self.pending),
            }));
        }
        if self.dropping {
            return Ok(ReportStatus::Pending);
        }
        if self.pending.len() >= MAX_REPORT_EVENTS {
            self.pending.clear();
            self.dropping = true;
            return Err(InputError::ReportTooLarge);
        }
        self.pending.push(event);
        Ok(ReportStatus::Pending)
    }

    /// Discards an incomplete report when device ownership or continuity changes.
    pub fn reset(&mut self) {
        self.pending.clear();
        self.dropping = false;
    }
}
