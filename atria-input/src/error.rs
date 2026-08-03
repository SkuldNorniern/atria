use std::error::Error;
use std::fmt;

/// A failed precondition at the evdev or normalization boundary.
#[derive(Debug, Eq, PartialEq)]
pub enum InputError {
    DeviceAbsent { event_index: u32 },
    OpenDevice { event_index: u32, errno: i32 },
    QueryEventTypes { event_index: u32, errno: i32 },
    QueryKeyCodes { event_index: u32, errno: i32 },
    QueryKeyState { event_index: u32, errno: i32 },
    SetMonotonicClock { event_index: u32, errno: i32 },
    DeviceUnhandled { event_index: u32 },
    ReadEvents { event_index: u32, errno: i32 },
    InvalidReadLength { bytes: usize },
    ReacquisitionRequired,
    ReacquisitionNotRequired,
    EpochOverflow,
    ReportTooLarge,
    InvalidTimestamp { seconds: i64, microseconds: i64 },
    TimestampOverflow,
    InvalidKeyValue(i32),
}

impl fmt::Display for InputError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DeviceAbsent { event_index } => {
                write!(formatter, "/dev/input/event{event_index} is absent")
            }
            Self::OpenDevice { event_index, errno } => write!(
                formatter,
                "opening /dev/input/event{event_index} failed with errno {errno}"
            ),
            Self::QueryEventTypes { event_index, errno } => write!(
                formatter,
                "querying event types for /dev/input/event{event_index} failed with errno {errno}"
            ),
            Self::QueryKeyCodes { event_index, errno } => write!(
                formatter,
                "querying key codes for /dev/input/event{event_index} failed with errno {errno}"
            ),
            Self::QueryKeyState { event_index, errno } => write!(
                formatter,
                "querying key state for /dev/input/event{event_index} failed with errno {errno}"
            ),
            Self::SetMonotonicClock { event_index, errno } => write!(
                formatter,
                "selecting monotonic timestamps for /dev/input/event{event_index} failed with errno {errno}"
            ),
            Self::DeviceUnhandled { event_index } => write!(
                formatter,
                "/dev/input/event{event_index} has no key class normalized by Atria"
            ),
            Self::ReadEvents { event_index, errno } => write!(
                formatter,
                "reading /dev/input/event{event_index} failed with errno {errno}"
            ),
            Self::InvalidReadLength { bytes } => write!(
                formatter,
                "evdev returned {bytes} bytes instead of complete input_event records"
            ),
            Self::ReacquisitionRequired => formatter
                .write_str("evdev key state must be reacquired after an input discontinuity"),
            Self::ReacquisitionNotRequired => {
                formatter.write_str("evdev key state reacquisition has no pending discontinuity")
            }
            Self::EpochOverflow => formatter.write_str("the input epoch cannot be advanced"),
            Self::ReportTooLarge => {
                formatter.write_str("an evdev report exceeds the bounded input queue")
            }
            Self::InvalidTimestamp {
                seconds,
                microseconds,
            } => write!(
                formatter,
                "evdev timestamp {seconds}s {microseconds}us is outside the valid range"
            ),
            Self::TimestampOverflow => {
                formatter.write_str("an evdev timestamp is not representable in nanoseconds")
            }
            Self::InvalidKeyValue(value) => {
                write!(
                    formatter,
                    "evdev key value {value} is not press, release, or repeat"
                )
            }
        }
    }
}

impl Error for InputError {}
