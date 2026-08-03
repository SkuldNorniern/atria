use std::fs::File;
use std::io::Read;
use std::mem::size_of;
use std::slice::from_raw_parts_mut;

use atria_software_output::{Frame, FrameReport, PixelLayout};
use libc::{EACCES, EBUSY, EIO, c_char};

use crate::backend::ioctl::buffer::{IoctlBackend, ScanoutBuffer};
use crate::backend::ioctl::uapi::{
    AtomicCommit, CreateBlob, DestroyBlob, EVENT_FLIP_COMPLETE, GetPlane, GetProperty,
    IOCTL_MODE_ATOMIC, IOCTL_MODE_CREATEPROPBLOB, IOCTL_MODE_DESTROYPROPBLOB, IOCTL_MODE_GETPLANE,
    IOCTL_MODE_GETPLANERESOURCES, IOCTL_MODE_GETPROPERTY, IOCTL_MODE_OBJ_GETPROPERTIES,
    IOCTL_MODE_PAGE_FLIP, IOCTL_MODE_SETCRTC, MODE_ATOMIC_ALLOW_MODESET, MODE_ATOMIC_NONBLOCK,
    MODE_ATOMIC_TEST_ONLY, MODE_OBJECT_CONNECTOR, MODE_OBJECT_CRTC, MODE_OBJECT_PLANE,
    MODE_PAGE_FLIP_EVENT, ModeCrtc, ModeInfo, ObjectProperties, PageFlip, PlaneResources, ioctl,
};
use crate::{DrmError, DrmFormat, Mode, drm_format, next_buffer_index};

const ENUMERATION_ATTEMPTS: usize = 4;
const PLANE_TYPE_PRIMARY: u64 = 1;
const FLIP_USER_DATA: u64 = 0x4154_5249_4146_4c50;

#[derive(Clone, Copy, Debug)]
pub(crate) enum PresentationPath {
    Uninitialized,
    Legacy,
    Atomic(AtomicState),
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct AtomicState {
    plane_id: u32,
    plane_fb_id: u32,
    mode_blob_id: u32,
}

#[derive(Clone, Copy)]
struct Property {
    id: u32,
    value: u64,
    name: [c_char; 32],
}

#[derive(Clone, Copy)]
struct InitialAtomicProperties {
    connector_crtc_id: u32,
    crtc_mode_id: u32,
    crtc_active: u32,
    plane_fb_id: u32,
    plane_crtc_id: u32,
    plane_src_x: u32,
    plane_src_y: u32,
    plane_src_w: u32,
    plane_src_h: u32,
    plane_crtc_x: u32,
    plane_crtc_y: u32,
    plane_crtc_w: u32,
    plane_crtc_h: u32,
}

pub(crate) fn initialize_display(sink: &mut IoctlBackend) -> Result<(), DrmError> {
    if sink.device.capabilities().atomic_commit() {
        initialize_atomic(sink)
    } else {
        initialize_legacy(sink)
    }
}

pub(crate) fn destroy_mode_blob(file: &File, path: &PresentationPath) {
    if let PresentationPath::Atomic(state) = path {
        let mut destroy = DestroyBlob {
            blob_id: state.mode_blob_id,
        };
        let _ = ioctl(file, IOCTL_MODE_DESTROYPROPBLOB, &mut destroy);
    }
}

impl IoctlBackend {
    pub fn present(&mut self, frame: &Frame, _report: FrameReport) -> Result<(), DrmError> {
        if self.pending_index.is_some() {
            return Err(DrmError::FlipPending);
        }
        let target = next_buffer_index(self.scanning_index);
        let mode = self.device.mode();
        let layout = self.layout();
        copy_frame(&mut self.buffers[target], frame, mode, layout)?;
        match self.presentation {
            PresentationPath::Atomic(state) => atomic_flip(self, state, target)?,
            PresentationPath::Legacy => legacy_flip(self, target)?,
            PresentationPath::Uninitialized => return Err(DrmError::InvalidFlipEvent),
        }
        self.pending_index = Some(target);
        Ok(())
    }

