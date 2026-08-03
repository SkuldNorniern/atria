use std::ffi::OsStr;
use std::fs::{File, OpenOptions};
use std::mem::size_of;
use std::os::fd::AsRawFd;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::time::Duration;

use libc::{CLOCK_MONOTONIC, EIO, ENOENT, POLLIN, c_int, pollfd};

use self::uapi::{
    EVENT_TYPE_BYTES, IOCTL_GET_EVENT_TYPES, IOCTL_GET_KEY_CODES, IOCTL_GET_KEY_STATE,
    IOCTL_SET_CLOCK_ID, InputEvent, KEY_STATE_BYTES, ioctl, poll_descriptors, read_events,
};
use super::InputBackend;
use crate::readiness::{DescriptorReadiness, classify_descriptor};
use crate::{DeviceClass, EventTypeSet, InputError, KeyCode, RawInputEvent};

pub(crate) mod uapi;

const EVENT_DEVICE_LIMIT: u32 = 256;
const READ_EVENT_COUNT: usize = 64;

/// One opened evdev node and the capability state queried from it.
#[derive(Debug)]
pub(crate) struct EvdevBackend {
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
    initial_key_state: [u8; KEY_STATE_BYTES],
}

#[derive(Debug)]
pub(crate) struct RawReports {
    events: [RawInputEvent; READ_EVENT_COUNT],
    count: usize,
}

#[derive(Debug)]
pub(crate) struct EvdevReadiness {
    descriptors: Vec<pollfd>,
    ready_count: usize,
}

impl AsRef<[RawInputEvent]> for RawReports {
    fn as_ref(&self) -> &[RawInputEvent] {
        &self.events[..self.count]
    }
}

impl InputBackend for EvdevBackend {
    type Error = InputError;
    type KeyState = [u8; KEY_STATE_BYTES];
    type Reports = RawReports;
    type Readiness = EvdevReadiness;

    fn open(event_index: u32) -> Result<Option<Self>, Self::Error> {
        open_device(event_index)
    }

    fn enumerate() -> Result<Vec<Self>, Self::Error> {
        let mut devices = Vec::new();
        for event_index in 0..EVENT_DEVICE_LIMIT {
            if let Some(device) = open_device(event_index)? {
                devices.push(device);
            }
        }
        Ok(devices)
    }

    fn event_index(&self) -> u32 {
        self.event_index
    }

    fn class(&self) -> DeviceClass {
        self.class
    }

    fn reports_key(&self, key: KeyCode) -> bool {
        // Walking the set bits through the existing forward map avoids a second, reversed
        // copy of the code table that could drift out of step with it.
        set_key_codes(&self.reported_keys).any(|code| KeyCode::from_evdev(code) == Some(key))
    }

    fn initial_key_state(&self) -> &Self::KeyState {
        &self.initial_key_state
    }

    fn read_reports(&self) -> Result<Self::Reports, Self::Error> {
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
            return Err(InputError::InvalidReadLength { bytes });
        }
        let count = bytes / size_of::<InputEvent>();
        let mut events = [RawInputEvent::ZERO; READ_EVENT_COUNT];
        for (target, event) in events.iter_mut().zip(&raw[..count]) {
            *target = RawInputEvent::new(
                event.time.tv_sec,
                event.time.tv_usec,
                event.event_type,
                event.code,
                event.value,
            );
        }
        Ok(RawReports { events, count })
    }

    fn query_key_state(&self) -> Result<Self::KeyState, Self::Error> {
        query_key_state(&self.file, self.event_index)
    }

    fn wait_readiness<'a>(
        devices: impl ExactSizeIterator<Item = &'a Self>,
        timeout: Option<Duration>,
    ) -> Result<Self::Readiness, Self::Error>
    where
        Self: 'a,
    {
        let mut descriptors = devices
            .map(|device| pollfd {
                fd: device.file.as_raw_fd(),
                events: POLLIN,
                revents: 0,
            })
            .collect::<Vec<_>>();
        let timeout_ms = timeout_milliseconds(timeout)?;
        let ready_count = poll_descriptors(&mut descriptors, timeout_ms)
            .map_err(|errno| InputError::PollDescriptors { errno })?;
        Ok(EvdevReadiness {
            descriptors,
            ready_count,
        })
    }

    fn has_ready(readiness: &Self::Readiness) -> bool {
        readiness.ready_count != 0
    }

    fn readiness_at(readiness: &Self::Readiness, index: usize) -> DescriptorReadiness {
        // Each input passed to `wait_readiness` owns exactly one descriptor at the same index.
        classify_descriptor(readiness.descriptors[index].revents)
    }
}

impl EvdevBackend {
    #[cfg(test)]
    pub(crate) fn for_test(event_index: u32, file: File, class: DeviceClass) -> Self {
        Self {
            event_index,
            file,
            class,
            reported_keys: [0; KEY_STATE_BYTES],
            initial_key_state: [0; KEY_STATE_BYTES],
        }
    }
}

fn open_device(event_index: u32) -> Result<Option<EvdevBackend>, InputError> {
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
    let initial_key_state = if class == DeviceClass::KeyboardRemote {
        query_key_state(&file, event_index)?
    } else {
        [0; KEY_STATE_BYTES]
    };
    Ok(Some(EvdevBackend {
        event_index,
        file,
        class,
        reported_keys,
        initial_key_state,
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
    Ok(EventTypeSet::from_bits(u32::from_ne_bytes(bitmap)))
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

    #[test]
    fn submillisecond_timeout_rounds_up_to_avoid_an_early_wakeup() {
        assert_eq!(timeout_milliseconds(Some(Duration::from_nanos(1))), Ok(1));
    }
}
