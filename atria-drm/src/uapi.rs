use std::fs::File;
use std::io::Error as IoError;
use std::mem::size_of;
use std::os::fd::AsRawFd;

use libc::{EIO, c_char, c_ulong, ioctl as libc_ioctl};

pub const CAP_DUMB_BUFFER: u64 = 0x1;
pub const CLIENT_CAP_ATOMIC: u64 = 3;
pub const MODE_CONNECTED: u32 = 1;
pub const MODE_PAGE_FLIP_EVENT: u32 = 0x01;
pub const MODE_ATOMIC_TEST_ONLY: u32 = 0x0100;
pub const MODE_ATOMIC_NONBLOCK: u32 = 0x0200;
pub const MODE_ATOMIC_ALLOW_MODESET: u32 = 0x0400;
pub const MODE_OBJECT_CRTC: u32 = 0xcccc_cccc;
pub const MODE_OBJECT_CONNECTOR: u32 = 0xc0c0_c0c0;
pub const MODE_OBJECT_PLANE: u32 = 0xeeee_eeee;
pub const EVENT_FLIP_COMPLETE: u32 = 0x02;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct GetCap {
    pub capability: u64,
    pub value: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct SetClientCap {
    pub capability: u64,
    pub value: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ModeInfo {
    pub clock: u32,
    pub hdisplay: u16,
    pub hsync_start: u16,
    pub hsync_end: u16,
    pub htotal: u16,
    pub hskew: u16,
    pub vdisplay: u16,
    pub vsync_start: u16,
    pub vsync_end: u16,
    pub vtotal: u16,
    pub vscan: u16,
    pub vrefresh: u32,
    pub flags: u32,
    pub mode_type: u32,
    pub name: [c_char; 32],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct CardResources {
    pub fb_id_ptr: u64,
    pub crtc_id_ptr: u64,
    pub connector_id_ptr: u64,
    pub encoder_id_ptr: u64,
    pub count_fbs: u32,
    pub count_crtcs: u32,
    pub count_connectors: u32,
    pub count_encoders: u32,
    pub min_width: u32,
    pub max_width: u32,
    pub min_height: u32,
    pub max_height: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct GetEncoder {
    pub encoder_id: u32,
    pub encoder_type: u32,
    pub crtc_id: u32,
    pub possible_crtcs: u32,
    pub possible_clones: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct GetConnector {
    pub encoders_ptr: u64,
    pub modes_ptr: u64,
    pub props_ptr: u64,
    pub prop_values_ptr: u64,
    pub count_modes: u32,
    pub count_props: u32,
    pub count_encoders: u32,
    pub encoder_id: u32,
    pub connector_id: u32,
    pub connector_type: u32,
    pub connector_type_id: u32,
    pub connection: u32,
    pub mm_width: u32,
    pub mm_height: u32,
    pub subpixel: u32,
    pub pad: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct CreateDumb {
    pub height: u32,
    pub width: u32,
    pub bpp: u32,
    pub flags: u32,
    pub handle: u32,
    pub pitch: u32,
    pub size: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct MapDumb {
    pub handle: u32,
    pub pad: u32,
    pub offset: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct DestroyDumb {
    pub handle: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct Framebuffer {
    pub fb_id: u32,
    pub width: u32,
    pub height: u32,
    pub pixel_format: u32,
    pub flags: u32,
    pub handles: [u32; 4],
    pub pitches: [u32; 4],
    pub offsets: [u32; 4],
    pub modifier: [u64; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct ModeCrtc {
    pub set_connectors_ptr: u64,
    pub count_connectors: u32,
    pub crtc_id: u32,
    pub fb_id: u32,
    pub x: u32,
    pub y: u32,
    pub gamma_size: u32,
    pub mode_valid: u32,
    pub mode: ModeInfo,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct PageFlip {
    pub crtc_id: u32,
    pub fb_id: u32,
    pub flags: u32,
    pub reserved: u32,
    pub user_data: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct PlaneResources {
    pub plane_id_ptr: u64,
    pub count_planes: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct GetPlane {
    pub plane_id: u32,
    pub crtc_id: u32,
    pub fb_id: u32,
    pub possible_crtcs: u32,
    pub gamma_size: u32,
    pub count_format_types: u32,
    pub format_type_ptr: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct ObjectProperties {
    pub props_ptr: u64,
    pub prop_values_ptr: u64,
    pub count_props: u32,
    pub obj_id: u32,
    pub obj_type: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct GetProperty {
    pub values_ptr: u64,
    pub enum_blob_ptr: u64,
    pub prop_id: u32,
    pub flags: u32,
    pub name: [c_char; 32],
    pub count_values: u32,
    pub count_enum_blobs: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct CreateBlob {
    pub data: u64,
    pub length: u32,
    pub blob_id: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct DestroyBlob {
    pub blob_id: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct AtomicCommit {
    pub flags: u32,
    pub count_objs: u32,
    pub objs_ptr: u64,
    pub count_props_ptr: u64,
    pub props_ptr: u64,
    pub prop_values_ptr: u64,
    pub reserved: u64,
    pub user_data: u64,
}

const DRM_TYPE: u32 = b'd' as u32;

#[cfg(any(target_os = "linux", target_os = "android"))]
const fn request_none(number: u32) -> c_ulong {
    ((DRM_TYPE << 8) | number) as c_ulong
}

#[cfg(any(target_os = "linux", target_os = "android"))]
const fn request_read_write<T>(number: u32) -> c_ulong {
    const READ_WRITE: u32 = 3;
    ((READ_WRITE << 30) | ((size_of::<T>() as u32) << 16) | (DRM_TYPE << 8) | number) as c_ulong
}

#[cfg(any(target_os = "linux", target_os = "android"))]
const fn request_write<T>(number: u32) -> c_ulong {
    const WRITE: u32 = 1;
    ((WRITE << 30) | ((size_of::<T>() as u32) << 16) | (DRM_TYPE << 8) | number) as c_ulong
}

#[cfg(target_os = "freebsd")]
const fn request_none(number: u32) -> c_ulong {
    (0x2000_0000_u32 | (DRM_TYPE << 8) | number) as c_ulong
}

#[cfg(target_os = "freebsd")]
const fn request_read_write<T>(number: u32) -> c_ulong {
    (0xc000_0000_u32 | (((size_of::<T>() as u32) & 0x1fff) << 16) | (DRM_TYPE << 8) | number)
        as c_ulong
}

#[cfg(target_os = "freebsd")]
const fn request_write<T>(number: u32) -> c_ulong {
    (0x8000_0000_u32 | (((size_of::<T>() as u32) & 0x1fff) << 16) | (DRM_TYPE << 8) | number)
        as c_ulong
}

pub const IOCTL_GET_CAP: c_ulong = request_read_write::<GetCap>(0x0c);
pub const IOCTL_SET_CLIENT_CAP: c_ulong = request_write::<SetClientCap>(0x0d);
pub const IOCTL_SET_MASTER: c_ulong = request_none(0x1e);
pub const IOCTL_DROP_MASTER: c_ulong = request_none(0x1f);
pub const IOCTL_MODE_GETRESOURCES: c_ulong = request_read_write::<CardResources>(0xa0);
pub const IOCTL_MODE_SETCRTC: c_ulong = request_read_write::<ModeCrtc>(0xa2);
pub const IOCTL_MODE_GETENCODER: c_ulong = request_read_write::<GetEncoder>(0xa6);
pub const IOCTL_MODE_GETCONNECTOR: c_ulong = request_read_write::<GetConnector>(0xa7);
pub const IOCTL_MODE_GETPROPERTY: c_ulong = request_read_write::<GetProperty>(0xaa);
pub const IOCTL_MODE_RMFB: c_ulong = request_read_write::<u32>(0xaf);
pub const IOCTL_MODE_CREATE_DUMB: c_ulong = request_read_write::<CreateDumb>(0xb2);
pub const IOCTL_MODE_MAP_DUMB: c_ulong = request_read_write::<MapDumb>(0xb3);
pub const IOCTL_MODE_DESTROY_DUMB: c_ulong = request_read_write::<DestroyDumb>(0xb4);
pub const IOCTL_MODE_ADDFB2: c_ulong = request_read_write::<Framebuffer>(0xb8);
pub const IOCTL_MODE_PAGE_FLIP: c_ulong = request_read_write::<PageFlip>(0xb0);
pub const IOCTL_MODE_GETPLANERESOURCES: c_ulong = request_read_write::<PlaneResources>(0xb5);
pub const IOCTL_MODE_GETPLANE: c_ulong = request_read_write::<GetPlane>(0xb6);
pub const IOCTL_MODE_OBJ_GETPROPERTIES: c_ulong = request_read_write::<ObjectProperties>(0xb9);
pub const IOCTL_MODE_ATOMIC: c_ulong = request_read_write::<AtomicCommit>(0xbc);
pub const IOCTL_MODE_CREATEPROPBLOB: c_ulong = request_read_write::<CreateBlob>(0xbd);
pub const IOCTL_MODE_DESTROYPROPBLOB: c_ulong = request_read_write::<DestroyBlob>(0xbe);

pub fn ioctl<T>(file: &File, request: c_ulong, argument: &mut T) -> Result<(), i32> {
    // SAFETY: `argument` is a live, writable value whose `repr(C)` layout matches the
    // request. The file remains open for the duration of the call, and the kernel copies
    // only the request-defined extent.
    let result = unsafe { libc_ioctl(file.as_raw_fd(), request, argument as *mut T) };
    if result == -1 { Err(errno()) } else { Ok(()) }
}

pub fn errno() -> i32 {
    IoError::last_os_error().raw_os_error().unwrap_or(EIO)
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    #[test]
    fn rust_layouts_match_the_linux_uapi() {
        assert_eq!(size_of::<ModeInfo>(), 68);
        assert_eq!(size_of::<CardResources>(), 64);
        assert_eq!(size_of::<GetConnector>(), 80);
        assert_eq!(size_of::<GetEncoder>(), 20);
        assert_eq!(size_of::<CreateDumb>(), 32);
        assert_eq!(size_of::<MapDumb>(), 16);
        assert_eq!(size_of::<DestroyDumb>(), 4);
        assert_eq!(size_of::<Framebuffer>(), 104);
        assert_eq!(size_of::<ModeCrtc>(), 104);
        assert_eq!(size_of::<PageFlip>(), 24);
        assert_eq!(size_of::<PlaneResources>(), 16);
        assert_eq!(size_of::<GetPlane>(), 32);
        assert_eq!(size_of::<ObjectProperties>(), 32);
        assert_eq!(size_of::<GetProperty>(), 64);
        assert_eq!(size_of::<CreateBlob>(), 16);
        assert_eq!(size_of::<DestroyBlob>(), 4);
        assert_eq!(size_of::<AtomicCommit>(), 56);
    }

    #[test]
    fn ioctl_numbers_match_the_linux_uapi_macros() {
        assert_eq!(IOCTL_GET_CAP, 0xc010_640c);
        assert_eq!(IOCTL_SET_CLIENT_CAP, 0x4010_640d);
        assert_eq!(IOCTL_MODE_GETRESOURCES, 0xc040_64a0);
        assert_eq!(IOCTL_MODE_SETCRTC, 0xc068_64a2);
        assert_eq!(IOCTL_MODE_PAGE_FLIP, 0xc018_64b0);
        assert_eq!(IOCTL_MODE_CREATE_DUMB, 0xc020_64b2);
        assert_eq!(IOCTL_MODE_MAP_DUMB, 0xc010_64b3);
        assert_eq!(IOCTL_MODE_DESTROY_DUMB, 0xc004_64b4);
        assert_eq!(IOCTL_MODE_ADDFB2, 0xc068_64b8);
        assert_eq!(IOCTL_MODE_ATOMIC, 0xc038_64bc);
        assert_eq!(IOCTL_MODE_CREATEPROPBLOB, 0xc010_64bd);
    }
}
