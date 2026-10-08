//! Owned device colors, independent of native preference capture.

/// RGB system colors used by a forced-color presentation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ForcedColorPalette {
    /// Background of ordinary content.
    pub canvas: [u8; 3],
    /// Text in ordinary content.
    pub canvas_text: [u8; 3],
    /// Unvisited link text.
    pub link_text: [u8; 3],
    /// Visited link text.
    pub visited_text: [u8; 3],
    /// Button background.
    pub button_face: [u8; 3],
    /// Button text.
    pub button_text: [u8; 3],
    /// Input field background.
    pub field: [u8; 3],
    /// Input field text.
    pub field_text: [u8; 3],
    /// Disabled control text.
    pub gray_text: [u8; 3],
    /// Selected content background.
    pub highlight: [u8; 3],
    /// Selected content text.
    pub highlight_text: [u8; 3],
}

impl ForcedColorPalette {
    /// Looks up a CSS Color 4 system color in this owned presentation palette.
    pub(crate) fn system_color(&self, name: &str) -> Option<[u8; 3]> {
        Some(match name.to_ascii_lowercase().as_str() {
            "canvas" => self.canvas,
            "canvastext" => self.canvas_text,
            "linktext" | "activetext" => self.link_text,
            "visitedtext" => self.visited_text,
            "buttonface" => self.button_face,
            "buttontext" | "buttonborder" => self.button_text,
            "field" => self.field,
            "fieldtext" => self.field_text,
            "graytext" => self.gray_text,
            "highlight" | "accentcolor" | "mark" | "selecteditem" => self.highlight,
            "highlighttext" | "accentcolortext" | "marktext" | "selecteditemtext" => {
                self.highlight_text
            }
            _ => return None,
        })
    }

    /// Classifies Canvas using D50 Lab lightness: below 33 is dark, above
    /// 67 is light. Intermediate colors leave the host preference unchanged.
    pub(crate) fn dark_color_scheme(&self) -> Option<bool> {
        let linear = self.canvas.map(|channel| {
            let value = f64::from(channel) / 255.0;
            if value <= 0.04045 {
                value / 12.92
            } else {
                ((value + 0.055) / 1.055).powf(2.4)
            }
        });
        // Y row of linear sRGB -> XYZ D65 -> Bradford-adapted XYZ D50.
        let y = 0.22249317711056527 * linear[0]
            + 0.7168870130944824 * linear[1]
            + 0.060619809794952365 * linear[2];
        let lightness = if y > 216.0 / 24389.0 {
            116.0 * y.cbrt() - 16.0
        } else {
            (24389.0 / 27.0) * y
        };
        if lightness < 33.0 {
            Some(true)
        } else if lightness > 67.0 {
            Some(false)
        } else {
            None
        }
    }

    /// Provides deterministic software presentation colors for a color scheme.
    pub fn for_color_scheme(dark: bool) -> Self {
        let (background, foreground) = if dark {
            ([0, 0, 0], [255, 255, 255])
        } else {
            ([255, 255, 255], [0, 0, 0])
        };
        Self {
            canvas: background,
            canvas_text: foreground,
            link_text: if dark { [255, 255, 0] } else { [0, 0, 159] },
            visited_text: if dark { [0, 255, 0] } else { [85, 26, 139] },
            button_face: background,
            button_text: foreground,
            field: background,
            field_text: foreground,
            gray_text: [128, 128, 128],
            highlight: foreground,
            highlight_text: background,
        }
    }
}
