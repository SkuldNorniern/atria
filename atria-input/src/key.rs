use crate::backend::evdev::uapi::{
    KEY_AUDIO, KEY_BACK, KEY_BLUE, KEY_CANCEL, KEY_CHANNELDOWN, KEY_CHANNELUP, KEY_DOWN, KEY_ENTER,
    KEY_EPG, KEY_ESC, KEY_EXIT, KEY_FASTFORWARD, KEY_GREEN, KEY_HOMEPAGE, KEY_INFO, KEY_LEFT,
    KEY_MENU, KEY_MUTE, KEY_NEXTSONG, KEY_NUMERIC_0, KEY_NUMERIC_1, KEY_NUMERIC_2, KEY_NUMERIC_3,
    KEY_NUMERIC_4, KEY_NUMERIC_5, KEY_NUMERIC_6, KEY_NUMERIC_7, KEY_NUMERIC_8, KEY_NUMERIC_9,
    KEY_NUMERIC_POUND, KEY_NUMERIC_STAR, KEY_OK, KEY_PLAYPAUSE, KEY_POWER, KEY_PREVIOUSSONG,
    KEY_RECORD, KEY_RED, KEY_REWIND, KEY_RIGHT, KEY_SELECT, KEY_STOPCD, KEY_SUBTITLE, KEY_TV,
    KEY_UP, KEY_VIDEO, KEY_VOLUMEDOWN, KEY_VOLUMEUP, KEY_YELLOW,
};
use crate::report::EV_KEY;
use crate::{Epoch, InputError, RawReport};

const NANOS_PER_SECOND: u64 = 1_000_000_000;
const NANOS_PER_MICROSECOND: u64 = 1_000;
const MICROSECONDS_PER_SECOND: i64 = 1_000_000;

/// A physical key whose meaning is stable across the supported evdev ABIs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum KeyCode {
    Escape,
    Enter,
    Up,
    Left,
    Right,
    Down,
    Mute,
    VolumeDown,
    VolumeUp,
    Power,
    Menu,
    Back,
    Home,
    NextTrack,
    PlayPause,
    PreviousTrack,
    Stop,
    Record,
    Rewind,
    Exit,
    FastForward,
    Cancel,
    Ok,
    Select,
    Info,
    Guide,
    Subtitle,
    Tv,
    Audio,
    Video,
    Red,
    Green,
    Yellow,
    Blue,
    ChannelUp,
    ChannelDown,
    Numeric0,
    Numeric1,
    Numeric2,
    Numeric3,
    Numeric4,
    Numeric5,
    Numeric6,
    Numeric7,
    Numeric8,
    Numeric9,
    NumericStar,
    NumericPound,
}

impl KeyCode {
    pub(crate) const COUNT: usize = 48;

    pub(crate) const fn from_evdev(code: u16) -> Option<Self> {
        match code {
            KEY_ESC => Some(Self::Escape),
            KEY_ENTER => Some(Self::Enter),
            KEY_UP => Some(Self::Up),
            KEY_LEFT => Some(Self::Left),
            KEY_RIGHT => Some(Self::Right),
            KEY_DOWN => Some(Self::Down),
            KEY_MUTE => Some(Self::Mute),
            KEY_VOLUMEDOWN => Some(Self::VolumeDown),
            KEY_VOLUMEUP => Some(Self::VolumeUp),
            KEY_POWER => Some(Self::Power),
            KEY_MENU => Some(Self::Menu),
            KEY_BACK => Some(Self::Back),
            KEY_NEXTSONG => Some(Self::NextTrack),
            KEY_PLAYPAUSE => Some(Self::PlayPause),
            KEY_PREVIOUSSONG => Some(Self::PreviousTrack),
            KEY_STOPCD => Some(Self::Stop),
            KEY_RECORD => Some(Self::Record),
            KEY_REWIND => Some(Self::Rewind),
            KEY_HOMEPAGE => Some(Self::Home),
            KEY_EXIT => Some(Self::Exit),
            KEY_FASTFORWARD => Some(Self::FastForward),
            KEY_CANCEL => Some(Self::Cancel),
            KEY_OK => Some(Self::Ok),
            KEY_SELECT => Some(Self::Select),
            KEY_INFO => Some(Self::Info),
            KEY_EPG => Some(Self::Guide),
            KEY_SUBTITLE => Some(Self::Subtitle),
            KEY_TV => Some(Self::Tv),
            KEY_AUDIO => Some(Self::Audio),
            KEY_VIDEO => Some(Self::Video),
            KEY_RED => Some(Self::Red),
            KEY_GREEN => Some(Self::Green),
            KEY_YELLOW => Some(Self::Yellow),
            KEY_BLUE => Some(Self::Blue),
            KEY_CHANNELUP => Some(Self::ChannelUp),
            KEY_CHANNELDOWN => Some(Self::ChannelDown),
            KEY_NUMERIC_0 => Some(Self::Numeric0),
            KEY_NUMERIC_1 => Some(Self::Numeric1),
            KEY_NUMERIC_2 => Some(Self::Numeric2),
            KEY_NUMERIC_3 => Some(Self::Numeric3),
            KEY_NUMERIC_4 => Some(Self::Numeric4),
            KEY_NUMERIC_5 => Some(Self::Numeric5),
            KEY_NUMERIC_6 => Some(Self::Numeric6),
            KEY_NUMERIC_7 => Some(Self::Numeric7),
            KEY_NUMERIC_8 => Some(Self::Numeric8),
            KEY_NUMERIC_9 => Some(Self::Numeric9),
            KEY_NUMERIC_STAR => Some(Self::NumericStar),
            KEY_NUMERIC_POUND => Some(Self::NumericPound),
            _ => None,
        }
    }

