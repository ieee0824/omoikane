//! Typed views of commonly read computed CSS keywords.

use super::{ComputedStyle, ComputedValue};

/// A CSS-wide keyword retained in a computed-value map.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CssWideKeyword {
    Inherit,
    Initial,
    Unset,
    Revert,
    RevertLayer,
    RevertRule,
}

/// A commonly used computed `display` keyword.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComputedDisplay<'a> {
    None,
    Contents,
    Block,
    Inline,
    InlineBlock,
    FlowRoot,
    Flex,
    InlineFlex,
    Grid,
    InlineGrid,
    Table,
    InlineTable,
    TableRow,
    TableCell,
    ListItem,
    CssWide(CssWideKeyword),
    Other(&'a str),
}

/// A computed `position` keyword.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComputedPosition<'a> {
    Static,
    Relative,
    Sticky,
    Absolute,
    Fixed,
    CssWide(CssWideKeyword),
    Other(&'a str),
}

/// A computed `float` keyword.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComputedFloat<'a> {
    None,
    Left,
    Right,
    InlineStart,
    InlineEnd,
    CssWide(CssWideKeyword),
    Other(&'a str),
}

/// A computed `direction` keyword.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComputedDirection<'a> {
    Ltr,
    Rtl,
    CssWide(CssWideKeyword),
    Other(&'a str),
}

/// A computed `writing-mode` keyword.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComputedWritingMode<'a> {
    HorizontalTb,
    VerticalRl,
    VerticalLr,
    SidewaysRl,
    SidewaysLr,
    CssWide(CssWideKeyword),
    Other(&'a str),
}

/// A computed `break-before` or `break-after` keyword.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComputedBreak<'a> {
    Auto,
    Avoid,
    Page,
    Left,
    Right,
    Recto,
    Verso,
    Column,
    AvoidPage,
    AvoidColumn,
    Region,
    AvoidRegion,
    CssWide(CssWideKeyword),
    Other(&'a str),
}

fn keyword(value: Option<&ComputedValue>) -> Option<&str> {
    match value {
        Some(ComputedValue::Keyword(value)) => Some(value),
        _ => None,
    }
}

fn keyword_or_string(value: Option<&ComputedValue>) -> Option<&str> {
    match value {
        Some(ComputedValue::Keyword(value) | ComputedValue::String(value)) => Some(value),
        _ => None,
    }
}

fn css_wide(value: &str) -> Option<CssWideKeyword> {
    if value.eq_ignore_ascii_case("inherit") {
        Some(CssWideKeyword::Inherit)
    } else if value.eq_ignore_ascii_case("initial") {
        Some(CssWideKeyword::Initial)
    } else if value.eq_ignore_ascii_case("unset") {
        Some(CssWideKeyword::Unset)
    } else if value.eq_ignore_ascii_case("revert") {
        Some(CssWideKeyword::Revert)
    } else if value.eq_ignore_ascii_case("revert-layer") {
        Some(CssWideKeyword::RevertLayer)
    } else if value.eq_ignore_ascii_case("revert-rule") {
        Some(CssWideKeyword::RevertRule)
    } else {
        None
    }
}

