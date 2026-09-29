//! Mapping CSS logical box properties onto physical layout properties.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct LogicalFlow {
    block_start: &'static str,
    block_end: &'static str,
    inline_start: &'static str,
    inline_end: &'static str,
    inline_dimension: &'static str,
    block_dimension: &'static str,
}

impl LogicalFlow {
    pub(super) fn new(writing_mode: &str, direction: &str) -> Self {
        let rtl = direction == "rtl";
        match writing_mode {
            "vertical-rl" | "sideways-rl" => Self {
                block_start: "right",
                block_end: "left",
                inline_start: if rtl { "bottom" } else { "top" },
                inline_end: if rtl { "top" } else { "bottom" },
                inline_dimension: "height",
                block_dimension: "width",
            },
            "vertical-lr" | "sideways-lr" => Self {
                block_start: "left",
                block_end: "right",
                inline_start: if rtl { "bottom" } else { "top" },
                inline_end: if rtl { "top" } else { "bottom" },
                inline_dimension: "height",
                block_dimension: "width",
            },
            _ => Self {
                block_start: "top",
                block_end: "bottom",
                inline_start: if rtl { "right" } else { "left" },
                inline_end: if rtl { "left" } else { "right" },
                inline_dimension: "width",
                block_dimension: "height",
            },
        }
    }

    fn side(self, logical: &str) -> Option<&'static str> {
        match logical {
            "block-start" => Some(self.block_start),
            "block-end" => Some(self.block_end),
            "inline-start" => Some(self.inline_start),
            "inline-end" => Some(self.inline_end),
            _ => None,
        }
    }

    pub(super) fn physical_name(self, name: &str) -> Option<String> {
        for (prefix, physical_prefix) in [
            ("margin-", "margin-"),
            ("padding-", "padding-"),
            ("inset-", ""),
            ("scroll-margin-", "scroll-margin-"),
            ("scroll-padding-", "scroll-padding-"),
        ] {
            if let Some(logical) = name.strip_prefix(prefix)
                && let Some(side) = self.side(logical)
            {
                return Some(format!("{physical_prefix}{side}"));
            }
        }
        if let Some(rest) = name.strip_prefix("border-") {
            if let Some((side, component)) = rest.rsplit_once('-')
                && matches!(component, "width" | "style" | "color")
                && let Some(physical) = self.side(side)
            {
                return Some(format!("border-{physical}-{component}"));
            }
            if let Some(logical) = rest.strip_suffix("-radius") {
                let (block, inline) = logical.split_once('-')?;
                if matches!(block, "start" | "end") && matches!(inline, "start" | "end") {
                    let block = self.side(&format!("block-{block}"))?;
                    let inline = self.side(&format!("inline-{inline}"))?;
                    let (vertical, horizontal) = if matches!(block, "top" | "bottom") {
                        (block, inline)
                    } else {
                        (inline, block)
                    };
                    return Some(format!("border-{vertical}-{horizontal}-radius"));
                }
            }
        }
        let dimension = match name {
            "inline-size" => Some(self.inline_dimension.to_string()),
            "block-size" => Some(self.block_dimension.to_string()),
            "min-inline-size" => Some(format!("min-{}", self.inline_dimension)),
            "min-block-size" => Some(format!("min-{}", self.block_dimension)),
            "max-inline-size" => Some(format!("max-{}", self.inline_dimension)),
            "max-block-size" => Some(format!("max-{}", self.block_dimension)),
            "contain-intrinsic-inline-size" => {
                Some(format!("contain-intrinsic-{}", self.inline_dimension))
            }
            "contain-intrinsic-block-size" => {
                Some(format!("contain-intrinsic-{}", self.block_dimension))
            }
            "overscroll-behavior-inline" => Some(format!(
                "overscroll-behavior-{}",
                if self.inline_dimension == "width" {
                    "x"
                } else {
                    "y"
                }
            )),
            "overscroll-behavior-block" => Some(format!(
                "overscroll-behavior-{}",
                if self.block_dimension == "width" {
                    "x"
                } else {
                    "y"
                }
            )),
            _ => None,
        };
        dimension
    }
}

#[cfg(test)]
mod tests {
    use super::LogicalFlow;

    #[test]
    fn maps_sides_and_corners_for_each_flow() {
        for (mode, direction, expected) in [
            ("horizontal-tb", "ltr", "border-top-left-radius"),
            ("horizontal-tb", "rtl", "border-top-right-radius"),
            ("vertical-rl", "ltr", "border-top-right-radius"),
            ("vertical-rl", "rtl", "border-bottom-right-radius"),
            ("vertical-lr", "ltr", "border-top-left-radius"),
            ("vertical-lr", "rtl", "border-bottom-left-radius"),
        ] {
            let flow = LogicalFlow::new(mode, direction);
            assert_eq!(
                flow.physical_name("border-start-start-radius").as_deref(),
                Some(expected)
            );
            assert_eq!(
                flow.physical_name("margin-inline-start").as_deref(),
                Some(flow.inline_start).map(|side| match side {
                    "top" => "margin-top",
                    "right" => "margin-right",
                    "bottom" => "margin-bottom",
                    _ => "margin-left",
                })
            );
        }
    }

    #[test]
    fn maps_dimensions_and_border_components() {
        let flow = LogicalFlow::new("vertical-rl", "ltr");
        assert_eq!(flow.physical_name("inline-size").as_deref(), Some("height"));
        assert_eq!(
            flow.physical_name("min-block-size").as_deref(),
            Some("min-width")
        );
        assert_eq!(
            flow.physical_name("inset-inline-end").as_deref(),
            Some("bottom")
        );
        assert_eq!(
            flow.physical_name("border-block-start-width").as_deref(),
            Some("border-right-width")
        );
    }
}
