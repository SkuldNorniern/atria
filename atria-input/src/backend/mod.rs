pub(crate) mod evdev;

use std::time::Duration;

use crate::readiness::DescriptorReadiness;
use crate::{DeviceClass, KeyCode, RawInputEvent};

pub(crate) use evdev::EvdevBackend;

/// Platform-owned operations needed to discover devices and produce raw input reports.
///
/// Capability results and initial physical state remain owned by the opened backend. Report
/// normalization and epochs are deliberately absent: those invariants are shared across every
/// source of input and belong above this boundary.
///
/// A trait with associated storage types is used instead of an enum because a build has one
/// concrete input transport. Static dispatch retains the transport's descriptor and readiness
/// representation without `dyn`, `Box`, or a cross-backend storage union.
pub(crate) trait InputBackend: Sized {
    type Error;
    type KeyState: AsRef<[u8]>;
    type Reports: AsRef<[RawInputEvent]>;
    type Readiness;

    fn open(event_index: u32) -> Result<Option<Self>, Self::Error>;
    fn enumerate() -> Result<Vec<Self>, Self::Error>;
    fn event_index(&self) -> u32;
    fn class(&self) -> DeviceClass;
    fn reports_key(&self, key: KeyCode) -> bool;
    fn initial_key_state(&self) -> &Self::KeyState;
    fn read_reports(&self) -> Result<Self::Reports, Self::Error>;
    fn query_key_state(&self) -> Result<Self::KeyState, Self::Error>;
    fn wait_readiness<'a>(
        devices: impl ExactSizeIterator<Item = &'a Self>,
        timeout: Option<Duration>,
    ) -> Result<Self::Readiness, Self::Error>
    where
        Self: 'a;
    fn has_ready(readiness: &Self::Readiness) -> bool;
    fn readiness_at(readiness: &Self::Readiness, index: usize) -> DescriptorReadiness;
}
