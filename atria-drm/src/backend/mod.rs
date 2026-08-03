mod ioctl;

use atria_protocol::capability::CapabilitySet;
use atria_software_output::{Frame, FrameReport, PixelLayout};

use crate::Mode;

pub(crate) use ioctl::IoctlBackend;
pub use ioctl::{Connector, DeviceConfig, DrmDevice, Encoder, ResourceSnapshot};

/// Operations whose resource ownership and system interface depend on the scanout platform.
///
/// Opening establishes the complete backend: it opens the device, enumerates and selects a
/// mode, allocates and maps its scanout buffers, and performs the initial presentation setup.
/// The owned backend keeps those resources valid through presentation and flip completion.
///
/// A trait with an associated error is used instead of an enum because one concrete backend is
/// selected for a build. Static dispatch keeps its resource ownership visible without `dyn` or
/// heap indirection, while another implementation can satisfy the same operations independently.
pub(crate) trait ScanoutBackend: Sized {
    type Error;

    fn open(config: DeviceConfig, layout: PixelLayout) -> Result<Self, Self::Error>;
    fn capabilities(&self) -> CapabilitySet;
    fn mode(&self) -> Mode;
    fn present(&mut self, frame: &Frame, report: FrameReport) -> Result<(), Self::Error>;
    fn complete_flip(&mut self) -> Result<(), Self::Error>;
}
