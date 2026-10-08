//! Native preference capture, separate from deterministic media evaluation.
use crate::css::{ForcedColorPalette, MediaEnvironment};
#[path = "platform_media/input.rs"]
mod input;
pub use input::{PointerCapabilities, PointerDeviceTracker};
#[cfg(all(target_os = "macos", feature = "gui"))]
#[path = "platform_media/macos.rs"]
mod macos;
#[cfg(all(target_os = "macos", feature = "gui"))]
// Native imports are implementation details, outside the public C ABI.
#[doc = "cbindgen:ignore"]
#[path = "platform_media/macos_input.rs"]
mod macos_input;
#[path = "platform_media/monitor.rs"]
mod monitor;
#[cfg(all(target_os = "windows", feature = "gui"))]
#[path = "platform_media/windows.rs"]
mod windows;
#[cfg(all(target_os = "linux", feature = "gui"))]
#[path = "platform_media/x11_input.rs"]
mod x11_input;
pub use monitor::MediaPreferenceMonitor;

/// Contrast preference reported by the host, independent of palette activation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContrastPreference {
    /// The host reports no preference for altered contrast.
    NoPreference,
    /// The host reports a preference for increased contrast.
    More,
    /// The host reports a preference for decreased contrast.
    Less,
    /// A chosen forced palette has no established high/low classification.
    Custom,
}

impl ContrastPreference {
    fn from_higher(higher: bool) -> Self {
        if higher {
            Self::More
        } else {
            Self::NoPreference
        }
    }

    fn as_media_value(self) -> &'static str {
        match self {
            Self::NoPreference => "no-preference",
            Self::More => "more",
            Self::Less => "less",
            Self::Custom => "custom",
        }
    }
}

/// An owned snapshot of preferences actually reported by the presentation host.
/// `None` means the host has no preference or the setting is unavailable.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PlatformMediaPreferences {
    /// Explicit preferred appearance; absent values preserve the window theme.
    pub dark_appearance: Option<bool>,
    /// Whether nonessential motion should be reduced.
    pub reduced_motion: Option<bool>,
    /// Explicit contrast preference; absent values preserve the snapshot.
    pub contrast: Option<ContrastPreference>,
    /// Whether the presentation host forces a limited color palette.
    pub forced_colors: Option<bool>,
    /// Owned active system palette; absent samples preserve the previous palette.
    /// Explicitly disabling forced colors clears it, even when this is absent.
    pub forced_color_palette: Option<ForcedColorPalette>,
    /// Whether semitransparent presentation should be reduced.
    pub reduced_transparency: Option<bool>,
    /// Whether the presentation host inverts displayed colors.
    pub inverted_colors: Option<bool>,
    /// Available pointing devices; absent values leave host fallback policy intact.
    pub pointers: Option<PointerCapabilities>,
}

impl PlatformMediaPreferences {
    /// Applies only reported values to a snapshot without performing I/O.
    pub fn apply_to(&self, environment: &mut MediaEnvironment) {
        if let Some(pointers) = self.pointers {
            pointers.apply_to(environment);
        }
        if let Some(dark) = self.dark_appearance {
            environment.color_scheme_dark = dark;
        }
        if let Some(reduce) = self.reduced_motion {
            environment.set_feature(
                "prefers-reduced-motion",
                if reduce { "reduce" } else { "no-preference" },
            );
        }
        if let Some(reduced) = self.reduced_transparency {
            environment.set_feature(
                "prefers-reduced-transparency",
                if reduced { "reduce" } else { "no-preference" },
            );
        }
        if let Some(inverted) = self.inverted_colors {
            environment.set_feature(
                "inverted-colors",
                if inverted { "inverted" } else { "none" },
            );
        }
        if let Some(forced) = self.forced_colors {
            environment.set_feature("forced-colors", if forced { "active" } else { "none" });
        }
        if self.forced_colors == Some(false) {
            environment.forced_color_palette = None;
        } else if let Some(palette) = self.forced_color_palette {
            environment.forced_color_palette = Some(palette);
        }
        if let Some(contrast) = self.contrast {
            environment.set_feature("prefers-contrast", contrast.as_media_value());
        }
    }
}

/// Queries the presentation host's available pointing devices, performing native I/O.
/// Unsupported backends return `None`, allowing explicit host fallback policy.
pub fn capture_pointer_capabilities() -> Option<PointerCapabilities> {
    #[cfg(all(target_os = "linux", feature = "gui"))]
    {
        x11_input::capture()
    }
    #[cfg(all(target_os = "windows", feature = "gui"))]
    {
        windows::capture_pointers()
    }
    #[cfg(all(target_os = "macos", feature = "gui"))]
    {
        macos_input::capture()
    }
    #[cfg(not(all(
        any(target_os = "linux", target_os = "windows", target_os = "macos"),
        feature = "gui"
    )))]
    {
        None
    }
}

