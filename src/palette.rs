//! Colour themes for icons and indicators. Code asks for a role colour from
//! the Full palette (pink, orange, …); the chosen theme maps it to its own
//! shade, so every theme keeps the same variety of colours.

use crate::config;

pub const PINK: u32 = 0xff375f;
pub const RED: u32 = 0xff453a;
pub const ORANGE: u32 = 0xff9f0a;
pub const YELLOW: u32 = 0xffd60a;
pub const GREEN: u32 = 0x30d158;
pub const MINT: u32 = 0x66d4cf;
pub const TEAL: u32 = 0x40c8e0;
pub const CYAN: u32 = 0x64d2ff;
pub const BLUE: u32 = 0x0a84ff;
pub const INDIGO: u32 = 0x5e5ce6;
pub const PURPLE: u32 = 0xbf5af2;

const ROLES: [u32; 11] = [PINK, RED, ORANGE, YELLOW, GREEN, MINT, TEAL, CYAN, BLUE, INDIGO, PURPLE];

pub const THEME_NAMES: [&str; 5] = ["Full", "Cool", "Cold", "Warm", "Hot"];

/// Per theme, the colour for each role in ROLES order.
const THEMES: [[u32; 11]; 5] = [
    // Full: every hue, Apple-style system colours.
    ROLES,
    // Cool: teals, blues and violets.
    [0xb18cff, 0xf27ba8, 0x7aa2ff, 0x8fd3ff, 0x3ccfb4, 0x6ee7d8, 0x2dd4bf, 0x67e8f9, 0x60a5fa, 0x818cf8, 0xa78bfa],
    // Cold: icy blues, glacier cyans and frost whites.
    [0xa5c8ff, 0x93b4f5, 0xb9d7ff, 0xe0f2fe, 0x9ff3e6, 0xccfbf1, 0x7dd3fc, 0xa5f3fc, 0x93c5fd, 0xa5b4fc, 0xc7d2fe],
    // Warm: amber, coral, peach and gold.
    [0xff8a80, 0xff6f61, 0xffa94d, 0xffd166, 0xe9c46a, 0xf6bd60, 0xf4a261, 0xffb4a2, 0xe76f51, 0xf28482, 0xffa07a],
    // Hot: reds, magentas and blazing orange.
    [0xff2d6f, 0xff1744, 0xff5722, 0xff9100, 0xff6d00, 0xff4081, 0xff3d00, 0xff6e40, 0xf50057, 0xd500f9, 0xe040fb],
];

/// The current theme's shade of a role colour.
pub fn themed(c: u32) -> u32 {
    let t = config::cur().i("theme_colors").clamp(0, 4) as usize;
    match ROLES.iter().position(|r| *r == c) {
        Some(i) => THEMES[t][i],
        None => c,
    }
}
