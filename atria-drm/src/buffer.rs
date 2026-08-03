use std::ffi::c_void;
use std::fs::File;
use std::os::fd::AsRawFd;
use std::ptr::NonNull;
use std::ptr::null_mut;

use atria_protocol::capability::CapabilitySet;
use atria_software_output::PixelLayout;
use libc::{EFAULT, MAP_FAILED, MAP_SHARED, PROT_READ, PROT_WRITE, mmap, munmap, off_t};

use crate::present::{PresentationPath, destroy_mode_blob, initialize_display};
use crate::uapi::{
    CreateDumb, DestroyDumb, Framebuffer, IOCTL_MODE_ADDFB2, IOCTL_MODE_CREATE_DUMB,
    IOCTL_MODE_DESTROY_DUMB, IOCTL_MODE_MAP_DUMB, IOCTL_MODE_RMFB, MapDumb, errno, ioctl,
};
use crate::{DeviceConfig, DrmDevice, DrmError, DrmFormat, Mode, buffer_geometry, drm_format};

#[derive(Debug)]
pub(crate) struct ScanoutBuffer {
    pub(crate) handle: u32,
    pub(crate) framebuffer_id: u32,
    pub(crate) pitch: u32,
    pub(crate) size: usize,
    pub(crate) mapping: NonNull<c_void>,
}

#[derive(Debug)]
pub struct DrmSink {
    pub(crate) device: DrmDevice,
    pub(crate) buffers: [ScanoutBuffer; 2],
    pub(crate) scanning_index: usize,
    pub(crate) pending_index: Option<usize>,
    pub(crate) presentation: PresentationPath,
    layout: PixelLayout,
}

impl DrmSink {
    pub fn open(config: DeviceConfig, layout: PixelLayout) -> Result<Self, DrmError> {
        let device = DrmDevice::open(config)?;
        if !device.capabilities().dumb_buffers() {
            return Err(DrmError::DumbBuffersUnavailable);
        }
        let first = allocate_buffer(&device, layout, 0)?;
        let second = match allocate_buffer(&device, layout, 1) {
            Ok(buffer) => buffer,
            Err(error) => {
                teardown_buffer(device.file(), &first);
                return Err(error);
            }
        };
        let mut sink = Self {
            device,
            buffers: [first, second],
            scanning_index: 0,
            pending_index: None,
            presentation: PresentationPath::Uninitialized,
            layout,
        };
        initialize_display(&mut sink)?;
        Ok(sink)
    }

    #[must_use]
    pub const fn capabilities(&self) -> CapabilitySet {
        self.device.capabilities().capability_set()
    }

    #[must_use]
    pub const fn layout(&self) -> PixelLayout {
        self.layout
    }

    #[must_use]
    pub fn mode(&self) -> Mode {
        self.device.mode()
    }
}

impl Drop for DrmSink {
    fn drop(&mut self) {
        destroy_mode_blob(self.device.file(), &self.presentation);
        // Framebuffer IDs are detached before their backing handles disappear.
        for buffer in &self.buffers {
            let mut framebuffer_id = buffer.framebuffer_id;
            let _ = ioctl(self.device.file(), IOCTL_MODE_RMFB, &mut framebuffer_id);
        }
        for buffer in &self.buffers {
            let mut destroy = DestroyDumb {
                handle: buffer.handle,
            };
            let _ = ioctl(self.device.file(), IOCTL_MODE_DESTROY_DUMB, &mut destroy);
        }
        for buffer in &self.buffers {
            unmap(buffer.mapping, buffer.size);
        }
    }
}

