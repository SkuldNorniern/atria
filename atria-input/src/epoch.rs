use crate::{InputError, KeyEvent, KeyNormalizer, RawInputEvent, ReportAccumulator, ReportStatus};

/// A cut in seat, device, or routing state.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct Epoch(u64);

impl Epoch {
    const INITIAL: Self = Self(1);

    #[must_use]
    pub const fn value(self) -> u64 {
        self.0
    }
}

/// The state change that invalidated work produced under the previous epoch.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EpochChange {
    Seat,
    Device,
    Routing,
}

/// A complete normalized report or an explicit loss-of-continuity boundary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum InputBatch {
    Keys { epoch: Epoch, events: Vec<KeyEvent> },
    Discontinuity { epoch: Epoch },
}

impl InputBatch {
    #[must_use]
    pub const fn epoch(&self) -> Epoch {
        match self {
            Self::Keys { epoch, .. } | Self::Discontinuity { epoch } => *epoch,
        }
    }

    #[must_use]
    pub fn events(&self) -> &[KeyEvent] {
        match self {
            Self::Keys { events, .. } => events,
            Self::Discontinuity { .. } => &[],
        }
    }
}

/// The action required after one raw event is consumed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PipelineStatus {
    Pending,
    Batch(InputBatch),
    ReacquireKeyState,
}

/// Pure report, normalization, and epoch state shared by device and replay readers.
#[derive(Debug)]
pub struct InputPipeline {
    reports: ReportAccumulator,
    keys: KeyNormalizer,
    epoch: Epoch,
    needs_reacquisition: bool,
}

impl Default for InputPipeline {
    fn default() -> Self {
        Self::new()
    }
}

impl InputPipeline {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            reports: ReportAccumulator::new(),
            keys: KeyNormalizer::new(),
            epoch: Epoch::INITIAL,
            needs_reacquisition: false,
        }
    }

    #[must_use]
    pub const fn epoch(&self) -> Epoch {
        self.epoch
    }

    /// Rejects stale work immediately before the next delivery boundary.
    #[must_use]
    pub fn accept(&self, event: KeyEvent) -> Option<KeyEvent> {
        if event.epoch == self.epoch {
            Some(event)
        } else {
            None
        }
    }

    pub fn push(&mut self, event: RawInputEvent) -> Result<PipelineStatus, InputError> {
        if self.needs_reacquisition {
            return Err(InputError::ReacquisitionRequired);
        }
        match self.reports.push(event)? {
            ReportStatus::Pending => Ok(PipelineStatus::Pending),
            ReportStatus::Complete(report) => {
                let events = self.keys.normalize(&report, self.epoch)?;
                Ok(PipelineStatus::Batch(InputBatch::Keys {
                    epoch: self.epoch,
                    events,
                }))
            }
            ReportStatus::Discontinuity => {
                self.needs_reacquisition = true;
                Ok(PipelineStatus::ReacquireKeyState)
            }
        }
    }

    /// Rebuilds private physical state after loss without synthesizing transitions.
    pub fn reacquire(&mut self, bitmap: &[u8]) -> Result<InputBatch, InputError> {
        if !self.needs_reacquisition {
            return Err(InputError::ReacquisitionNotRequired);
        }
        self.advance(EpochChange::Device)?;
        self.keys.reacquire(bitmap);
        self.needs_reacquisition = false;
        Ok(InputBatch::Discontinuity { epoch: self.epoch })
    }

    /// Establishes the initial suppressed baseline before the first report is accepted.
    pub fn initialize_key_state(&mut self, bitmap: &[u8]) {
        self.keys.reacquire(bitmap);
    }

    /// Cuts queued work and prevents held controls from becoming visible in the new epoch.
    pub fn advance(&mut self, _change: EpochChange) -> Result<Epoch, InputError> {
        self.epoch = Epoch(
            self.epoch
                .0
                .checked_add(1)
                .ok_or(InputError::EpochOverflow)?,
        );
        self.reports.reset();
        self.keys.suppress_held();
        Ok(self.epoch)
    }
}
