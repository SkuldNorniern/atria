//! That a viewer sees the colour that was composed, in whatever layout it asked for.
//!
//! RFB gives the viewer the choice of pixel layout and gives the server no way to refuse one. A
//! server that ignored the choice and sent its own would put a picture on screen that is wrong in
//! a way nothing reports — which is exactly how a blue window arrives red.

use atria_vnc::PixelFormat;

/// One pixel of `0x2f9ed8`, laid out as the compositor's frames are: red, green, blue, unused.
fn composed() -> Vec<u8> {
    vec![0x2f, 0x9e, 0xd8, 0xff]
}

#[test]
fn the_declared_format_is_passed_through_untouched() {
    let frame = composed();
    let converted = PixelFormat::declared().convert(&frame);
    assert_eq!(
        converted.as_ref(),
        frame.as_slice(),
        "a viewer that kept the server's layout costs no copy"
    );
}

#[test]
fn a_viewer_asking_for_the_opposite_channel_order_gets_the_same_colour() {
    let bgra = PixelFormat {
        red_shift: 16,
        blue_shift: 0,
        ..PixelFormat::declared()
    };
    let frame = composed();
    let converted = bgra.convert(&frame);
    let packed = u32::from_le_bytes([converted[0], converted[1], converted[2], converted[3]]);
    assert_eq!((packed >> 16) & 0xff, 0x2f, "red survives the move");
    assert_eq!((packed >> 8) & 0xff, 0x9e, "so does green");
    assert_eq!(packed & 0xff, 0xd8, "and blue");
}

#[test]
fn a_big_endian_viewer_gets_its_bytes_the_way_round_it_asked() {
    let format = PixelFormat {
        big_endian: true,
        red_shift: 16,
        blue_shift: 0,
        ..PixelFormat::declared()
    };
    let frame = composed();
    let converted = format.convert(&frame);
    let packed = u32::from_be_bytes([converted[0], converted[1], converted[2], converted[3]]);
    assert_eq!((packed >> 16) & 0xff, 0x2f);
    assert_eq!(packed & 0xff, 0xd8);
}

#[test]
fn a_sixteen_bit_viewer_gets_the_nearest_colour_its_depth_can_hold() {
    let rgb565 = PixelFormat {
        bits_per_pixel: 16,
        red_max: 31,
        green_max: 63,
        blue_max: 31,
        red_shift: 11,
        green_shift: 5,
        blue_shift: 0,
        ..PixelFormat::declared()
    };
    assert_eq!(rgb565.stride(), 2, "two bytes a pixel, not four");
    let frame = composed();
    let converted = rgb565.convert(&frame);
    assert_eq!(converted.len(), 2);
    let packed = u16::from_le_bytes([converted[0], converted[1]]);
    // Quantised, not wrong: five bits of red cannot hold 0x2f exactly.
    assert_eq!(u32::from((packed >> 11) & 31) * 255 / 31, 41);
    assert_eq!(u32::from((packed >> 5) & 63) * 255 / 63, 157);
    assert_eq!(u32::from(packed & 31) * 255 / 31, 213);
}

#[test]
fn a_format_this_server_cannot_produce_is_refused_rather_than_approximated() {
    // A colour-mapped viewer. Serving it our bytes would be serving nonsense.
    let mut mapped = [0_u8; 16];
    mapped[0] = 8;
    mapped[1] = 8;
    assert!(
        PixelFormat::decode(&mapped).is_err(),
        "a colour map is not something this server has"
    );

    let mut odd = [0_u8; 16];
    odd[0] = 24;
    odd[3] = 1;
    assert!(
        PixelFormat::decode(&odd).is_err(),
        "and neither is a three-byte pixel"
    );
}
