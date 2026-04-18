use super::super::event_listener::{xterm_256_cube_rgb, xterm_256_grayscale_rgb};

#[test]
fn test_xterm_cube_corners() {
    // First cube entry (16) is true black, last (231) is true white.
    assert_eq!(xterm_256_cube_rgb(16), (0, 0, 0));
    assert_eq!(xterm_256_cube_rgb(231), (255, 255, 255));
}

#[test]
fn test_xterm_cube_axis_order() {
    // Pure blue axis: changing only the blue component.
    assert_eq!(xterm_256_cube_rgb(17), (0, 0, 95));
    assert_eq!(xterm_256_cube_rgb(18), (0, 0, 135));
    assert_eq!(xterm_256_cube_rgb(21), (0, 0, 255));

    // Pure green axis: next cube row.
    assert_eq!(xterm_256_cube_rgb(22), (0, 95, 0));
    assert_eq!(xterm_256_cube_rgb(28), (0, 135, 0));

    // Pure red axis: next cube plane.
    assert_eq!(xterm_256_cube_rgb(52), (95, 0, 0));
    assert_eq!(xterm_256_cube_rgb(88), (135, 0, 0));
}

#[test]
fn test_xterm_cube_mixed() {
    // xterm's canonical value for 208 is the familiar "orange" (#ff8700).
    assert_eq!(xterm_256_cube_rgb(208), (255, 135, 0));
    // 196 = pure bright red (#ff0000).
    assert_eq!(xterm_256_cube_rgb(196), (255, 0, 0));
    // 226 = pure bright yellow (#ffff00).
    assert_eq!(xterm_256_cube_rgb(226), (255, 255, 0));
}

#[test]
fn test_xterm_grayscale_endpoints() {
    // First grayscale step is 8 (just above black); last is 238.
    assert_eq!(xterm_256_grayscale_rgb(232), (8, 8, 8));
    assert_eq!(xterm_256_grayscale_rgb(255), (238, 238, 238));
}

#[test]
fn test_xterm_grayscale_linear() {
    // Each step adds 10 to every channel.
    assert_eq!(xterm_256_grayscale_rgb(233), (18, 18, 18));
    assert_eq!(xterm_256_grayscale_rgb(244), (128, 128, 128));
}