    pub(crate) const fn is_supported_evdev(code: u16) -> bool {
        Self::from_evdev(code).is_some()
    }

    pub(crate) const fn index(self) -> usize {
        self as usize
    }
}

/// The transition carried by an evdev `EV_KEY` record.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum KeyAction {
    Released,
    Pressed,
    Repeated,
}

/// A normalized physical key event with the device-selected timestamp in nanoseconds.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct KeyEvent {
    pub timestamp_ns: u64,
    pub code: KeyCode,
    pub action: KeyAction,
    pub epoch: Epoch,
}

/// Tracks physical key state while translating complete reports.
#[derive(Debug)]
pub struct KeyNormalizer {
    down: [bool; KeyCode::COUNT],
    suppressed: [bool; KeyCode::COUNT],
}

impl Default for KeyNormalizer {
    fn default() -> Self {
        Self::new()
    }
}

impl KeyNormalizer {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            down: [false; KeyCode::COUNT],
            suppressed: [false; KeyCode::COUNT],
        }
    }

    /// Normalizes supported key records without interpreting other event classes.
    pub fn normalize(
        &mut self,
        report: &RawReport,
        epoch: Epoch,
    ) -> Result<Vec<KeyEvent>, InputError> {
        let mut normalized = Vec::new();
        let mut next_down = self.down;
        let mut next_suppressed = self.suppressed;
        for raw in report.events() {
            if raw.event_type != EV_KEY {
                continue;
            }
            let Some(code) = KeyCode::from_evdev(raw.code) else {
                continue;
            };
            let action = match raw.value {
                0 => KeyAction::Released,
                1 => KeyAction::Pressed,
                2 => KeyAction::Repeated,
                value => return Err(InputError::InvalidKeyValue(value)),
            };
            let timestamp_ns = timestamp_ns(raw.seconds, raw.microseconds)?;
            let index = code.index();
            next_down[index] = action != KeyAction::Released;
            if next_suppressed[index] {
                if action == KeyAction::Released {
                    next_suppressed[index] = false;
                }
                continue;
            }
            normalized.push(KeyEvent {
                timestamp_ns,
                code,
                action,
                epoch,
            });
        }
        self.down = next_down;
        self.suppressed = next_suppressed;
        Ok(normalized)
    }

    #[must_use]
    pub const fn is_down(&self, code: KeyCode) -> bool {
        self.down[code.index()]
    }

    pub(crate) fn suppress_held(&mut self) {
        for (suppressed, down) in self.suppressed.iter_mut().zip(self.down) {
            *suppressed = down;
        }
    }

    pub(crate) fn reacquire(&mut self, bitmap: &[u8]) {
        for code in 0_u16..=0x2ff {
            let Some(key) = KeyCode::from_evdev(code) else {
                continue;
            };
            let byte_index = usize::from(code / 8);
            let bit_index = code % 8;
            let down = bitmap
                .get(byte_index)
                .is_some_and(|byte| byte & (1_u8 << bit_index) != 0);
            self.down[key.index()] = down;
            self.suppressed[key.index()] = down;
        }
    }
}

fn timestamp_ns(seconds: i64, microseconds: i64) -> Result<u64, InputError> {
    if seconds < 0 || !(0..MICROSECONDS_PER_SECOND).contains(&microseconds) {
        return Err(InputError::InvalidTimestamp {
            seconds,
            microseconds,
        });
    }
    let seconds = u64::try_from(seconds).map_err(|_| InputError::TimestampOverflow)?;
    let microseconds = u64::try_from(microseconds).map_err(|_| InputError::TimestampOverflow)?;
    seconds
        .checked_mul(NANOS_PER_SECOND)
        .and_then(|base| {
            microseconds
                .checked_mul(NANOS_PER_MICROSECOND)
                .and_then(|fraction| base.checked_add(fraction))
        })
        .ok_or(InputError::TimestampOverflow)
}
