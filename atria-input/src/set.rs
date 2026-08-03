use std::time::Duration;

use libc::{POLLIN, c_int, pollfd};

use crate::readiness::{classify_descriptor, readable_indices};
use crate::uapi::poll_descriptors;
use crate::{DeviceClass, InputBatch, InputDevice, InputError, enumerate};

/// One normalized report tagged with the stable index of its producing device.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeviceInputBatch {
    event_index: u32,
    batch: InputBatch,
}

impl DeviceInputBatch {
    #[must_use]
    pub const fn event_index(&self) -> u32 {
        self.event_index
    }

    #[must_use]
    pub const fn batch(&self) -> &InputBatch {
        &self.batch
    }

    #[must_use]
    pub fn into_batch(self) -> InputBatch {
        self.batch
    }
}

/// A fault-isolated collection of key devices sharing one blocking readiness wait.
#[derive(Debug)]
pub struct InputSet {
    devices: Vec<InputDevice>,
}

impl InputSet {
    /// Opens the current evdev snapshot and retains every device Atria normalizes as keys.
    pub fn enumerate() -> Result<Self, InputError> {
        Self::new(enumerate()?)
    }

    /// Takes ownership of an explicit device list and retains its key devices.
    pub fn new(mut devices: Vec<InputDevice>) -> Result<Self, InputError> {
        devices.retain(|device| device.class() == DeviceClass::KeyboardRemote);
        if devices.is_empty() {
            return Err(InputError::EmptyInputSet);
        }
        Ok(Self { devices })
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.devices.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.devices.is_empty()
    }

    /// Waits once for all held devices and drains only descriptors reported ready.
    ///
    /// A failed device contributes a final per-device discontinuity and is retired. Other
    /// devices remain live, with their pipeline epochs and held-key state unchanged.
    pub fn read_batches(
        &mut self,
        timeout: Option<Duration>,
    ) -> Result<Vec<DeviceInputBatch>, InputError> {
        if self.devices.is_empty() {
            return Err(InputError::EmptyInputSet);
        }
        let mut descriptors = self
            .devices
            .iter()
            .map(|device| pollfd {
                fd: device.raw_fd(),
                events: POLLIN,
                revents: 0,
            })
            .collect::<Vec<_>>();
        let timeout_ms = timeout_milliseconds(timeout)?;
        if poll_descriptors(&mut descriptors, timeout_ms)
            .map_err(|errno| InputError::PollDescriptors { errno })?
            == 0
        {
            return Ok(Vec::new());
        }

        let mut batches = Vec::new();
        let mut retired = Vec::new();
        let mut readable =
            readable_indices(descriptors.iter().map(|descriptor| descriptor.revents)).peekable();
        for (index, descriptor) in descriptors.iter().enumerate() {
            let readiness = classify_descriptor(descriptor.revents);
            let device = &mut self.devices[index];
            let event_index = device.event_index();
            let mut failed = readiness.has_failed();
            let mut read_advanced_epoch = false;
            if readable.next_if_eq(&index).is_some() {
                match device.read_batches() {
                    Ok(device_batches) => {
                        batches.extend(tag_batches(event_index, device_batches));
                    }
                    Err(error) => {
                        failed = true;
                        read_advanced_epoch = matches!(error, InputError::InvalidReadLength { .. });
                    }
                }
            }
            if failed {
                let batch = if read_advanced_epoch {
                    device.current_discontinuity()
                } else {
                    device.retire()?
                };
                batches.push(DeviceInputBatch { event_index, batch });
                retired.push(index);
            }
        }
        remove_indices(&mut self.devices, &retired);
        Ok(batches)
    }
}

fn tag_batches(
    event_index: u32,
    batches: Vec<InputBatch>,
) -> impl Iterator<Item = DeviceInputBatch> {
    batches
        .into_iter()
        .map(move |batch| DeviceInputBatch { event_index, batch })
}

fn remove_indices<T>(values: &mut Vec<T>, indices: &[usize]) {
    // Callers discover indices in ascending descriptor order, so reverse removal preserves
    // every still-live index until its turn and retains the relative order of survivors.
    for index in indices.iter().rev() {
        values.remove(*index);
    }
}

fn timeout_milliseconds(timeout: Option<Duration>) -> Result<c_int, InputError> {
    let Some(timeout) = timeout else {
        return Ok(-1);
    };
    let whole_milliseconds = timeout.as_millis();
    let has_partial_millisecond = timeout.subsec_nanos() % 1_000_000 != 0;
    let milliseconds = whole_milliseconds
        .checked_add(u128::from(has_partial_millisecond))
        .ok_or(InputError::PollTimeoutTooLong { timeout })?;
    c_int::try_from(milliseconds).map_err(|_| InputError::PollTimeoutTooLong { timeout })
}

#[cfg(test)]
mod tests {
    use std::fs::File;
    use std::os::fd::OwnedFd;
    use std::os::unix::net::UnixStream;

    use super::*;
    use crate::{InputPipeline, PipelineStatus, RawInputEvent};

    fn idle_device(event_index: u32) -> (InputDevice, UnixStream) {
        let (reader, writer) = UnixStream::pair().expect("Unix stream pair should open");
        let file = File::from(OwnedFd::from(reader));
        (
            InputDevice::for_test(event_index, file, DeviceClass::KeyboardRemote),
            writer,
        )
    }

    #[test]
    fn empty_set_is_a_distinct_error() {
        assert!(matches!(
            InputSet::new(Vec::new()),
            Err(InputError::EmptyInputSet)
        ));
    }

    #[test]
    fn timeout_without_input_returns_no_batches() {
        let (device, _writer) = idle_device(4);
        let mut set = InputSet::new(vec![device]).expect("key device should form a set");
        assert_eq!(
            set.read_batches(Some(Duration::ZERO))
                .expect("timeout should not fail"),
            []
        );
        assert_eq!(set.len(), 1);
    }

    #[test]
    fn failed_device_is_removed_without_disturbing_others() {
        let ended = InputDevice::for_test(
            2,
            File::open("/dev/null").expect("null device should open"),
            DeviceClass::KeyboardRemote,
        );
        let (idle, _writer) = idle_device(7);
        let mut set = InputSet::new(vec![ended, idle]).expect("key devices should form a set");
        let batches = set
            .read_batches(Some(Duration::ZERO))
            .expect("one failed device should not fail the set");
        assert_eq!(batches.len(), 1);
        assert_eq!(batches[0].event_index(), 2);
        assert!(matches!(
            batches[0].batch(),
            InputBatch::Discontinuity { .. }
        ));
        assert_eq!(set.len(), 1);
    }

    #[test]
    fn batches_carry_their_producing_device() {
        let mut pipeline = InputPipeline::new();
        let batch = match pipeline
            .push(RawInputEvent::new(0, 0, 0, 0, 0))
            .expect("empty report should normalize")
        {
            PipelineStatus::Batch(batch) => batch,
            status => panic!("expected a batch, got {status:?}"),
        };
        let tagged = tag_batches(19, vec![batch]).collect::<Vec<_>>();
        assert_eq!(tagged[0].event_index(), 19);
    }

    #[test]
    fn removing_failed_indices_keeps_other_devices_in_order() {
        let mut devices = vec![10, 11, 12, 13];
        remove_indices(&mut devices, &[1, 3]);
        assert_eq!(devices, [10, 12]);
    }

    #[test]
    fn submillisecond_timeout_rounds_up_to_avoid_an_early_wakeup() {
        assert_eq!(timeout_milliseconds(Some(Duration::from_nanos(1))), Ok(1));
    }
}
