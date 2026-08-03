use std::path::Path;

use atria_drm::{DeviceConfig, DrmDevice, DrmError};

#[test]
fn absent_default_device_is_a_clear_runtime_error() {
    if Path::new("/dev/dri/card0").exists() {
        return;
    }
    assert!(matches!(
        DrmDevice::open(DeviceConfig::default()),
        Err(DrmError::DeviceAbsent { card_index: 0 })
    ));
}
