use atria_input::{InputDevice, InputError};

#[test]
fn opening_an_absent_device_is_a_clear_error() {
    assert!(matches!(
        InputDevice::open(u32::MAX),
        Err(InputError::DeviceAbsent {
            event_index: u32::MAX
        })
    ));
}