fn allocate_buffer(
    device: &DrmDevice,
    layout: PixelLayout,
    buffer_index: usize,
) -> Result<ScanoutBuffer, DrmError> {
    let mode = device.mode();
    let format = drm_format(layout)?;
    let width = u32::from(mode.width);
    let height = u32::from(mode.height);
    let bpp = u32::from(layout.bytes_per_pixel())
        .checked_mul(8)
        .ok_or(DrmError::ArithmeticOverflow)?;
    let mut create = CreateDumb {
        height,
        width,
        bpp,
        ..CreateDumb::default()
    };
    ioctl(device.file(), IOCTL_MODE_CREATE_DUMB, &mut create).map_err(|errno| {
        DrmError::CreateDumbBuffer {
            buffer_index,
            errno,
        }
    })?;

    let allocated = allocate_after_create(device.file(), layout, format, buffer_index, create);
    if allocated.is_err() {
        let mut destroy = DestroyDumb {
            handle: create.handle,
        };
        let _ = ioctl(device.file(), IOCTL_MODE_DESTROY_DUMB, &mut destroy);
    }
    allocated
}

fn allocate_after_create(
    file: &File,
    layout: PixelLayout,
    format: DrmFormat,
    buffer_index: usize,
    create: CreateDumb,
) -> Result<ScanoutBuffer, DrmError> {
    let geometry = buffer_geometry(
        create.width,
        create.height,
        layout.bytes_per_pixel(),
        create.pitch,
        create.size,
    )?;
    let size = usize::try_from(geometry.size).map_err(|_| DrmError::ArithmeticOverflow)?;
    let mut map = MapDumb {
        handle: create.handle,
        ..MapDumb::default()
    };
    ioctl(file, IOCTL_MODE_MAP_DUMB, &mut map).map_err(|errno| DrmError::MapDumbBuffer {
        buffer_index,
        errno,
    })?;
    let offset = off_t::try_from(map.offset).map_err(|_| DrmError::MappingOffsetOutOfRange {
        buffer_index,
        offset: map.offset,
    })?;
    // SAFETY: the DRM driver returned `offset` for this live dumb-buffer handle, `size`
    // was checked against the kernel-reported allocation, and the file remains open while
    // the mapping is owned by `DrmSink`.
    let mapped = unsafe {
        mmap(
            null_mut(),
            size,
            PROT_READ | PROT_WRITE,
            MAP_SHARED,
            file.as_raw_fd(),
            offset,
        )
    };
    if mapped == MAP_FAILED {
        return Err(DrmError::MapMemory {
            buffer_index,
            errno: errno(),
        });
    }
    let Some(mapping) = NonNull::new(mapped) else {
        // A null address is not representable by the owned mapping type, even though the
        // kernel may technically map page zero on permissive systems.
        // SAFETY: `mapped` and `size` are exactly the successful mmap result above and have
        // not been exposed or unmapped.
        let _ = unsafe { munmap(mapped, size) };
        return Err(DrmError::MapMemory {
            buffer_index,
            errno: EFAULT,
        });
    };
    let mut framebuffer = Framebuffer {
        width: create.width,
        height: create.height,
        pixel_format: format.fourcc(),
        ..Framebuffer::default()
    };
    framebuffer.handles[0] = create.handle;
    framebuffer.pitches[0] = create.pitch;
    if let Err(errno) = ioctl(file, IOCTL_MODE_ADDFB2, &mut framebuffer) {
        unmap(mapping, size);
        return Err(DrmError::AddFramebuffer {
            buffer_index,
            errno,
        });
    }
    Ok(ScanoutBuffer {
        handle: create.handle,
        framebuffer_id: framebuffer.fb_id,
        pitch: create.pitch,
        size,
        mapping,
    })
}

fn teardown_buffer(file: &File, buffer: &ScanoutBuffer) {
    let mut framebuffer_id = buffer.framebuffer_id;
    let _ = ioctl(file, IOCTL_MODE_RMFB, &mut framebuffer_id);
    let mut destroy = DestroyDumb {
        handle: buffer.handle,
    };
    let _ = ioctl(file, IOCTL_MODE_DESTROY_DUMB, &mut destroy);
    unmap(buffer.mapping, buffer.size);
}

fn unmap(mapping: NonNull<c_void>, size: usize) {
    // SAFETY: `mapping` and `size` are exactly the successful `mmap` result owned by one
    // scanout buffer, and this function is called once after its last access.
    let _ = unsafe { munmap(mapping.as_ptr(), size) };
}
