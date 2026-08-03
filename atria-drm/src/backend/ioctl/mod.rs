mod buffer;
mod device;
mod present;
mod uapi;

use atria_protocol::capability::CapabilitySet;
use atria_software_output::{Frame, FrameReport, PixelLayout};

use super::ScanoutBackend;
use crate::{DrmError, Mode};

pub(crate) use buffer::IoctlBackend;
pub use device::{Connector, DeviceConfig, DrmDevice, Encoder, ResourceSnapshot};

impl ScanoutBackend for IoctlBackend {
    type Error = DrmError;

    fn open(config: DeviceConfig, layout: PixelLayout) -> Result<Self, Self::Error> {
        Self::open(config, layout)
    }

    fn capabilities(&self) -> CapabilitySet {
        self.capabilities()
    }

    fn mode(&self) -> Mode {
        self.mode()
    }

    fn present(&mut self, frame: &Frame, report: FrameReport) -> Result<(), Self::Error> {
        self.present(frame, report)
    }

    fn complete_flip(&mut self) -> Result<(), Self::Error> {
        self.complete_flip()
    }
}
