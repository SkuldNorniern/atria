use std::io::{Error as IoError, ErrorKind};

use atria_protocol::capability::CapabilitySet;
use atria_software_output::{Frame, FrameReport, FrameSink, PixelLayout, Presented, SinkError};

use crate::backend::{IoctlBackend, ScanoutBackend};
use crate::{DeviceConfig, DrmError, Mode};

/// A scanout sink backed by the platform implementation selected by this package.
#[derive(Debug)]
pub struct DrmSink {
    backend: IoctlBackend,
    capabilities: CapabilitySet,
    layout: PixelLayout,
}

impl DrmSink {
    pub fn open(config: DeviceConfig, layout: PixelLayout) -> Result<Self, DrmError> {
        let backend = <IoctlBackend as ScanoutBackend>::open(config, layout)?;
        let capabilities = ScanoutBackend::capabilities(&backend);
        Ok(Self {
            backend,
            capabilities,
            layout,
        })
    }

    #[must_use]
    pub const fn capabilities(&self) -> CapabilitySet {
        self.capabilities
    }

    #[must_use]
    pub const fn layout(&self) -> PixelLayout {
        self.layout
    }

    #[must_use]
    pub fn mode(&self) -> Mode {
        ScanoutBackend::mode(&self.backend)
    }

    pub fn present(&mut self, frame: &Frame, report: FrameReport) -> Result<(), DrmError> {
        ScanoutBackend::present(&mut self.backend, frame, report)
    }

    /// Reads and consumes one queued page-flip completion event.
    pub fn complete_flip(&mut self) -> Result<(), DrmError> {
        ScanoutBackend::complete_flip(&mut self.backend)
    }
}

impl FrameSink for DrmSink {
    fn present(&mut self, presented: Presented<'_>) -> Result<(), SinkError> {
        DrmSink::present(self, &presented.frame, presented.report).map_err(drm_sink_error)
    }
}

fn drm_sink_error(error: DrmError) -> SinkError {
    let io_error = match error {
        DrmError::FlipPending => IoError::from(ErrorKind::WouldBlock),
        DrmError::DisplayAccessLost { errno }
        | DrmError::DisplayBusy { errno }
        | DrmError::AtomicCommit { errno }
        | DrmError::PageFlip { errno }
        | DrmError::ReadFlipEvent { errno } => IoError::from_raw_os_error(errno),
        _ => IoError::from(ErrorKind::InvalidInput),
    };
    SinkError::from(io_error)
}
