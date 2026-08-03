use std::fs::File;
use std::io::Error as IoError;
use std::mem::size_of;
use std::os::fd::AsRawFd;

use libc::{
    EIO, Ioctl, c_int, c_void, ioctl as libc_ioctl, nfds_t, poll as libc_poll, pollfd,
    read as libc_read, timeval,
};

pub const EV_KEY: u16 = 1;
pub const EV_REL: u16 = 2;
pub const EV_ABS: u16 = 3;
pub const EV_SW: u16 = 5;
pub const KEY_ESC: u16 = 1;
pub const KEY_ENTER: u16 = 28;
pub const KEY_UP: u16 = 103;
pub const KEY_LEFT: u16 = 105;
pub const KEY_RIGHT: u16 = 106;
pub const KEY_DOWN: u16 = 108;
pub const KEY_MUTE: u16 = 113;
pub const KEY_VOLUMEDOWN: u16 = 114;
pub const KEY_VOLUMEUP: u16 = 115;
pub const KEY_POWER: u16 = 116;
pub const KEY_MENU: u16 = 139;
pub const KEY_BACK: u16 = 158;
pub const KEY_NEXTSONG: u16 = 163;
pub const KEY_PLAYPAUSE: u16 = 164;
pub const KEY_PREVIOUSSONG: u16 = 165;
pub const KEY_STOPCD: u16 = 166;
pub const KEY_RECORD: u16 = 167;
pub const KEY_REWIND: u16 = 168;
pub const KEY_HOMEPAGE: u16 = 172;
pub const KEY_EXIT: u16 = 174;
pub const KEY_FASTFORWARD: u16 = 208;
pub const KEY_CANCEL: u16 = 223;
pub const KEY_OK: u16 = 0x160;
pub const KEY_SELECT: u16 = 0x161;
pub const KEY_INFO: u16 = 0x166;
pub const KEY_EPG: u16 = 0x16d;
pub const KEY_SUBTITLE: u16 = 0x172;
pub const KEY_TV: u16 = 0x179;
pub const KEY_AUDIO: u16 = 0x188;
pub const KEY_VIDEO: u16 = 0x189;
pub const KEY_RED: u16 = 0x18e;
pub const KEY_GREEN: u16 = 0x18f;
pub const KEY_YELLOW: u16 = 0x190;
pub const KEY_BLUE: u16 = 0x191;
pub const KEY_CHANNELUP: u16 = 0x192;
pub const KEY_CHANNELDOWN: u16 = 0x193;
pub const KEY_NUMERIC_0: u16 = 0x200;
pub const KEY_NUMERIC_1: u16 = 0x201;
pub const KEY_NUMERIC_2: u16 = 0x202;
pub const KEY_NUMERIC_3: u16 = 0x203;
pub const KEY_NUMERIC_4: u16 = 0x204;
pub const KEY_NUMERIC_5: u16 = 0x205;
pub const KEY_NUMERIC_6: u16 = 0x206;
pub const KEY_NUMERIC_7: u16 = 0x207;
pub const KEY_NUMERIC_8: u16 = 0x208;
pub const KEY_NUMERIC_9: u16 = 0x209;
pub const KEY_NUMERIC_STAR: u16 = 0x20a;
pub const KEY_NUMERIC_POUND: u16 = 0x20b;
pub const EVENT_TYPE_BYTES: usize = 4;
pub const KEY_STATE_BYTES: usize = 96;

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct InputEvent {
    pub time: timeval,
    pub event_type: u16,
    pub code: u16,
    pub value: i32,
}

impl InputEvent {
    pub const ZERO: Self = Self {
        time: timeval {
            tv_sec: 0,
            tv_usec: 0,
        },
        event_type: 0,
        code: 0,
        value: 0,
    };
}

const EVDEV_TYPE: u32 = b'E' as u32;

#[cfg(any(target_os = "linux", target_os = "android"))]
const fn request_read(number: u32, length: usize) -> Ioctl {
    const READ: u32 = 2;
    ((READ << 30) | ((length as u32) << 16) | (EVDEV_TYPE << 8) | number) as Ioctl
}

#[cfg(any(target_os = "linux", target_os = "android"))]
const fn request_write<T>(number: u32) -> Ioctl {
    const WRITE: u32 = 1;
    ((WRITE << 30) | ((size_of::<T>() as u32) << 16) | (EVDEV_TYPE << 8) | number) as Ioctl
}

