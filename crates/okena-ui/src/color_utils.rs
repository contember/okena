//! Generic color utility functions.

/// Blend `tint` into `base` RGB color at the given ratio (0.0 = pure base, 1.0 = pure tint).
pub fn tint_color(base: u32, tint: u32, amount: f32) -> u32 {
    let lerp = |b: u32, t: u32| (b as f32 + (t as f32 - b as f32) * amount) as u32;
    let r = lerp((base >> 16) & 0xFF, (tint >> 16) & 0xFF);
    let g = lerp((base >> 8) & 0xFF, (tint >> 8) & 0xFF);
    let b = lerp(base & 0xFF, tint & 0xFF);
    (r << 16) | (g << 8) | b
}

/// A surface one small step off the page background `bg`, nudged toward the
/// foreground `fg`. Reads as a distinct panel without becoming a heavy block —
/// and stays on the right side of the page in both dark and light themes.
pub fn raised_surface(bg: u32, fg: u32) -> u32 {
    tint_color(bg, fg, 0.05)
}

/// The hairline that goes with [`raised_surface`]: the theme border pulled back
/// toward the page so it outlines the surface without framing it.
pub fn raised_surface_border(bg: u32, border: u32) -> u32 {
    tint_color(bg, border, 0.7)
}
