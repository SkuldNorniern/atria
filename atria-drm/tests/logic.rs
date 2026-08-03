use atria_drm::{
    DrmCapabilities, DrmError, DrmFormat, Mode, ModeSelection, buffer_geometry, drm_format,
    next_buffer_index, select_mode,
};
use atria_protocol::capability::Capability;
use atria_software_output::PixelLayout;

fn mode(width: u16, height: u16, refresh_hz: u32, preferred: bool) -> Mode {
    Mode {
        width,
        height,
        refresh_hz,
        mode_type: if preferred { 1 << 3 } else { 0 },
    }
}

#[test]
fn selects_the_preferred_mode() {
    let modes = [mode(1280, 720, 60, false), mode(1920, 1080, 60, true)];
    assert_eq!(select_mode(&modes, ModeSelection::Preferred).unwrap(), 1);
}

#[test]
fn selects_an_exact_override() {
    let modes = [mode(1920, 1080, 60, true), mode(1280, 720, 50, false)];
    let requested = ModeSelection::Exact {
        width: 1280,
        height: 720,
        refresh_hz: 50,
    };
    assert_eq!(select_mode(&modes, requested).unwrap(), 1);
}

#[test]
fn rejects_a_connector_without_a_preferred_mode() {
    let modes = [mode(1280, 720, 60, false)];
    assert!(matches!(
        select_mode(&modes, ModeSelection::Preferred),
        Err(DrmError::NoPreferredMode)
    ));
}

#[test]
fn validates_pitch_and_mapping_size() {
    assert_eq!(
        buffer_geometry(1920, 1080, 4, 7680, 8_294_400)
            .unwrap()
            .size,
        8_294_400
    );
    assert!(matches!(
        buffer_geometry(u32::MAX, 1, u8::MAX, u32::MAX, u64::MAX),
        Err(DrmError::PitchTooSmall { .. })
    ));
    assert!(matches!(
        buffer_geometry(4, u32::MAX, 4, u32::MAX, u64::MAX),
        Err(DrmError::ArithmeticOverflow)
    ));
    assert!(matches!(
        buffer_geometry(4, 4, 4, 16, 63),
        Err(DrmError::BufferTooSmall { .. })
    ));
}

#[test]
fn rotates_between_exactly_two_buffers() {
    assert_eq!(next_buffer_index(0), 1);
    assert_eq!(next_buffer_index(1), 0);
}

#[test]
fn maps_pixel_layouts_to_drm_fourcc_codes() {
    let expected = [
        (1, DrmFormat::C8, u32::from_le_bytes(*b"C8  ")),
        (2, DrmFormat::Rgb565, u32::from_le_bytes(*b"RG16")),
        (3, DrmFormat::Rgb888, u32::from_le_bytes(*b"RG24")),
        (4, DrmFormat::Xrgb8888, u32::from_le_bytes(*b"XR24")),
    ];
    for (bytes, format, fourcc) in expected {
        let layout = PixelLayout::new(bytes).unwrap();
        assert_eq!(drm_format(layout).unwrap(), format);
        assert_eq!(format.fourcc(), fourcc);
    }
    assert!(matches!(
        drm_format(PixelLayout::new(5).unwrap()),
        Err(DrmError::UnsupportedPixelLayout(5))
    ));
}

#[test]
fn capabilities_report_atomic_or_the_real_page_flip_fallback() {
    let atomic = DrmCapabilities::new(true, true).capability_set();
    assert!(atomic.contains(Capability::DrmDumbBuffer));
    assert!(atomic.contains(Capability::DrmAtomicCommit));
    assert!(!atomic.contains(Capability::DrmPageFlip));

    let legacy = DrmCapabilities::new(true, false).capability_set();
    assert!(legacy.contains(Capability::DrmDumbBuffer));
    assert!(!legacy.contains(Capability::DrmAtomicCommit));
    assert!(legacy.contains(Capability::DrmPageFlip));
}
