#[cfg(test)]
use std::fs::File;

use crate::backend::{EvdevBackend, InputBackend};
use crate::{Epoch, EpochChange, InputBatch, InputError, InputPipeline, KeyCode, PipelineStatus};

const EVENT_TYPE_KEY: u16 = 1;
const EVENT_TYPE_RELATIVE: u16 = 2;
const EVENT_TYPE_ABSOLUTE: u16 = 3;
const EVENT_TYPE_SWITCH: u16 = 5;

/// Event classes advertised by an input device.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct EventTypeSet(u32);

impl EventTypeSet {
    #[must_use]
    pub const fn has_keys(self) -> bool {
        self.contains(EVENT_TYPE_KEY)
    }

    #[must_use]
    pub const fn has_relative_motion(self) -> bool {
        self.contains(EVENT_TYPE_RELATIVE)
    }

    #[must_use]
    pub const fn has_absolute_motion(self) -> bool {
        self.contains(EVENT_TYPE_ABSOLUTE)
    }

    #[must_use]
    pub const fn has_switches(self) -> bool {
        self.contains(EVENT_TYPE_SWITCH)
    }

    pub(crate) const fn from_bits(bits: u32) -> Self {
        Self(bits)
    }

    const fn contains(self, event_type: u16) -> bool {
        let mask = 1_u32 << event_type;
        self.0 & mask != 0
    }
}

/// Whether this package has a normalization path for an opened device.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeviceClass {
    KeyboardRemote,
    Unhandled { event_types: EventTypeSet },
}

/// One opened input source and its private report and key state.
#[derive(Debug)]
pub struct InputDevice {
    pub(crate) backend: EvdevBackend,
    event_index: u32,
    class: DeviceClass,
    pipeline: InputPipeline,
}

impl InputDevice {
    pub fn open(event_index: u32) -> Result<Self, InputError> {
        match <EvdevBackend as InputBackend>::open(event_index)? {
            Some(backend) => Ok(Self::from_backend(backend)),
            None => Err(InputError::DeviceAbsent { event_index }),
        }
    }

    #[must_use]
    pub const fn event_index(&self) -> u32 {
        self.event_index
    }

    #[must_use]
    pub const fn class(&self) -> DeviceClass {
        self.class
    }

    /// Whether the device reports it can produce `key`.
    ///
    /// Selecting a device by the keys it reports is what distinguishes a keyboard from any
    /// other key source, such as a power button or a lid switch.
    #[must_use]
    pub fn reports_key(&self, key: KeyCode) -> bool {
        InputBackend::reports_key(&self.backend, key)
    }

    #[must_use]
    pub const fn epoch(&self) -> Epoch {
        self.pipeline.epoch()
    }

    pub fn advance_epoch(&mut self, change: EpochChange) -> Result<Epoch, InputError> {
        self.pipeline.advance(change)
    }

    /// Reads available platform records and emits only complete normalized reports.
    pub fn read_batches(&mut self) -> Result<Vec<InputBatch>, InputError> {
        let reports = match InputBackend::read_reports(&self.backend) {
            Ok(reports) => reports,
            Err(error @ InputError::InvalidReadLength { .. }) => {
                self.pipeline.advance(EpochChange::Device)?;
                return Err(error);
            }
            Err(error) => return Err(error),
        };
        let mut batches = Vec::new();
        for event in reports.as_ref() {
            match self.pipeline.push(*event)? {
                PipelineStatus::Pending => {}
                PipelineStatus::Batch(batch) => batches.push(batch),
                PipelineStatus::ReacquireKeyState => {
                    let state = InputBackend::query_key_state(&self.backend)?;
                    batches.push(self.pipeline.reacquire(state.as_ref())?);
                }
            }
        }
        Ok(batches)
    }

    pub(crate) fn from_backend(backend: EvdevBackend) -> Self {
        let event_index = InputBackend::event_index(&backend);
        let class = InputBackend::class(&backend);
        let mut pipeline = InputPipeline::new();
        if class == DeviceClass::KeyboardRemote {
            pipeline.initialize_key_state(InputBackend::initial_key_state(&backend).as_ref());
        }
        Self {
            backend,
            event_index,
            class,
            pipeline,
        }
    }

    pub(crate) fn retire(&mut self) -> Result<InputBatch, InputError> {
        let epoch = self.pipeline.advance(EpochChange::Device)?;
        Ok(InputBatch::Discontinuity { epoch })
    }

    pub(crate) fn current_discontinuity(&self) -> InputBatch {
        InputBatch::Discontinuity {
            epoch: self.pipeline.epoch(),
        }
    }

    #[cfg(test)]
    pub(crate) fn for_test(event_index: u32, file: File, class: DeviceClass) -> Self {
        Self::from_backend(EvdevBackend::for_test(event_index, file, class))
    }
}

/// Opens and classifies the finite platform device snapshot.
pub fn enumerate() -> Result<Vec<InputDevice>, InputError> {
    <EvdevBackend as InputBackend>::enumerate()
        .map(|devices| devices.into_iter().map(InputDevice::from_backend).collect())
}