    /// Reads and consumes one queued page-flip completion event.
    pub fn complete_flip(&mut self) -> Result<(), DrmError> {
        let Some(target) = self.pending_index else {
            return Err(DrmError::FlipEventMissing);
        };
        let mut bytes = [0_u8; 4096];
        let mut file = self.device.file();
        let count = file
            .read(&mut bytes)
            .map_err(|error| DrmError::ReadFlipEvent {
                errno: error.raw_os_error().unwrap_or(EIO),
            })?;
        if contains_flip_event(&bytes[..count])? {
            self.scanning_index = target;
            self.pending_index = None;
            Ok(())
        } else {
            Err(DrmError::FlipEventMissing)
        }
    }
}

fn initialize_legacy(sink: &mut IoctlBackend) -> Result<(), DrmError> {
    let mut connector_id = sink.device.connector().id;
    let mut modeset = ModeCrtc {
        set_connectors_ptr: pointer(&mut connector_id),
        count_connectors: 1,
        crtc_id: sink.device.crtc_id(),
        fb_id: sink.buffers[0].framebuffer_id,
        mode_valid: 1,
        mode: sink.device.raw_mode(),
        ..ModeCrtc::default()
    };
    ioctl(sink.device.file(), IOCTL_MODE_SETCRTC, &mut modeset).map_err(initial_modeset_error)?;
    sink.presentation = PresentationPath::Legacy;
    Ok(())
}

fn initialize_atomic(sink: &mut IoctlBackend) -> Result<(), DrmError> {
    let format = drm_format(sink.layout())?;
    let plane_id = find_primary_plane(sink, format)?;
    let properties = initial_atomic_properties(sink, plane_id)?;
    let raw_mode = sink.device.raw_mode();
    let mut blob = CreateBlob {
        data: pointer_const(&raw_mode),
        length: u32::try_from(size_of::<ModeInfo>()).map_err(|_| DrmError::ArithmeticOverflow)?,
        blob_id: 0,
    };
    ioctl(sink.device.file(), IOCTL_MODE_CREATEPROPBLOB, &mut blob)
        .map_err(|errno| DrmError::CreateModeBlob { errno })?;
    let result = commit_initial_atomic(sink, plane_id, properties, blob.blob_id);
    if let Err(error) = result {
        let mut destroy = DestroyBlob {
            blob_id: blob.blob_id,
        };
        let _ = ioctl(sink.device.file(), IOCTL_MODE_DESTROYPROPBLOB, &mut destroy);
        return Err(error);
    }
    sink.presentation = PresentationPath::Atomic(AtomicState {
        plane_id,
        plane_fb_id: properties.plane_fb_id,
        mode_blob_id: blob.blob_id,
    });
    Ok(())
}

fn commit_initial_atomic(
    sink: &IoctlBackend,
    plane_id: u32,
    properties: InitialAtomicProperties,
    mode_blob_id: u32,
) -> Result<(), DrmError> {
    let mut objects = [sink.device.connector().id, sink.device.crtc_id(), plane_id];
    let mut counts = [1_u32, 2, 10];
    let mut property_ids = [
        properties.connector_crtc_id,
        properties.crtc_mode_id,
        properties.crtc_active,
        properties.plane_fb_id,
        properties.plane_crtc_id,
        properties.plane_src_x,
        properties.plane_src_y,
        properties.plane_src_w,
        properties.plane_src_h,
        properties.plane_crtc_x,
        properties.plane_crtc_y,
        properties.plane_crtc_w,
        properties.plane_crtc_h,
    ];
    let mode = sink.device.mode();
    let source_width = u64::from(mode.width)
        .checked_shl(16)
        .ok_or(DrmError::ArithmeticOverflow)?;
    let source_height = u64::from(mode.height)
        .checked_shl(16)
        .ok_or(DrmError::ArithmeticOverflow)?;
    let mut values = [
        u64::from(sink.device.crtc_id()),
        u64::from(mode_blob_id),
        1,
        u64::from(sink.buffers[0].framebuffer_id),
        u64::from(sink.device.crtc_id()),
        0,
        0,
        source_width,
        source_height,
        0,
        0,
        u64::from(mode.width),
        u64::from(mode.height),
    ];
    let mut commit = AtomicCommit {
        count_objs: 3,
        objs_ptr: slice_pointer(&mut objects),
        count_props_ptr: slice_pointer(&mut counts),
        props_ptr: slice_pointer(&mut property_ids),
        prop_values_ptr: slice_pointer(&mut values),
        flags: MODE_ATOMIC_TEST_ONLY | MODE_ATOMIC_ALLOW_MODESET,
        ..AtomicCommit::default()
    };
    ioctl(sink.device.file(), IOCTL_MODE_ATOMIC, &mut commit).map_err(test_commit_error)?;
    commit.flags = MODE_ATOMIC_ALLOW_MODESET;
    ioctl(sink.device.file(), IOCTL_MODE_ATOMIC, &mut commit).map_err(atomic_commit_error)
}

fn atomic_flip(sink: &IoctlBackend, state: AtomicState, target: usize) -> Result<(), DrmError> {
    let mut objects = [state.plane_id];
    let mut counts = [1_u32];
    let mut properties = [state.plane_fb_id];
    let mut values = [u64::from(sink.buffers[target].framebuffer_id)];
    let mut commit = AtomicCommit {
        flags: MODE_ATOMIC_TEST_ONLY,
        count_objs: 1,
        objs_ptr: slice_pointer(&mut objects),
        count_props_ptr: slice_pointer(&mut counts),
        props_ptr: slice_pointer(&mut properties),
        prop_values_ptr: slice_pointer(&mut values),
        user_data: FLIP_USER_DATA,
        ..AtomicCommit::default()
    };
    ioctl(sink.device.file(), IOCTL_MODE_ATOMIC, &mut commit).map_err(test_commit_error)?;
    commit.flags = MODE_PAGE_FLIP_EVENT | MODE_ATOMIC_NONBLOCK;
    ioctl(sink.device.file(), IOCTL_MODE_ATOMIC, &mut commit).map_err(atomic_commit_error)
}

fn legacy_flip(sink: &IoctlBackend, target: usize) -> Result<(), DrmError> {
    let mut flip = PageFlip {
        crtc_id: sink.device.crtc_id(),
        fb_id: sink.buffers[target].framebuffer_id,
        flags: MODE_PAGE_FLIP_EVENT,
        user_data: FLIP_USER_DATA,
        ..PageFlip::default()
    };
    ioctl(sink.device.file(), IOCTL_MODE_PAGE_FLIP, &mut flip).map_err(page_flip_error)
}

fn find_primary_plane(sink: &IoctlBackend, format: DrmFormat) -> Result<u32, DrmError> {
    let crtc_index = sink
        .device
        .resources()
        .crtc_ids
        .iter()
        .position(|id| *id == sink.device.crtc_id())
        .ok_or(DrmError::NoPrimaryPlane)?;
    let crtc_mask = 1_u32
        .checked_shl(u32::try_from(crtc_index).map_err(|_| DrmError::ArithmeticOverflow)?)
        .ok_or(DrmError::NoPrimaryPlane)?;
    for plane_id in plane_ids(sink.device.file())? {
        let plane = query_plane(sink.device.file(), plane_id)?;
        if plane.possible_crtcs & crtc_mask == 0 || !plane.formats.contains(&format.fourcc()) {
            continue;
        }
        let properties = object_properties(sink.device.file(), plane_id, MODE_OBJECT_PLANE)?;
        if properties.iter().any(|property| {
            property_name(property, b"type") && property.value == PLANE_TYPE_PRIMARY
        }) {
            return Ok(plane_id);
        }
    }
    Err(DrmError::NoPrimaryPlane)
}

struct Plane {
    possible_crtcs: u32,
    formats: Vec<u32>,
}

fn plane_ids(file: &File) -> Result<Vec<u32>, DrmError> {
    for _ in 0..ENUMERATION_ATTEMPTS {
        let mut query = PlaneResources::default();
        ioctl(file, IOCTL_MODE_GETPLANERESOURCES, &mut query)
            .map_err(|errno| DrmError::EnumeratePlanes { errno })?;
        let mut ids = vec![0_u32; usize_count(query.count_planes)?];
        query.plane_id_ptr = slice_pointer(&mut ids);
        ioctl(file, IOCTL_MODE_GETPLANERESOURCES, &mut query)
            .map_err(|errno| DrmError::EnumeratePlanes { errno })?;
        if query.count_planes as usize <= ids.len() {
            ids.truncate(query.count_planes as usize);
            return Ok(ids);
        }
    }
    Err(DrmError::ResourcesChanged)
}

fn query_plane(file: &File, plane_id: u32) -> Result<Plane, DrmError> {
    for _ in 0..ENUMERATION_ATTEMPTS {
        let mut query = GetPlane {
            plane_id,
            ..GetPlane::default()
        };
        ioctl(file, IOCTL_MODE_GETPLANE, &mut query)
            .map_err(|errno| DrmError::QueryPlane { plane_id, errno })?;
        let mut formats = vec![0_u32; usize_count(query.count_format_types)?];
        query.format_type_ptr = slice_pointer(&mut formats);
        ioctl(file, IOCTL_MODE_GETPLANE, &mut query)
            .map_err(|errno| DrmError::QueryPlane { plane_id, errno })?;
        if query.count_format_types as usize <= formats.len() {
            formats.truncate(query.count_format_types as usize);
            return Ok(Plane {
                possible_crtcs: query.possible_crtcs,
                formats,
            });
        }
    }
    Err(DrmError::ResourcesChanged)
}

fn initial_atomic_properties(
    sink: &IoctlBackend,
    plane_id: u32,
) -> Result<InitialAtomicProperties, DrmError> {
    let connector = object_properties(
        sink.device.file(),
        sink.device.connector().id,
        MODE_OBJECT_CONNECTOR,
    )?;
    let crtc = object_properties(sink.device.file(), sink.device.crtc_id(), MODE_OBJECT_CRTC)?;
    let plane = object_properties(sink.device.file(), plane_id, MODE_OBJECT_PLANE)?;
    Ok(InitialAtomicProperties {
        connector_crtc_id: required_property(&connector, sink.device.connector().id, b"CRTC_ID")?,
        crtc_mode_id: required_property(&crtc, sink.device.crtc_id(), b"MODE_ID")?,
        crtc_active: required_property(&crtc, sink.device.crtc_id(), b"ACTIVE")?,
        plane_fb_id: required_property(&plane, plane_id, b"FB_ID")?,
        plane_crtc_id: required_property(&plane, plane_id, b"CRTC_ID")?,
        plane_src_x: required_property(&plane, plane_id, b"SRC_X")?,
        plane_src_y: required_property(&plane, plane_id, b"SRC_Y")?,
        plane_src_w: required_property(&plane, plane_id, b"SRC_W")?,
        plane_src_h: required_property(&plane, plane_id, b"SRC_H")?,
        plane_crtc_x: required_property(&plane, plane_id, b"CRTC_X")?,
        plane_crtc_y: required_property(&plane, plane_id, b"CRTC_Y")?,
        plane_crtc_w: required_property(&plane, plane_id, b"CRTC_W")?,
        plane_crtc_h: required_property(&plane, plane_id, b"CRTC_H")?,
    })
}

fn object_properties(
    file: &File,
    object_id: u32,
    object_type: u32,
) -> Result<Vec<Property>, DrmError> {
    for _ in 0..ENUMERATION_ATTEMPTS {
        let mut query = ObjectProperties {
            obj_id: object_id,
            obj_type: object_type,
            ..ObjectProperties::default()
        };
        ioctl(file, IOCTL_MODE_OBJ_GETPROPERTIES, &mut query)
            .map_err(|errno| DrmError::QueryObjectProperties { object_id, errno })?;
        let mut ids = vec![0_u32; usize_count(query.count_props)?];
        let mut values = vec![0_u64; usize_count(query.count_props)?];
        query.props_ptr = slice_pointer(&mut ids);
        query.prop_values_ptr = slice_pointer(&mut values);
        ioctl(file, IOCTL_MODE_OBJ_GETPROPERTIES, &mut query)
            .map_err(|errno| DrmError::QueryObjectProperties { object_id, errno })?;
        if query.count_props as usize > ids.len() {
            continue;
        }
        ids.truncate(query.count_props as usize);
        values.truncate(query.count_props as usize);
        return ids
            .into_iter()
            .zip(values)
            .map(|(id, value)| {
                let mut property = GetProperty {
                    prop_id: id,
                    ..GetProperty::default()
                };
                ioctl(file, IOCTL_MODE_GETPROPERTY, &mut property).map_err(|errno| {
                    DrmError::QueryProperty {
                        property_id: id,
                        errno,
                    }
                })?;
                Ok(Property {
                    id,
                    value,
                    name: property.name,
                })
            })
            .collect();
    }
    Err(DrmError::ResourcesChanged)
}

fn required_property(
    properties: &[Property],
    object_id: u32,
    name: &'static [u8],
) -> Result<u32, DrmError> {
    properties
        .iter()
        .find(|property| property_name(property, name))
        .map(|property| property.id)
        .ok_or(DrmError::MissingAtomicProperty {
            object_id,
            property_name: property_label(name),
        })
}

fn property_name(property: &Property, expected: &[u8]) -> bool {
    property
        .name
        .iter()
        .map(|byte| *byte as u8)
        .take(expected.len())
        .eq(expected.iter().copied())
        && property
            .name
            .get(expected.len())
            .is_some_and(|byte| *byte == 0)
}

const fn property_label(name: &'static [u8]) -> &'static str {
    match name {
        b"CRTC_ID" => "CRTC_ID",
        b"MODE_ID" => "MODE_ID",
        b"ACTIVE" => "ACTIVE",
        b"FB_ID" => "FB_ID",
        b"SRC_X" => "SRC_X",
        b"SRC_Y" => "SRC_Y",
        b"SRC_W" => "SRC_W",
        b"SRC_H" => "SRC_H",
        b"CRTC_X" => "CRTC_X",
        b"CRTC_Y" => "CRTC_Y",
        b"CRTC_W" => "CRTC_W",
        b"CRTC_H" => "CRTC_H",
        _ => "unknown",
    }
}

fn copy_frame(
    buffer: &mut ScanoutBuffer,
    frame: &Frame,
    mode: Mode,
    layout: PixelLayout,
) -> Result<(), DrmError> {
    let size = frame.size();
    if size.width != u32::from(mode.width) || size.height != u32::from(mode.height) {
        return Err(DrmError::FrameSizeMismatch {
            frame_width: size.width,
            frame_height: size.height,
            mode_width: mode.width,
            mode_height: mode.height,
        });
    }
    if frame.layout() != layout {
        return Err(DrmError::FrameLayoutMismatch {
            expected: layout.bytes_per_pixel(),
            actual: frame.layout().bytes_per_pixel(),
        });
    }
    let row_bytes = usize::try_from(
        u64::from(size.width)
            .checked_mul(u64::from(layout.bytes_per_pixel()))
            .ok_or(DrmError::ArithmeticOverflow)?,
    )
    .map_err(|_| DrmError::ArithmeticOverflow)?;
    let source_stride =
        usize::try_from(frame.stride()).map_err(|_| DrmError::ArithmeticOverflow)?;
    let destination_stride =
        usize::try_from(buffer.pitch).map_err(|_| DrmError::ArithmeticOverflow)?;
    // SAFETY: the mapping covers `buffer.size` writable bytes for the lifetime of the sink.
    // Row offsets and lengths are checked below before any mapped byte is accessed.
    let destination =
        unsafe { from_raw_parts_mut(buffer.mapping.as_ptr().cast::<u8>(), buffer.size) };
    for row in 0..usize::try_from(size.height).map_err(|_| DrmError::ArithmeticOverflow)? {
        let source_start = row
            .checked_mul(source_stride)
            .ok_or(DrmError::ArithmeticOverflow)?;
        let source_end = source_start
            .checked_add(row_bytes)
            .ok_or(DrmError::ArithmeticOverflow)?;
        let destination_start = row
            .checked_mul(destination_stride)
            .ok_or(DrmError::ArithmeticOverflow)?;
        let destination_end = destination_start
            .checked_add(row_bytes)
            .ok_or(DrmError::ArithmeticOverflow)?;
        let source = frame
            .bytes()
            .get(source_start..source_end)
            .ok_or(DrmError::FrameBufferTooSmall)?;
        let target = destination
            .get_mut(destination_start..destination_end)
            .ok_or(DrmError::FrameBufferTooSmall)?;
        target.copy_from_slice(source);
    }
    Ok(())
}

fn contains_flip_event(bytes: &[u8]) -> Result<bool, DrmError> {
    let mut offset = 0_usize;
    while offset < bytes.len() {
        let header_end = offset.checked_add(8).ok_or(DrmError::InvalidFlipEvent)?;
        let header = bytes
            .get(offset..header_end)
            .ok_or(DrmError::InvalidFlipEvent)?;
        let event_type = u32::from_ne_bytes(
            header[0..4]
                .try_into()
                .map_err(|_| DrmError::InvalidFlipEvent)?,
        );
        let length = usize::try_from(u32::from_ne_bytes(
            header[4..8]
                .try_into()
                .map_err(|_| DrmError::InvalidFlipEvent)?,
        ))
        .map_err(|_| DrmError::InvalidFlipEvent)?;
        if length < 8 {
            return Err(DrmError::InvalidFlipEvent);
        }
        let end = offset
            .checked_add(length)
            .ok_or(DrmError::InvalidFlipEvent)?;
        let event = bytes.get(offset..end).ok_or(DrmError::InvalidFlipEvent)?;
        if event_type == EVENT_FLIP_COMPLETE {
            let user_data = event.get(8..16).ok_or(DrmError::InvalidFlipEvent)?;
            let user_data = u64::from_ne_bytes(
                user_data
                    .try_into()
                    .map_err(|_| DrmError::InvalidFlipEvent)?,
            );
            if user_data == FLIP_USER_DATA {
                return Ok(true);
            }
        }
        offset = end;
    }
    Ok(false)
}

fn initial_modeset_error(errno: i32) -> DrmError {
    match errno {
        EACCES => DrmError::DisplayAccessLost { errno },
        EBUSY => DrmError::DisplayBusy { errno },
        _ => DrmError::InitialModeset { errno },
    }
}

fn atomic_commit_error(errno: i32) -> DrmError {
    match errno {
        EACCES => DrmError::DisplayAccessLost { errno },
        EBUSY => DrmError::DisplayBusy { errno },
        _ => DrmError::AtomicCommit { errno },
    }
}

fn page_flip_error(errno: i32) -> DrmError {
    match errno {
        EACCES => DrmError::DisplayAccessLost { errno },
        EBUSY => DrmError::DisplayBusy { errno },
        _ => DrmError::PageFlip { errno },
    }
}

fn test_commit_error(errno: i32) -> DrmError {
    match errno {
        EACCES => DrmError::DisplayAccessLost { errno },
        EBUSY => DrmError::DisplayBusy { errno },
        _ => DrmError::AtomicTestFailed { errno },
    }
}

fn usize_count(value: u32) -> Result<usize, DrmError> {
    usize::try_from(value).map_err(|_| DrmError::ArithmeticOverflow)
}

fn pointer<T>(value: &mut T) -> u64 {
    value as *mut T as usize as u64
}

fn pointer_const<T>(value: &T) -> u64 {
    value as *const T as usize as u64
}

fn slice_pointer<T>(values: &mut [T]) -> u64 {
    values.as_mut_ptr() as usize as u64
}

#[cfg(test)]
mod tests {
    use super::{EVENT_FLIP_COMPLETE, FLIP_USER_DATA, contains_flip_event};
    use crate::DrmError;

    #[test]
    fn parses_the_matching_flip_event() {
        let mut event = Vec::new();
        event.extend_from_slice(&EVENT_FLIP_COMPLETE.to_ne_bytes());
        event.extend_from_slice(&32_u32.to_ne_bytes());
        event.extend_from_slice(&FLIP_USER_DATA.to_ne_bytes());
        event.resize(32, 0);
        assert!(contains_flip_event(&event).unwrap());
    }

    #[test]
    fn rejects_a_truncated_event() {
        let mut event = Vec::new();
        event.extend_from_slice(&EVENT_FLIP_COMPLETE.to_ne_bytes());
        event.extend_from_slice(&32_u32.to_ne_bytes());
        assert!(matches!(
            contains_flip_event(&event),
            Err(DrmError::InvalidFlipEvent)
        ));
    }
}