impl ComputedStyle {
    /// Returns a typed view of `display`, or `None` when it is absent or non-keyword.
    pub fn display(&self) -> Option<ComputedDisplay<'_>> {
        let value = keyword(self.get("display"))?;
        Some(if let Some(wide) = css_wide(value) {
            ComputedDisplay::CssWide(wide)
        } else if value.eq_ignore_ascii_case("none") {
            ComputedDisplay::None
        } else if value.eq_ignore_ascii_case("contents") {
            ComputedDisplay::Contents
        } else if value.eq_ignore_ascii_case("block") {
            ComputedDisplay::Block
        } else if value.eq_ignore_ascii_case("inline") {
            ComputedDisplay::Inline
        } else if value.eq_ignore_ascii_case("inline-block") {
            ComputedDisplay::InlineBlock
        } else if value.eq_ignore_ascii_case("flow-root") {
            ComputedDisplay::FlowRoot
        } else if value.eq_ignore_ascii_case("flex") {
            ComputedDisplay::Flex
        } else if value.eq_ignore_ascii_case("inline-flex") {
            ComputedDisplay::InlineFlex
        } else if value.eq_ignore_ascii_case("grid") {
            ComputedDisplay::Grid
        } else if value.eq_ignore_ascii_case("inline-grid") {
            ComputedDisplay::InlineGrid
        } else if value.eq_ignore_ascii_case("table") {
            ComputedDisplay::Table
        } else if value.eq_ignore_ascii_case("inline-table") {
            ComputedDisplay::InlineTable
        } else if value.eq_ignore_ascii_case("table-row") {
            ComputedDisplay::TableRow
        } else if value.eq_ignore_ascii_case("table-cell") {
            ComputedDisplay::TableCell
        } else if value.eq_ignore_ascii_case("list-item") {
            ComputedDisplay::ListItem
        } else {
            ComputedDisplay::Other(value)
        })
    }

    /// Tests `display: none` without repeating a string lookup at the call site.
    pub fn is_display_none(&self) -> bool {
        self.display() == Some(ComputedDisplay::None)
    }

    /// Returns a typed view of `position`, or `None` when it is absent or non-keyword.
    pub fn position(&self) -> Option<ComputedPosition<'_>> {
        let value = keyword(self.get("position"))?;
        Some(if let Some(wide) = css_wide(value) {
            ComputedPosition::CssWide(wide)
        } else if value.eq_ignore_ascii_case("static") {
            ComputedPosition::Static
        } else if value.eq_ignore_ascii_case("relative") {
            ComputedPosition::Relative
        } else if value.eq_ignore_ascii_case("sticky") {
            ComputedPosition::Sticky
        } else if value.eq_ignore_ascii_case("absolute") {
            ComputedPosition::Absolute
        } else if value.eq_ignore_ascii_case("fixed") {
            ComputedPosition::Fixed
        } else {
            ComputedPosition::Other(value)
        })
    }

    /// Returns a typed view of `float`, or `None` when it is absent or non-keyword.
    pub fn float(&self) -> Option<ComputedFloat<'_>> {
        let value = keyword(self.get("float"))?;
        Some(if let Some(wide) = css_wide(value) {
            ComputedFloat::CssWide(wide)
        } else if value.eq_ignore_ascii_case("none") {
            ComputedFloat::None
        } else if value.eq_ignore_ascii_case("left") {
            ComputedFloat::Left
        } else if value.eq_ignore_ascii_case("right") {
            ComputedFloat::Right
        } else if value.eq_ignore_ascii_case("inline-start") {
            ComputedFloat::InlineStart
        } else if value.eq_ignore_ascii_case("inline-end") {
            ComputedFloat::InlineEnd
        } else {
            ComputedFloat::Other(value)
        })
    }

    /// Returns a typed view of `direction`, including string-backed values.
    pub fn direction(&self) -> Option<ComputedDirection<'_>> {
        let value = keyword_or_string(self.get("direction"))?;
        Some(if let Some(wide) = css_wide(value) {
            ComputedDirection::CssWide(wide)
        } else if value.eq_ignore_ascii_case("ltr") {
            ComputedDirection::Ltr
        } else if value.eq_ignore_ascii_case("rtl") {
            ComputedDirection::Rtl
        } else {
            ComputedDirection::Other(value)
        })
    }

    /// Returns a typed view of `writing-mode`, including string-backed values.
    pub fn writing_mode(&self) -> Option<ComputedWritingMode<'_>> {
        let value = keyword_or_string(self.get("writing-mode"))?;
        Some(if let Some(wide) = css_wide(value) {
            ComputedWritingMode::CssWide(wide)
        } else if value.eq_ignore_ascii_case("horizontal-tb") {
            ComputedWritingMode::HorizontalTb
        } else if value.eq_ignore_ascii_case("vertical-rl") {
            ComputedWritingMode::VerticalRl
        } else if value.eq_ignore_ascii_case("vertical-lr") {
            ComputedWritingMode::VerticalLr
        } else if value.eq_ignore_ascii_case("sideways-rl") {
            ComputedWritingMode::SidewaysRl
        } else if value.eq_ignore_ascii_case("sideways-lr") {
            ComputedWritingMode::SidewaysLr
        } else {
            ComputedWritingMode::Other(value)
        })
    }

    /// Returns a typed view of `break-before`, or `None` when absent.
    pub fn break_before(&self) -> Option<ComputedBreak<'_>> {
        self.break_value("break-before")
    }

    /// Returns a typed view of `break-after`, or `None` when absent.
    pub fn break_after(&self) -> Option<ComputedBreak<'_>> {
        self.break_value("break-after")
    }

    fn break_value(&self, name: &str) -> Option<ComputedBreak<'_>> {
        let value = keyword(self.get(name))?;
        Some(if let Some(wide) = css_wide(value) {
            ComputedBreak::CssWide(wide)
        } else if value.eq_ignore_ascii_case("auto") {
            ComputedBreak::Auto
        } else if value.eq_ignore_ascii_case("avoid") {
            ComputedBreak::Avoid
        } else if value.eq_ignore_ascii_case("page") {
            ComputedBreak::Page
        } else if value.eq_ignore_ascii_case("left") {
            ComputedBreak::Left
        } else if value.eq_ignore_ascii_case("right") {
            ComputedBreak::Right
        } else if value.eq_ignore_ascii_case("recto") {
            ComputedBreak::Recto
        } else if value.eq_ignore_ascii_case("verso") {
            ComputedBreak::Verso
        } else if value.eq_ignore_ascii_case("column") {
            ComputedBreak::Column
        } else if value.eq_ignore_ascii_case("avoid-page") {
            ComputedBreak::AvoidPage
        } else if value.eq_ignore_ascii_case("avoid-column") {
            ComputedBreak::AvoidColumn
        } else if value.eq_ignore_ascii_case("region") {
            ComputedBreak::Region
        } else if value.eq_ignore_ascii_case("avoid-region") {
            ComputedBreak::AvoidRegion
        } else {
            ComputedBreak::Other(value)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_keywords_preserve_case_wide_values_and_missing_properties() {
        let mut style = ComputedStyle::default();
        assert_eq!(style.display(), None);
        assert_eq!(style.position(), None);
        assert_eq!(style.float(), None);
        assert_eq!(style.direction(), None);
        assert_eq!(style.writing_mode(), None);
        assert_eq!(style.break_before(), None);
        assert_eq!(style.break_after(), None);
        assert!(!style.is_display_none());

        style.set_paint_value("display", "NoNe".to_string());
        style.set_paint_value("position", "ReLaTiVe".to_string());
        style.set_paint_value("float", "InLiNe-StArT".to_string());
        style.set_paint_value("direction", "RtL".to_string());
        style.set_paint_value("writing-mode", "SiDeWaYs-Rl".to_string());
        style.set_paint_value("break-before", "CoLuMn".to_string());
        style.set_paint_value("break-after", "AvOiD-PaGe".to_string());
        assert_eq!(style.display(), Some(ComputedDisplay::None));
        assert!(style.is_display_none());
        assert_eq!(style.position(), Some(ComputedPosition::Relative));
        assert_eq!(style.float(), Some(ComputedFloat::InlineStart));
        assert_eq!(style.direction(), Some(ComputedDirection::Rtl));
        assert_eq!(style.writing_mode(), Some(ComputedWritingMode::SidewaysRl));
        assert_eq!(style.break_before(), Some(ComputedBreak::Column));
        assert_eq!(style.break_after(), Some(ComputedBreak::AvoidPage));

        for name in [
            "display",
            "position",
            "float",
            "direction",
            "writing-mode",
            "break-before",
            "break-after",
        ] {
            style.set_paint_value(name, "ReVeRt-LaYeR".to_string());
        }
        assert_eq!(
            style.display(),
            Some(ComputedDisplay::CssWide(CssWideKeyword::RevertLayer))
        );
        assert_eq!(
            style.position(),
            Some(ComputedPosition::CssWide(CssWideKeyword::RevertLayer))
        );
        assert_eq!(
            style.float(),
            Some(ComputedFloat::CssWide(CssWideKeyword::RevertLayer))
        );
        assert_eq!(
            style.direction(),
            Some(ComputedDirection::CssWide(CssWideKeyword::RevertLayer))
        );
        assert_eq!(
            style.writing_mode(),
            Some(ComputedWritingMode::CssWide(CssWideKeyword::RevertLayer))
        );
        assert_eq!(
            style.break_before(),
            Some(ComputedBreak::CssWide(CssWideKeyword::RevertLayer))
        );
        assert_eq!(
            style.break_after(),
            Some(ComputedBreak::CssWide(CssWideKeyword::RevertLayer))
        );
    }
}
