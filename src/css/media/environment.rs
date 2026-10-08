//! Owned media settings supplied explicitly to pure query evaluation.
use super::{ForcedColorPalette, MediaType};
use std::collections::BTreeMap;

/// A snapshot of input devices, display capabilities and user preferences.
/// Callers configure a snapshot before passing it to query evaluation.
#[derive(Debug, Clone, PartialEq)]
pub struct MediaEnvironment {
    features: BTreeMap<String, String>,
    /// Host-selected colors; absent values use deterministic software colors.
    pub forced_color_palette: Option<ForcedColorPalette>,
    /// Output medium selected by the presentation host.
    pub media_type: MediaType,
    /// Ordinary host color preference, retained while an active forced palette
    /// determines the effective scheme.
    pub color_scheme_dark: bool,
    /// Whether the output device has a television scanning mode.
    pub scan_available: bool,
    /// Display resolution in dots per CSS pixel.
    pub resolution_dppx: f32,
    /// Color bits per component supported by the rendering backend.
    pub color_bits_per_component: u32,
    /// Palette entries; zero denotes a direct-color backend.
    pub color_index: u32,
    /// Monochrome bits per pixel; zero denotes a color backend.
    pub monochrome_bits_per_pixel: u32,
    /// Device width in CSS pixels; absent values use the virtual display viewport.
    pub device_width: Option<f32>,
    /// Device height in CSS pixels; absent values use the virtual display viewport.
    pub device_height: Option<f32>,
}

const DISCRETE_FEATURES: &[(&str, &[&str], &str)] = &[
    ("hover", &["none", "hover"], "hover"),
    ("any-hover", &["none", "hover"], "hover"),
    ("pointer", &["none", "coarse", "fine"], "fine"),
    ("any-pointer", &["none", "coarse", "fine"], "fine"),
    ("update", &["none", "slow", "fast"], "fast"),
    ("scan", &["interlace", "progressive"], "progressive"),
    ("grid", &["0", "1"], "0"),
    ("overflow-block", &["none", "scroll", "paged"], "scroll"),
    ("overflow-inline", &["none", "scroll"], "scroll"),
    ("color-gamut", &["srgb", "p3", "rec2020"], "srgb"),
    (
        "prefers-reduced-motion",
        &["no-preference", "reduce"],
        "no-preference",
    ),
    (
        "prefers-reduced-transparency",
        &["no-preference", "reduce"],
        "no-preference",
    ),
    (
        "prefers-reduced-data",
        &["no-preference", "reduce"],
        "no-preference",
    ),
    (
        "prefers-contrast",
        &["no-preference", "more", "less", "custom"],
        "no-preference",
    ),
    ("forced-colors", &["none", "active"], "none"),
    ("inverted-colors", &["none", "inverted"], "none"),
    ("scripting", &["none", "initial-only", "enabled"], "enabled"),
    ("dynamic-range", &["standard", "high"], "standard"),
    ("video-dynamic-range", &["standard", "high"], "standard"),
    (
        "display-mode",
        &[
            "browser",
            "fullscreen",
            "standalone",
            "minimal-ui",
            "picture-in-picture",
            "window-controls-overlay",
        ],
        "browser",
    ),
];

impl Default for MediaEnvironment {
    fn default() -> Self {
        Self {
            features: DISCRETE_FEATURES
                .iter()
                .map(|(name, _, value)| (name.to_string(), value.to_string()))
                .collect(),
            forced_color_palette: None,
            resolution_dppx: 1.0,
            color_bits_per_component: 8,
            color_index: 0,
            monochrome_bits_per_pixel: 0,
            device_width: None,
            device_height: None,
            media_type: MediaType::Screen,
            color_scheme_dark: false,
            scan_available: false,
        }
    }
}

impl MediaEnvironment {
    /// Resolves the preferred scheme from active Canvas colors, preserving
    /// the ordinary host preference for intermediate or inactive palettes.
    pub fn preferred_color_scheme_dark(&self) -> bool {
        self.used_forced_color_palette()
            .and_then(|palette| palette.dark_color_scheme())
            .unwrap_or(self.color_scheme_dark)
    }

    pub(crate) fn used_forced_color_palette(&self) -> Option<ForcedColorPalette> {
        (self.features.get("forced-colors").map(String::as_str) == Some("active")).then(|| {
            self.forced_color_palette
                .unwrap_or_else(|| ForcedColorPalette::for_color_scheme(self.color_scheme_dark))
        })
    }

    /// Replaces one discrete setting if its name and value are supported.
    /// Invalid input leaves the snapshot unchanged.
    pub fn set_feature(&mut self, name: &str, value: &str) -> bool {
        let Some((_, allowed, _)) = DISCRETE_FEATURES
            .iter()
            .find(|(feature, _, _)| *feature == name)
        else {
            return false;
        };
        if !allowed.contains(&value) {
            return false;
        }
        self.features.insert(name.into(), value.into());
        true
    }

    /// Sets the union of capabilities of all attached pointing devices.
    pub fn set_available_pointers(&mut self, fine: bool, coarse: bool) {
        let value = match (fine, coarse) {
            (true, true) => "fine coarse",
            (true, false) => "fine",
            (false, true) => "coarse",
            (false, false) => "none",
        };
        self.features.insert("any-pointer".into(), value.into());
    }

    pub(super) fn matches(&self, name: &str, value: &str) -> Option<bool> {
        if name == "resolution" && value.is_empty() {
            return Some(self.resolution_dppx > 0.0);
        }
        let (_, allowed, _) = DISCRETE_FEATURES
            .iter()
            .find(|(feature, _, _)| *feature == name)?;
        let actual = self.features.get(name)?;
        if value.is_empty() {
            if matches!(name, "dynamic-range" | "video-dynamic-range") {
                // Match the fixed WPT contract and reference-browser behavior.
                return Some(false);
            }
            if name == "scan" && !self.scan_available {
                return Some(false);
            }
            return Some(!matches!(actual.as_str(), "none" | "no-preference" | "0"));
        }
        if !allowed.contains(&value) {
            return None;
        }
        if name == "scan" && !self.scan_available {
            return Some(false);
        }
        if name == "color-gamut" {
            return Some(
                allowed.iter().position(|v| *v == actual)?
                    >= allowed.iter().position(|v| *v == value)?,
            );
        }
        if matches!(name, "dynamic-range" | "video-dynamic-range") && value == "standard" {
            return Some(true);
        }
        if name == "any-pointer" {
            return Some(
                actual
                    .split_whitespace()
                    .any(|capability| capability == value),
            );
        }
        Some(actual == value)
    }

    pub(super) fn recognizes(name: &str) -> bool {
        DISCRETE_FEATURES
            .iter()
            .any(|(feature, _, _)| *feature == name)
    }
}
