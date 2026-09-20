//! Panel theme

/// Appearance mode
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Appearance {
    Light,
    Dark,
}

/// Theme token set adaptive to light/dark
pub struct Theme {
    /// Panel background
    pub bg: [f32; 4],
    /// Primary text
    pub text_primary: [f32; 4],
    /// Secondary text
    pub text_secondary: [f32; 4],
    /// Weak text / labels
    pub text_tertiary: [f32; 4],
    /// Hairline borders and dividers
    pub border: [f32; 4],
    /// Progress track / secondary button fill
    pub track: [f32; 4],
    /// Ember accent color for text
    pub accent: [f32; 4],
    /// Primary button fill
    pub action: [f32; 4],
    /// Primary button text
    pub action_text: [f32; 4],
    /// Tier colors: Forest for normal / Crimson for danger
    pub ok: [f32; 4],
    pub danger: [f32; 4],
    /// Amber for the peak-hours hint
    pub peak: [f32; 4],
    /// Logo tile base color, sharing the source with the primary button
    pub logo_tile: [f32; 4],
    /// Card stroke
    pub card_border: [f32; 4],
}

pub const PANEL_WIDTH: i32 = 340;
/// Control-level corner radius
pub const RADIUS: f32 = 4.0;
/// Card-level corner radius
pub const CARD_RADIUS: f32 = 6.0;

/// Explicit value of the configured appearance string: Some means an explicit light/dark,
/// None means follow the system;
/// trims first then ignores case — the single source of truth for literal parsing,
/// shared by the control highlighting and the effective value.
pub fn explicit_appearance(setting: Option<&str>) -> Option<Appearance> {
    match setting.map(str::trim) {
        Some(s) if s.eq_ignore_ascii_case("light") => Some(Appearance::Light),
        Some(s) if s.eq_ignore_ascii_case("dark") => Some(Appearance::Dark),
        _ => None,
    }
}

/// Resolves the config into the effective appearance: explicit wins; unconfigured/unknown
/// follows the system, and a failed system read falls back to light.
pub fn resolved(setting: Option<&str>) -> Appearance {
    resolved_checked(setting).unwrap_or(Appearance::Light)
}

/// Same resolution with the read-failure case surfaced: None only when
/// following the system and the registry read failed — callers that can
/// keep their previous value (apply_appearance) use this so a transient
/// read failure never flips the theme.
pub fn resolved_checked(setting: Option<&str>) -> Option<Appearance> {
    explicit_appearance(setting).or_else(system_appearance)
}

fn rgba(r: u8, g: u8, b: u8, a: f32) -> [f32; 4] {
    [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, a]
}

impl Theme {
    pub fn new(appearance: Appearance) -> Self {
        match appearance {
            Appearance::Light => Self {
                bg: rgba(0xF7, 0xF7, 0xF4, 1.0),
                text_primary: rgba(0x26, 0x25, 0x1E, 0.94),
                text_secondary: rgba(0x84, 0x84, 0x7E, 1.0),
                text_tertiary: rgba(0x7A, 0x79, 0x74, 1.0),
                border: rgba(0xCD, 0xCD, 0xC9, 1.0),
                track: rgba(0xE6, 0xE5, 0xE0, 1.0),
                accent: rgba(0xF5, 0x4E, 0x00, 1.0),
                action: rgba(0x26, 0x25, 0x1E, 1.0),
                action_text: rgba(0xF7, 0xF7, 0xF4, 1.0),
                ok: rgba(0x34, 0x78, 0x5C, 1.0),
                danger: rgba(0xCF, 0x2D, 0x56, 1.0),
                peak: rgba(0xD9, 0x94, 0x00, 1.0),
                logo_tile: rgba(0x26, 0x25, 0x1E, 1.0),
                card_border: rgba(0xD8, 0xD7, 0xD2, 1.0),
            },
            Appearance::Dark => Self {
                bg: rgba(0x20, 0x1F, 0x1B, 1.0),
                text_primary: rgba(0xEC, 0xEA, 0xE4, 0.94),
                text_secondary: rgba(0xA8, 0xA7, 0xA0, 1.0),
                text_tertiary: rgba(0x7D, 0x7C, 0x75, 1.0),
                border: rgba(0x3B, 0x3A, 0x34, 1.0),
                track: rgba(0x34, 0x33, 0x2E, 1.0),
                accent: rgba(0xFF, 0x6A, 0x2E, 1.0),
                action: rgba(0xEC, 0xE9, 0xE2, 1.0),
                action_text: rgba(0x26, 0x25, 0x1E, 1.0),
                ok: rgba(0x3F, 0xA3, 0x77, 1.0),
                danger: rgba(0xE2, 0x5A, 0x77, 1.0),
                peak: rgba(0xFF, 0xC1, 0x4D, 1.0),
                logo_tile: rgba(0x2D, 0x2D, 0x2D, 1.0),
                card_border: rgba(0x45, 0x44, 0x3E, 1.0),
            },
        }
    }
}

/// Detects the system appearance; None on a registry read failure so
/// callers can keep their last applied value instead of flipping.
pub fn system_appearance() -> Option<Appearance> {
    windows_registry::CURRENT_USER
        .open("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize")
        .and_then(|k| k.get_u32("AppsUseLightTheme"))
        .ok()
        .map(|val| {
            if val == 1 {
                Appearance::Light
            } else {
                Appearance::Dark
            }
        })
}
