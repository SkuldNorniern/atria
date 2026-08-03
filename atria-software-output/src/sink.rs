use std::fs::{File, OpenOptions};
use std::io::{Error, Seek, SeekFrom, Write};
use std::path::Path;

use crate::{Frame, FrameReport, SinkError};

pub trait FrameSink {
    fn present(&mut self, frame: &Frame, report: FrameReport) -> Result<(), SinkError>;
}

#[derive(Debug, Default)]
pub struct HeadlessSink {
    frames_presented: u64,
    last_report: Option<FrameReport>,
}

impl HeadlessSink {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            frames_presented: 0,
            last_report: None,
        }
    }

    #[must_use]
    pub const fn frames_presented(&self) -> u64 {
        self.frames_presented
    }

    #[must_use]
    pub const fn last_report(&self) -> Option<FrameReport> {
        self.last_report
    }
}

impl FrameSink for HeadlessSink {
    fn present(&mut self, _frame: &Frame, report: FrameReport) -> Result<(), SinkError> {
        self.frames_presented = self.frames_presented.saturating_add(1);
        self.last_report = Some(report);
        Ok(())
    }
}

#[derive(Debug)]
pub struct FileSink {
    file: File,
}

impl FileSink {
    pub fn create(path: impl AsRef<Path>) -> Result<Self, SinkError> {
        let file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .read(true)
            .write(true)
            .open(path)?;
        Ok(Self { file })
    }
}

impl FrameSink for FileSink {
    fn present(&mut self, frame: &Frame, _report: FrameReport) -> Result<(), SinkError> {
        self.file.seek(SeekFrom::Start(0))?;
        self.file.write_all(frame.bytes())?;
        self.file.set_len(
            u64::try_from(frame.bytes().len()).map_err(|_| Error::other("frame too large"))?,
        )?;
        self.file.flush()?;
        Ok(())
    }
}