#[cfg(target_os = "freebsd")]
const fn request_read(number: u32, length: usize) -> Ioctl {
    const IOC_OUT: u32 = 0x4000_0000;
    const IOCPARM_MASK: u32 = 0x1fff;
    (IOC_OUT | (((length as u32) & IOCPARM_MASK) << 16) | (EVDEV_TYPE << 8) | number) as Ioctl
}

#[cfg(target_os = "freebsd")]
const fn request_write<T>(number: u32) -> Ioctl {
    const IOC_IN: u32 = 0x8000_0000;
    const IOCPARM_MASK: u32 = 0x1fff;
    (IOC_IN | (((size_of::<T>() as u32) & IOCPARM_MASK) << 16) | (EVDEV_TYPE << 8) | number)
        as Ioctl
}

pub const IOCTL_GET_EVENT_TYPES: Ioctl = request_read(0x20, EVENT_TYPE_BYTES);
pub const IOCTL_GET_KEY_CODES: Ioctl = request_read(0x20 + EV_KEY as u32, KEY_STATE_BYTES);
pub const IOCTL_GET_KEY_STATE: Ioctl = request_read(0x18, KEY_STATE_BYTES);
pub const IOCTL_SET_CLOCK_ID: Ioctl = request_write::<c_int>(0xa0);

pub fn ioctl<T>(file: &File, request: Ioctl, argument: &mut T) -> Result<(), i32> {
    // SAFETY: `argument` is live and writable for the request-encoded extent, the request
    // matches its type, and `file` keeps the evdev descriptor open for the entire call.
    let result = unsafe { libc_ioctl(file.as_raw_fd(), request, argument as *mut T) };
    if result == -1 { Err(errno()) } else { Ok(()) }
}

pub fn read_events(file: &File, events: &mut [InputEvent]) -> Result<usize, i32> {
    let byte_count = events
        .len()
        .checked_mul(size_of::<InputEvent>())
        .ok_or(EIO)?;
    // SAFETY: `events` owns a writable region of `byte_count` bytes, the kernel initializes
    // only bytes reported by the return value, and `file` remains open during the read.
    let result = unsafe {
        libc_read(
            file.as_raw_fd(),
            events.as_mut_ptr().cast::<c_void>(),
            byte_count,
        )
    };
    if result == -1 {
        Err(errno())
    } else {
        usize::try_from(result).map_err(|_| EIO)
    }
}

pub fn poll_descriptors(descriptors: &mut [pollfd], timeout_ms: c_int) -> Result<usize, i32> {
    let descriptor_count = nfds_t::try_from(descriptors.len()).map_err(|_| EIO)?;
    // SAFETY: `descriptors` remains live and exclusively borrowed for all `descriptor_count`
    // entries, and every contained descriptor remains owned by its InputDevice during the call.
    let result = unsafe { libc_poll(descriptors.as_mut_ptr(), descriptor_count, timeout_ms) };
    if result == -1 {
        Err(errno())
    } else {
        usize::try_from(result).map_err(|_| EIO)
    }
}

fn errno() -> i32 {
    IoError::last_os_error().raw_os_error().unwrap_or(EIO)
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    #[test]
    fn rust_layout_matches_the_linux_uapi() {
        assert_eq!(size_of::<InputEvent>(), 24);
    }

    #[test]
    fn ioctl_numbers_match_the_linux_uapi_macros() {
        assert_eq!(IOCTL_GET_EVENT_TYPES, 0x8004_4520);
        assert_eq!(IOCTL_GET_KEY_CODES, 0x8060_4521);
        assert_eq!(IOCTL_GET_KEY_STATE, 0x8060_4518);
        assert_eq!(IOCTL_SET_CLOCK_ID, 0x4004_45a0);
    }
}

#[cfg(all(test, target_os = "freebsd"))]
mod freebsd_tests {
    use super::*;

    #[test]
    fn ioctl_numbers_match_the_freebsd_uapi_macros() {
        assert_eq!(IOCTL_GET_EVENT_TYPES, 0x4004_4520_u32 as Ioctl);
        assert_eq!(IOCTL_GET_KEY_CODES, 0x4060_4521_u32 as Ioctl);
        assert_eq!(IOCTL_GET_KEY_STATE, 0x4060_4518_u32 as Ioctl);
        assert_eq!(IOCTL_SET_CLOCK_ID, 0x8004_45a0_u32 as Ioctl);
    }
}
