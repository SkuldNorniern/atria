use std::ffi::OsStr;
use std::fs::{File, OpenOptions};
use std::mem::size_of;
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use libc::{CLOCK_MONOTONIC, EIO, ENOENT};

use crate::uapi::{
    EV_ABS, EV_KEY, EV_REL, EV_SW, EVENT_TYPE_BYTES, IOCTL_GET_EVENT_TYPES, IOCTL_GET_KEY_CODES,
    IOCTL_GET_KEY_STATE, IOCTL_SET_CLOCK_ID, InputEvent, KEY_STATE_BYTES, ioctl, read_events,
};
use crate::{
    Epoch, EpochChange, InputBatch, InputError, InputPipeline, KeyCode, PipelineStatus,
    RawInputEvent,
};

const EVENT_DEVICE_LIMIT: u32 = 256;
const READ_EVENT_COUNT: usize = 64;

/// Event classes advertised by an evdev device.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct EventTypeSet(u32);

impl EventTypeSet {
    #[must_use]
    pub const fn has_keys(self) -> bool {
        self.contains(EV_KEY)
    }

    #[must_use]
    pub const fn has_relative_motion(self) -> bool {
        self.contains(EV_REL)
    }

    #[must_use]
    pub const fn has_absolute_motion(self) -> bool {
        self.contains(EV_ABS)
    }