/// Captures native preferences using the platform accessibility settings.
/// This performs process and session-bus I/O; callers should sample away from
/// rendering and input dispatch. Each Linux portal request has a one-second timeout.
/// Missing portal/tool settings and currently unsupported platforms are absent.
/// macOS GUI capture must run on the main thread; other threads return absent
/// preferences. The macOS monitor uses the main operation queue.
pub fn capture_media_preferences() -> PlatformMediaPreferences {
    #[cfg(target_os = "linux")]
    {
        let preferences = capture_portal_preferences(read_portal_setting);
        #[cfg(feature = "gui")]
        let preferences = PlatformMediaPreferences {
            pointers: x11_input::capture(),
            ..preferences
        };
        preferences
    }
    #[cfg(all(target_os = "windows", feature = "gui"))]
    {
        windows::capture()
    }
    #[cfg(all(target_os = "macos", feature = "gui"))]
    {
        macos::capture()
    }
    #[cfg(not(any(
        target_os = "linux",
        all(any(target_os = "windows", target_os = "macos"), feature = "gui")
    )))]
    {
        PlatformMediaPreferences::default()
    }
}

#[cfg(any(target_os = "linux", test))]
fn capture_portal_preferences(
    mut read: impl FnMut(&str) -> Option<u32>,
) -> PlatformMediaPreferences {
    PlatformMediaPreferences {
        dark_appearance: match read("color-scheme") {
            Some(1) => Some(true),
            Some(2) => Some(false),
            _ => None,
        },
        reduced_motion: read("reduced-motion").map(|value| value == 1),
        contrast: read("contrast").map(|value| ContrastPreference::from_higher(value == 1)),
        ..PlatformMediaPreferences::default()
    }
}

#[cfg(any(all(target_os = "windows", feature = "gui"), test))]
fn windows_preferences(
    animations: Option<bool>,
    high_contrast: Option<bool>,
) -> PlatformMediaPreferences {
    PlatformMediaPreferences {
        reduced_motion: animations.map(|enabled| !enabled),
        // The flag identifies palette activation, not its contrast ratio.
        // Until the selected colors are classified, MQ5 requires custom.
        contrast: high_contrast.map(|active| {
            if active {
                ContrastPreference::Custom
            } else {
                ContrastPreference::NoPreference
            }
        }),
        forced_colors: high_contrast,
        ..PlatformMediaPreferences::default()
    }
}

#[cfg(target_os = "linux")]
fn read_portal_setting(key: &str) -> Option<u32> {
    use std::process::{Command, Stdio};
    let output = Command::new("gdbus")
        .args([
            "call",
            "--session",
            "--timeout",
            "1",
            "--dest",
            "org.freedesktop.portal.Desktop",
            "--object-path",
            "/org/freedesktop/portal/desktop",
            "--method",
            "org.freedesktop.portal.Settings.ReadOne",
            "org.freedesktop.appearance",
            key,
        ])
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    parse_portal_uint(std::str::from_utf8(&output.stdout).ok()?)
}