    #[must_use]
    pub const fn has_switches(self) -> bool {
        self.contains(EV_SW)
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

/// One opened evdev node and its private report and key state.
#[derive(Debug)]
pub struct InputDevice {
    event_index: u32,
    file: File,
    class: DeviceClass,
    /// The key codes this device reports it can produce, as returned by `EVIOCGBIT(EV_KEY)`.
    ///
    /// Kept rather than discarded after classification because "reports some key" is too
    /// coarse to choose a device by: an ACPI power button reports `KEY_POWER` and nothing
    /// else, so it classifies identically to a keyboard. A caller that needs navigation keys
    /// has to ask for them.
    reported_keys: [u8; KEY_STATE_BYTES],
    pipeline: InputPipeline,
}

impl InputDevice {
    pub fn open(event_index: u32) -> Result<Self, InputError> {
        match open_device(event_index)? {
            Some(device) => Ok(device),
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
    /// other `EV_KEY` source, such as a power button or a lid switch.
    #[must_use]
    pub fn reports_key(&self, key: KeyCode) -> bool {
        // Walking the set bits through the existing forward map avoids a second, reversed
        // copy of the code table that could drift out of step with it.
        set_key_codes(&self.reported_keys).any(|code| KeyCode::from_evdev(code) == Some(key))
    }

    #[must_use]
    pub const fn epoch(&self) -> Epoch {
        self.pipeline.epoch()
    }

    pub fn advance_epoch(&mut self, change: EpochChange) -> Result<Epoch, InputError> {
        self.pipeline.advance(change)
    }

    /// Reads available kernel records and emits only complete normalized reports.
    pub fn read_batches(&mut self) -> Result<Vec<InputBatch>, InputError> {
        if matches!(self.class, DeviceClass::Unhandled { .. }) {
            return Err(InputError::DeviceUnhandled {
                event_index: self.event_index,
            });
        }
        let mut raw = [InputEvent::ZERO; READ_EVENT_COUNT];
        let bytes = read_events(&self.file, &mut raw).map_err(|errno| InputError::ReadEvents {
            event_index: self.event_index,
            errno,
        })?;
        if bytes == 0 {
            return Err(InputError::EndOfInput {
                event_index: self.event_index,
            });
        }
        if bytes % size_of::<InputEvent>() != 0 {
            self.pipeline.advance(EpochChange::Device)?;
            return Err(InputError::InvalidReadLength { bytes });
        }
        let count = bytes / size_of::<InputEvent>();
        let mut batches = Vec::new();
        for event in &raw[..count] {
            let event = RawInputEvent::new(
                event.time.tv_sec,
                event.time.tv_usec,
                event.event_type,
                event.code,
                event.value,
            );
            match self.pipeline.push(event)? {
                PipelineStatus::Pending => {}
                PipelineStatus::Batch(batch) => batches.push(batch),
                PipelineStatus::ReacquireKeyState => {
                    let bitmap = query_key_state(&self.file, self.event_index)?;
                    batches.push(self.pipeline.reacquire(&bitmap)?);
                }
            }
        }
        Ok(batches)
    }

    pub(crate) fn raw_fd(&self) -> RawFd {
        self.file.as_raw_fd()
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
        Self {
            event_index,
            file,
            class,
            reported_keys: [0; KEY_STATE_BYTES],
            pipeline: InputPipeline::new(),
        }
    }
}

/// Opens and classifies the finite `/dev/input/eventN` snapshot.
pub fn enumerate() -> Result<Vec<InputDevice>, InputError> {
    let mut devices = Vec::new();
    for event_index in 0..EVENT_DEVICE_LIMIT {
        if let Some(device) = open_device(event_index)? {
            devices.push(device);
        }
    }
    Ok(devices)
}

fn open_device(event_index: u32) -> Result<Option<InputDevice>, InputError> {
    let (path, path_len) = device_path(event_index);
    let path = Path::new(OsStr::from_bytes(&path[..path_len]));
    let file = match OpenOptions::new().read(true).open(path) {
        Ok(file) => file,
        Err(error) if error.raw_os_error() == Some(ENOENT) => return Ok(None),
        Err(error) => {
            return Err(InputError::OpenDevice {
                event_index,
                errno: error.raw_os_error().unwrap_or(EIO),
            });
        }
    };
    let event_types = query_event_types(&file, event_index)?;
    let (class, reported_keys) = classify(&file, event_index, event_types)?;
    if class == DeviceClass::KeyboardRemote {
        let mut clock_id = CLOCK_MONOTONIC;
        ioctl(&file, IOCTL_SET_CLOCK_ID, &mut clock_id)
            .map_err(|errno| InputError::SetMonotonicClock { event_index, errno })?;
    }
    let mut pipeline = InputPipeline::new();
    if class == DeviceClass::KeyboardRemote {
        pipeline.initialize_key_state(&query_key_state(&file, event_index)?);
    }
    Ok(Some(InputDevice {
        event_index,
        file,
        class,
        reported_keys,
        pipeline,
    }))
}

fn query_key_state(file: &File, event_index: u32) -> Result<[u8; KEY_STATE_BYTES], InputError> {
    let mut bitmap = [0_u8; KEY_STATE_BYTES];
    ioctl(file, IOCTL_GET_KEY_STATE, &mut bitmap)
        .map_err(|errno| InputError::QueryKeyState { event_index, errno })?;
    Ok(bitmap)
}

fn query_event_types(file: &File, event_index: u32) -> Result<EventTypeSet, InputError> {
    let mut bitmap = [0_u8; EVENT_TYPE_BYTES];
    ioctl(file, IOCTL_GET_EVENT_TYPES, &mut bitmap)
        .map_err(|errno| InputError::QueryEventTypes { event_index, errno })?;
    Ok(EventTypeSet(u32::from_ne_bytes(bitmap)))
}

fn classify(
    file: &File,
    event_index: u32,
    event_types: EventTypeSet,
) -> Result<(DeviceClass, [u8; KEY_STATE_BYTES]), InputError> {
    let mut bitmap = [0_u8; KEY_STATE_BYTES];
    if !event_types.has_keys() {
        return Ok((DeviceClass::Unhandled { event_types }, bitmap));
    }
    ioctl(file, IOCTL_GET_KEY_CODES, &mut bitmap)
        .map_err(|errno| InputError::QueryKeyCodes { event_index, errno })?;
    if bitmap_has_supported_key(&bitmap) {
        Ok((DeviceClass::KeyboardRemote, bitmap))
    } else {
        Ok((DeviceClass::Unhandled { event_types }, bitmap))
    }
}

/// Every evdev code whose bit is set in a reported-key bitmap.
fn set_key_codes(bitmap: &[u8; KEY_STATE_BYTES]) -> impl Iterator<Item = u16> + '_ {
    bitmap.iter().enumerate().flat_map(|(byte_index, byte)| {
        (0_u8..8).filter_map(move |bit_index| {
            if byte & (1_u8 << bit_index) == 0 {
                return None;
            }
            byte_index
                .checked_mul(8)
                .and_then(|base| base.checked_add(usize::from(bit_index)))
                .and_then(|code| u16::try_from(code).ok())
        })
    })
}

fn bitmap_has_supported_key(bitmap: &[u8; KEY_STATE_BYTES]) -> bool {
    set_key_codes(bitmap).any(KeyCode::is_supported_evdev)
}

fn device_path(event_index: u32) -> ([u8; 32], usize) {
    let mut path = [0_u8; 32];
    let prefix = b"/dev/input/event";
    path[..prefix.len()].copy_from_slice(prefix);
    let mut digits = [0_u8; 10];
    let mut value = event_index;
    let mut digit_count = 0;
    loop {
        digits[digit_count] = b'0' + (value % 10) as u8;
        digit_count += 1;
        value /= 10;
        if value == 0 {
            break;
        }
    }
    for index in 0..digit_count {
        path[prefix.len() + index] = digits[digit_count - index - 1];
    }
    (path, prefix.len() + digit_count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supported_key_bitmap_selects_keyboard_remote_class() {
        let mut bitmap = [0_u8; KEY_STATE_BYTES];
        bitmap[103 / 8] |= 1 << (103 % 8);
        assert!(bitmap_has_supported_key(&bitmap));
    }

    #[test]
    fn pointer_button_bitmap_is_not_treated_as_a_key_device() {
        let mut bitmap = [0_u8; KEY_STATE_BYTES];
        let button_left = 0x110;
        bitmap[button_left / 8] |= 1 << (button_left % 8);
        assert!(!bitmap_has_supported_key(&bitmap));
    }
}