#[cfg(any(target_os = "linux", test))]
fn parse_portal_uint(output: &str) -> Option<u32> {
    let input = output.trim().trim_start_matches(['(', '<']);
    let input = input.strip_prefix("uint32 ")?;
    let length = input.bytes().take_while(u8::is_ascii_digit).count();
    if length == 0
        || !input[length..]
            .chars()
            .all(|ch| matches!(ch, '>' | ')' | ',') || ch.is_ascii_whitespace())
    {
        return None;
    }
    input[..length].parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn portal_preferences_apply_reported_values_and_preserve_unavailable_values() {
        let snapshot = capture_portal_preferences(|key| match key {
            "color-scheme" => Some(1),
            "reduced-motion" => Some(1),
            "contrast" => Some(1),
            _ => None,
        });
        let mut environment = MediaEnvironment::default();
        snapshot.apply_to(&mut environment);
        assert!(environment.color_scheme_dark);
        let evaluate = |text: &str| {
            crate::css::evaluate_media_query_with_environment(
                &crate::css::parse_media_query_list(text).unwrap()[0],
                800.0,
                600.0,
                &environment,
            )
        };
        assert!(evaluate(
            "(prefers-reduced-motion: reduce) and (prefers-contrast: more)"
        ));
        let previous = environment.clone();
        capture_portal_preferences(|_| None).apply_to(&mut environment);
        assert_eq!(environment, previous);
        let reset = capture_portal_preferences(|_| Some(0));
        assert_eq!(reset.dark_appearance, None);
        assert_eq!(reset.reduced_motion, Some(false));
        assert_eq!(reset.contrast, Some(ContrastPreference::NoPreference));
        assert_eq!(capture_portal_preferences(|_| Some(99)), reset);
    }

    #[test]
    fn windows_animation_and_high_contrast_flags_map_to_media_preferences() {
        let enabled = windows_preferences(Some(false), Some(true));
        assert_eq!(enabled.reduced_motion, Some(true));
        assert_eq!(enabled.contrast, Some(ContrastPreference::Custom));
        assert_eq!(enabled.forced_colors, Some(true));
        let mut environment = MediaEnvironment::default();
        enabled.apply_to(&mut environment);
        let query = crate::css::parse_media_query_list("(prefers-reduced-motion: reduce) and (prefers-contrast: custom) and (forced-colors: active)").unwrap();
        assert!(crate::css::evaluate_media_query_with_environment(
            &query[0],
            800.0,
            600.0,
            &environment,
        ));
        let normal = windows_preferences(Some(true), Some(false));
        assert_eq!(normal.reduced_motion, Some(false));
        assert_eq!(normal.forced_colors, Some(false));
        assert_eq!(
            windows_preferences(None, None),
            PlatformMediaPreferences::default()
        );
    }

    #[test]
    fn explicit_contrast_snapshots_preserve_all_values_and_unavailable_capture() {
        for preference in [
            ContrastPreference::NoPreference,
            ContrastPreference::More,
            ContrastPreference::Less,
            ContrastPreference::Custom,
        ] {
            let mut environment = MediaEnvironment::default();
            PlatformMediaPreferences {
                contrast: Some(preference),
                ..Default::default()
            }
            .apply_to(&mut environment);
            for candidate in ["no-preference", "more", "less", "custom"] {
                let queries =
                    crate::css::parse_media_query_list(&format!("(prefers-contrast: {candidate})"))
                        .unwrap();
                assert_eq!(
                    crate::css::evaluate_media_query_with_environment(
                        &queries[0],
                        800.0,
                        600.0,
                        &environment
                    ),
                    candidate == preference.as_media_value()
                );
            }
            let previous = environment.clone();
            PlatformMediaPreferences::default().apply_to(&mut environment);
            assert_eq!(environment, previous);
        }
    }

    #[test]
    fn windows_custom_palette_is_not_assumed_to_have_high_contrast() {
        let mut environment = MediaEnvironment::default();
        windows_preferences(Some(true), Some(true)).apply_to(&mut environment);
        let matches = |query: &str| {
            let queries = crate::css::parse_media_query_list(query).unwrap();
            crate::css::evaluate_media_query_with_environment(
                &queries[0],
                800.0,
                600.0,
                &environment,
            )
        };
        assert!(matches(
            "(forced-colors: active) and (prefers-contrast: custom)"
        ));
        assert!(!matches("(prefers-contrast: more)"));
    }

    #[test]
    fn portal_output_requires_a_single_unsigned_typed_value() {
        for output in ["(<uint32 1>,)", "(<<uint32 1>>,)", "(<uint32 1>,)\n"] {
            assert_eq!(parse_portal_uint(output), Some(1));
        }
        for output in [
            "(<true>,)",
            "(<uint32 -1>,)",
            "(<uint32 1>, <uint32 2>)",
            "(<uint32 4294967296>,)",
            "garbage uint32 1",
        ] {
            assert_eq!(parse_portal_uint(output), None, "{output}");
        }
    }
    #[test]
    fn disabling_native_forced_colors_clears_the_previous_palette() {
        let mut environment = MediaEnvironment::default();
        environment.set_feature("forced-colors", "active");
        environment.forced_color_palette =
            Some(crate::css::ForcedColorPalette::for_color_scheme(true));
        PlatformMediaPreferences {
            forced_colors: Some(false),
            ..Default::default()
        }
        .apply_to(&mut environment);
        assert_eq!(environment.forced_color_palette, None);
    }

    #[test]
    fn native_palette_updates_are_owned_and_unavailable_samples_preserve_them() {
        let mut environment = MediaEnvironment::default();
        let mut palette = ForcedColorPalette::for_color_scheme(false);
        palette.canvas = [11, 22, 33];
        let initial = PlatformMediaPreferences {
            forced_colors: Some(true),
            forced_color_palette: Some(palette),
            ..Default::default()
        };
        initial.apply_to(&mut environment);
        palette.canvas = [44, 55, 66];
        assert_eq!(
            environment.forced_color_palette.unwrap().canvas,
            [11, 22, 33]
        );
        let previous = environment.clone();
        PlatformMediaPreferences::default().apply_to(&mut environment);
        assert_eq!(environment, previous);
        PlatformMediaPreferences {
            forced_color_palette: Some(palette),
            ..initial.clone()
        }
        .apply_to(&mut environment);
        assert_eq!(
            environment.forced_color_palette.unwrap().canvas,
            [44, 55, 66]
        );
        PlatformMediaPreferences {
            forced_colors: Some(false),
            ..initial
        }
        .apply_to(&mut environment);
        assert_eq!(environment.forced_color_palette, None);
    }
}
