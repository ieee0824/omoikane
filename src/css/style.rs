//! CSS cascade and computed style resolution.

mod animation;
mod class_invalidation;
#[cfg(test)]
mod container_dependency_tests;
#[cfg(test)]
mod inherited_lookup_tests;
mod user_action;
pub(crate) use animation::StyleClockEffects;
pub(crate) use user_action::UserActionStyleInvalidation;
mod counter_readers;
mod font_palette;
mod forced_colors;
mod generic_transform;
pub(crate) mod grid_properties;
pub(crate) mod individual_transform;
mod leaf_paint;
mod page;
mod property_id;
mod sibling_functions;
mod specified_math;
mod style_cache;
pub(crate) mod text_shadow;
mod tree_mutation_invalidation;
mod typed_math;
mod will_change;

pub use page::{
    PageBoxGeometry, PageMarginContent, PageSelectorContext, PageSide, ResolvedPageStyle,
};
pub(crate) use property_id::{PropertyId, PropertyMap};
use property_id::{SUPPORTED_PROPERTIES, is_shorthand_or_legacy_alias};

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use super::matcher::{
    SelectorMatchCache, matches_selector_boundary_cached, matches_selector_with_pseudo_cached,
    matches_selector_with_scope_cached,
};
use super::transition::TransitionCacheScope;
use crate::dom::{Node, NodeHandle, NodeType};
use crate::font::{
    CssRelativeFontMetrics, Font, FontFamilyKey, FontStretch, FontVariantKey, FontWeight,
    WebFontRegistry, load_default_text_fonts_shared, select_text_font,
};
use rusqlite::{Connection, params};

use super::{
    Combinator, CssToken, Declaration, MediaEnvironment, MediaQuery, MediaType, PseudoElement,
    Rule, Selector, SelectorPart, SimpleSelector, Specificity, Stylesheet, Value,
    evaluate_media_query_with_environment,
    is_css_wide_keyword_with_revert_rule as is_css_wide_keyword, parse_media_query_list,
    specificity,
};

/// CSS origin.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Origin {
    UserAgent,
    User,
    Author,
}

/// A property value after computation.
#[derive(Debug, Clone, PartialEq)]
pub enum ComputedValue {
    Keyword(String),
    Px(f32),
    Percentage(f32),
    Color(String),
    String(String),
    Number(f32),
    /// A typed `<length-percentage>` math expression whose percentage basis is
    /// not available until used-value resolution.
    LengthPercentage(LengthPercentageMath),
    /// A computed `text-indent` amount and its line-selection flags.
    TextIndent {
        /// The computed `<length-percentage>` amount.
        value: Box<Self>,
        /// Whether the normally affected lines are inverted.
        hanging: bool,
        /// Whether lines after forced breaks are normally affected.
        each_line: bool,
    },
    /// The two typed axes of a computed `<position>` value.
    Position {
        /// Horizontal component.
        x: Box<Self>,
        /// Vertical component.
        y: Box<Self>,
    },
    /// An owned computed value used by `border-image-*` and `mask-border-*`.
    NineSlice(NineSliceComputedValue),
}

/// One computed component of a nine-slice image property.
#[derive(Debug, Clone, PartialEq)]
pub enum NineSliceLength {
    /// A unitless multiplier or source-image slice coordinate.
    Number(f32),
    /// A percentage, where `100.0` represents the full reference dimension.
    Percentage(f32),
    /// A computed CSS-pixel length.
    Px(f32),
    /// A typed `<length-percentage>` calculation, resolved by the paint consumer.
    Math(LengthPercentageMath),
    /// The `auto` border-image width.
    Auto,
}

impl NineSliceLength {
    fn css_text(&self) -> String {
        match self {
            Self::Number(value) => value.to_string(),
            Self::Percentage(value) => format!("{value}%"),
            Self::Px(value) => format!("{value}px"),
            Self::Math(value) => value.css_text(),
            Self::Auto => "auto".to_string(),
        }
    }
}

/// Tiling behavior for one axis of a nine-slice image.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NineSliceRepeat {
    Stretch,
    Repeat,
    Round,
    Space,
}

impl NineSliceRepeat {
    fn css_text(self) -> &'static str {
        match self {
            Self::Stretch => "stretch",
            Self::Repeat => "repeat",
            Self::Round => "round",
            Self::Space => "space",
        }
    }
}

/// Computed, owned representation of a nine-slice image longhand.
#[derive(Debug, Clone, PartialEq)]
pub enum NineSliceComputedValue {
    Slice {
        values: Vec<NineSliceLength>,
        fill: bool,
    },
    Width(Vec<NineSliceLength>),
    Outset(Vec<NineSliceLength>),
    Repeat(Vec<NineSliceRepeat>),
}

impl NineSliceComputedValue {
    fn css_text(&self) -> String {
        match self {
            Self::Slice { values, fill } => {
                let mut parts = values
                    .iter()
                    .map(NineSliceLength::css_text)
                    .collect::<Vec<_>>();
                if *fill {
                    parts.push("fill".to_string());
                }
                parts.join(" ")
            }
            Self::Width(values) | Self::Outset(values) => values
                .iter()
                .map(NineSliceLength::css_text)
                .collect::<Vec<_>>()
                .join(" "),
            Self::Repeat(values) => values
                .iter()
                .copied()
                .map(NineSliceRepeat::css_text)
                .collect::<Vec<_>>()
                .join(" "),
        }
    }
}

impl ComputedValue {
    pub(crate) fn resolve_length_percentage(&self, basis: f32) -> Option<f32> {
        match self {
            Self::Px(value) => Some(*value),
            Self::Percentage(value) => Some(basis * value / 100.0),
            Self::LengthPercentage(value) => Some(value.resolve(basis)),
            Self::Number(value) if *value == 0.0 => Some(0.0),
            _ => None,
        }
    }

    pub(crate) fn linear_length_percentage_components(&self) -> Option<(f32, f32)> {
        match self {
            Self::Px(value) => Some((*value, 0.0)),
            Self::Percentage(value) => Some((0.0, *value)),
            Self::LengthPercentage(value) => value.linear_components(),
            _ => None,
        }
    }

    pub(crate) fn css_text(&self) -> String {
        match self {
            Self::Keyword(value) | Self::String(value) | Self::Color(value) => value.clone(),
            Self::Px(value) => format!("{value}px"),
            Self::Percentage(value) => format!("{value}%"),
            Self::Number(value) => value.to_string(),
            Self::LengthPercentage(value) => value.css_text(),
            Self::TextIndent {
                value,
                hanging,
                each_line,
            } => {
                let mut parts = vec![value.css_text()];
                if *hanging {
                    parts.push("hanging".to_string());
                }
                if *each_line {
                    parts.push("each-line".to_string());
                }
                parts.join(" ")
            }
            Self::Position { x, y } => format!("{} {}", x.css_text(), y.css_text()),
            Self::NineSlice(value) => value.css_text(),
        }
    }
}

/// A computed CSS `<length-percentage>` expression.
///
/// Relative and absolute lengths have already been converted to CSS pixels.
/// Percentages remain symbolic until the property supplies its percentage
/// basis during layout or paint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MathRoundingStrategy {
    /// Round to the closest multiple, resolving ties toward positive infinity.
    Nearest,
    /// Round toward positive infinity.
    Up,
    /// Round toward negative infinity.
    Down,
    /// Round toward zero.
    ToZero,
}

fn round_math_value(value: f32, step: f32, strategy: MathRoundingStrategy) -> f32 {
    if step == 0.0 {
        return f32::NAN;
    }
    let step = step.abs();
    if step.is_infinite() && value.is_finite() {
        if value == 0.0 {
            return value;
        }
        return match strategy {
            MathRoundingStrategy::Nearest | MathRoundingStrategy::ToZero => 0.0_f32.copysign(value),
            MathRoundingStrategy::Up if value > 0.0 => f32::INFINITY,
            MathRoundingStrategy::Up => -0.0,
            MathRoundingStrategy::Down if value < 0.0 => f32::NEG_INFINITY,
            MathRoundingStrategy::Down => -0.0,
        };
    }
    let quotient = value / step;
    let rounded = match strategy {
        MathRoundingStrategy::Nearest => (quotient + 0.5).floor(),
        MathRoundingStrategy::Up => quotient.ceil(),
        MathRoundingStrategy::Down => quotient.floor(),
        MathRoundingStrategy::ToZero => quotient.trunc(),
    };
    rounded * step
}

#[derive(Debug, Clone, PartialEq)]
pub enum LengthPercentageMath {
    /// An affine `px + percentage` value.
    Linear {
        /// The absolute component in CSS pixels.
        px: f32,
        /// The percentage component, where `100.0` means the full basis.
        percentage: f32,
    },
    /// A sum of compatible `<length-percentage>` expressions.
    Sum(Vec<Self>),
    /// An expression multiplied by a unitless number.
    Scale {
        /// The unitless multiplier.
        factor: f32,
        /// The typed expression being multiplied.
        value: Box<Self>,
    },
    /// The smallest argument after resolving every argument against one basis.
    Min(Vec<Self>),
    /// The largest argument after resolving every argument against one basis.
    Max(Vec<Self>),
    /// The absolute value after resolving its percentage basis.
    Abs(Box<Self>),
    /// The Euclidean norm after resolving all arguments against one basis.
    Hypot(Vec<Self>),
    /// A modulo or remainder operation with a shared percentage basis.
    Modulo {
        /// The value being divided.
        dividend: Box<Self>,
        /// The divisor.
        divisor: Box<Self>,
        /// True for `rem()`, false for `mod()`.
        remainder: bool,
    },
    /// Rounds a value to an integer multiple of a step.
    Round {
        /// The rounding direction.
        strategy: MathRoundingStrategy,
        /// The value to round.
        value: Box<Self>,
        /// The interval between adjacent rounded values.
        step: Box<Self>,
    },
    /// `max(minimum, min(preferred, maximum))`.
    Clamp {
        /// The lower bound.
        minimum: Box<Self>,
        /// The preferred value.
        preferred: Box<Self>,
        /// The upper bound.
        maximum: Box<Self>,
    },
}

impl LengthPercentageMath {
    /// Resolves this expression using `basis` as the value represented by
    /// `100%`.
    #[must_use]
    pub fn resolve(&self, basis: f32) -> f32 {
        let value = self.resolve_unclamped(basis);
        if value.is_nan() {
            0.0
        } else {
            value.clamp(-f32::MAX, f32::MAX)
        }
    }

    fn resolve_unclamped(&self, basis: f32) -> f32 {
        match self {
            Self::Linear { px, percentage } => px + basis * percentage / 100.0,
            Self::Sum(values) => values
                .iter()
                .map(|value| value.resolve_unclamped(basis))
                .sum(),
            Self::Scale { factor, value } => factor * value.resolve_unclamped(basis),
            Self::Min(values) => values
                .iter()
                .map(|value| value.resolve_unclamped(basis))
                .reduce(f32::min)
                .unwrap_or(0.0),
            Self::Max(values) => values
                .iter()
                .map(|value| value.resolve_unclamped(basis))
                .reduce(f32::max)
                .unwrap_or(0.0),
            Self::Abs(value) => value.resolve_unclamped(basis).abs(),
            Self::Hypot(values) => values.iter().fold(0.0, |norm, value| {
                norm.hypot(value.resolve_unclamped(basis))
            }),
            Self::Modulo {
                dividend,
                divisor,
                remainder,
            } => {
                let a = dividend.resolve_unclamped(basis);
                let b = divisor.resolve_unclamped(basis);
                if *remainder {
                    a % b
                } else if b == 0.0 || a.is_infinite() {
                    f32::NAN
                } else if b.is_infinite() {
                    if a == 0.0 || a.is_sign_negative() == b.is_sign_negative() {
                        a
                    } else {
                        f32::NAN
                    }
                } else {
                    let value = a % b;
                    if value == 0.0 {
                        0.0_f32.copysign(b)
                    } else if value.is_sign_negative() != b.is_sign_negative() {
                        value + b
                    } else {
                        value
                    }
                }
            }
            Self::Round {
                strategy,
                value,
                step,
            } => round_math_value(
                value.resolve_unclamped(basis),
                step.resolve_unclamped(basis),
                *strategy,
            ),
            Self::Clamp {
                minimum,
                preferred,
                maximum,
            } => preferred
                .resolve_unclamped(basis)
                .min(maximum.resolve_unclamped(basis))
                .max(minimum.resolve_unclamped(basis)),
        }
    }

    pub(crate) fn linear_components(&self) -> Option<(f32, f32)> {
        match self {
            Self::Linear { px, percentage } => Some((*px, *percentage)),
            Self::Sum(values) => values.iter().try_fold((0.0, 0.0), |total, value| {
                let value = value.linear_components()?;
                Some((total.0 + value.0, total.1 + value.1))
            }),
            Self::Scale { factor, value } => value
                .linear_components()
                .map(|(px, percentage)| (factor * px, factor * percentage)),
            Self::Min(_)
            | Self::Max(_)
            | Self::Clamp { .. }
            | Self::Abs(_)
            | Self::Hypot(_)
            | Self::Modulo { .. }
            | Self::Round { .. } => None,
        }
    }

    fn scaled(self, factor: f32) -> Self {
        if let Some((px, percentage)) = self.linear_components() {
            return Self::Linear {
                px: px * factor,
                percentage: percentage * factor,
            };
        }
        Self::Scale {
            factor,
            value: Box::new(self),
        }
    }

    fn add(self, other: Self) -> Self {
        if let (Some(left), Some(right)) = (self.linear_components(), other.linear_components()) {
            return Self::Linear {
                px: left.0 + right.0,
                percentage: left.1 + right.1,
            };
        }
        let mut values = match self {
            Self::Sum(values) => values,
            value => vec![value],
        };
        match other {
            Self::Sum(other) => values.extend(other),
            value => values.push(value),
        }
        Self::Sum(values)
    }

    fn is_pure_px(&self) -> bool {
        match self {
            Self::Linear { percentage, .. } => *percentage == 0.0,
            Self::Sum(values) | Self::Min(values) | Self::Max(values) | Self::Hypot(values) => {
                !values.is_empty() && values.iter().all(Self::is_pure_px)
            }
            Self::Scale { value, .. } => value.is_pure_px(),
            Self::Abs(value) => value.is_pure_px(),
            Self::Modulo {
                dividend, divisor, ..
            } => dividend.is_pure_px() && divisor.is_pure_px(),
            Self::Round { value, step, .. } => value.is_pure_px() && step.is_pure_px(),
            Self::Clamp {
                minimum,
                preferred,
                maximum,
            } => minimum.is_pure_px() && preferred.is_pure_px() && maximum.is_pure_px(),
        }
    }

    fn css_text(&self) -> String {
        match self {
            Self::Linear { px, percentage } if *px == 0.0 || *percentage == 0.0 => {
                self.expression_text()
            }
            Self::Min(_)
            | Self::Max(_)
            | Self::Clamp { .. }
            | Self::Abs(_)
            | Self::Hypot(_)
            | Self::Modulo { .. }
            | Self::Round { .. } => self.expression_text(),
            Self::Linear { .. } | Self::Sum(_) | Self::Scale { .. } => {
                format!("calc({})", self.expression_text())
            }
        }
    }

    /// Serializes this node as a calculation expression. Math-function
    /// arguments already provide a calculation context, so mixed linear terms
    /// must not gain a nested `calc()` wrapper there.
    fn expression_text(&self) -> String {
        match self {
            Self::Abs(value) => format!("abs({})", value.expression_text()),
            Self::Hypot(values) => format!(
                "hypot({})",
                values
                    .iter()
                    .map(Self::expression_text)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Self::Modulo {
                dividend,
                divisor,
                remainder,
            } => format!(
                "{}({}, {})",
                if *remainder { "rem" } else { "mod" },
                dividend.expression_text(),
                divisor.expression_text()
            ),
            Self::Round {
                strategy,
                value,
                step,
            } => {
                let strategy = match strategy {
                    MathRoundingStrategy::Nearest => "",
                    MathRoundingStrategy::Up => "up, ",
                    MathRoundingStrategy::Down => "down, ",
                    MathRoundingStrategy::ToZero => "to-zero, ",
                };
                format!(
                    "round({strategy}{}, {})",
                    value.expression_text(),
                    step.expression_text()
                )
            }
            Self::Linear { px, percentage } if *percentage == 0.0 => format!("{px}px"),
            Self::Linear { px, percentage } if *px == 0.0 => format!("{percentage}%"),
            Self::Linear { px, percentage } if *px < 0.0 => {
                format!("{percentage}% - {}px", px.abs())
            }
            Self::Linear { px, percentage } => format!("{percentage}% + {px}px"),
            Self::Sum(values) => values
                .iter()
                .map(Self::expression_text)
                .collect::<Vec<_>>()
                .join(" + "),
            Self::Scale { factor, value } => {
                let value = match value.as_ref() {
                    Self::Sum(_) => format!("calc({})", value.expression_text()),
                    _ => value.expression_text(),
                };
                format!("{value} * {factor}")
            }
            Self::Min(values) => format!(
                "min({})",
                values
                    .iter()
                    .map(Self::expression_text)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Self::Max(values) => format!(
                "max({})",
                values
                    .iter()
                    .map(Self::expression_text)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Self::Clamp {
                minimum,
                preferred,
                maximum,
            } => format!(
                "clamp({}, {}, {})",
                minimum.expression_text(),
                preferred.expression_text(),
                maximum.expression_text()
            ),
        }
    }
}

/// Paint data captured from a box that originates a text decoration.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PropagatedTextDecoration {
    pub(crate) origin: (usize, Option<PseudoElement>),
    pub(crate) line: String,
    pub(crate) color: String,
    pub(crate) style: String,
    pub(crate) skip_ink: String,
    pub(crate) thickness: ComputedValue,
    pub(crate) underline_position: String,
    pub(crate) underline_offset: ComputedValue,
    pub(crate) font_size: f32,
    pub(crate) font_family: Option<crate::font::FontFamilyKey>,
    pub(crate) font_weight: crate::font::FontWeight,
    pub(crate) font_style: crate::font::FontStyle,
    pub(crate) font_style_angle: i32,
    pub(crate) font_stretch: crate::font::FontStretch,
    pub(crate) font_scope_root: Option<usize>,
}

/// Resolved computed style for a node.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ComputedStyle {
    properties: PropertyMap,
    /// Resolved component values for properties whose used value cannot be
    /// recovered from their CSSOM serialization alone (for example generated
    /// content containing strings and counter functions).
    component_values: BTreeMap<String, Value>,
    custom_properties: BTreeMap<String, Value>,
    /// Tree scope captured by the declaration that supplied `animation-name`.
    animation_name_scope_root: Option<usize>,
    transform_animation_context: bool,
    /// Tree scope captured by the declaration that supplied `font-family`.
    font_family_scope_root: Option<usize>,
    /// OpenType settings validated and converted once at the style boundary.
    shaping_options: Arc<crate::font::ShapingOptions>,
    /// Font synthesis permissions validated once at the style boundary.
    font_synthesis: crate::font::FontSynthesis,
    /// CPAL palette selection resolved once at the style boundary.
    font_palette: Arc<crate::font::FontPalette>,
    /// Decorations propagated through the box tree, separate from inheritance.
    text_decorations: Arc<[PropagatedTextDecoration]>,
}

/// Owned values for the document canvas, with the root's adjustment policy.
pub(crate) struct ViewportPaintStyle {
    pub(crate) style: Arc<ComputedStyle>,
    pub(crate) canvas_color: Option<[u8; 3]>,
}

impl ComputedStyle {
    pub(crate) fn shaping_options(&self) -> &Arc<crate::font::ShapingOptions> {
        &self.shaping_options
    }

    pub(crate) fn font_palette(&self) -> &Arc<crate::font::FontPalette> {
        &self.font_palette
    }

    pub(crate) fn font_synthesis(&self) -> crate::font::FontSynthesis {
        self.font_synthesis
    }

    /// Animation-created stacking context, independent of the sampled values.
    pub(crate) fn has_transform_animation_context(&self) -> bool {
        self.transform_animation_context
    }

    /// Returns a computed property.
    pub fn get(&self, name: &str) -> Option<&ComputedValue> {
        self.properties.get(name)
    }

    /// Returns a computed longhand by its canonical property identity.
    pub(crate) fn get_by_id(&self, id: PropertyId) -> Option<&ComputedValue> {
        self.properties.get(&id)
    }

    /// Returns the number of computed properties without copying their values.
    pub(crate) fn property_count(&self) -> usize {
        self.properties.len()
    }

    /// Borrows computed property names in ascending order.
    pub(crate) fn property_names(&self) -> impl Iterator<Item = &str> {
        self.properties.keys()
    }

    /// Returns a snapshot of all computed properties, keyed by name.
    ///
    /// The map is built on each call; use [`Self::get`] to read a single
    /// property.
    pub fn properties(&self) -> BTreeMap<String, ComputedValue> {
        self.properties
            .iter()
            .map(|(name, value)| (name.to_string(), value.clone()))
            .collect()
    }

    /// Combines a separately cascaded visited style with the ordinary style
    /// for painting only. The ordinary style remains the sole CSSOM/layout
    /// style; a visited selector must not change geometry or reveal history
    /// through computed-style queries.
    pub(crate) fn with_visited_paint_colors(&self, visited: &Self) -> Self {
        const VISITED_COLOR_PROPERTIES: &[&str] = &[
            "color",
            "background-color",
            "border-color",
            "border-top-color",
            "border-right-color",
            "border-bottom-color",
            "border-left-color",
            "outline-color",
            "column-rule-color",
            "text-decoration-color",
        ];

        let mut paint = self.clone();
        for name in VISITED_COLOR_PROPERTIES {
            let system_name = visited
                .properties
                .system_color_name(name)
                .map(str::to_owned);
            let (Some(ordinary), Some(visited)) = (self.get(name), visited.get(name)) else {
                continue;
            };
            let (Some(ordinary), Some(visited)) = (
                crate::paint::color::parse_color(&ordinary.css_text()),
                crate::paint::color::parse_color(&visited.css_text()),
            ) else {
                continue;
            };
            // A transparent ordinary color cannot reveal whether a link was
            // visited. The visited declaration may change RGB, but not alpha.
            if ordinary.a == 0 {
                continue;
            }
            let value = format!(
                "#{:02x}{:02x}{:02x}{:02x}",
                visited.r, visited.g, visited.b, ordinary.a
            );
            paint.properties.insert(*name, ComputedValue::Color(value));
            if let Some(system_name) = system_name {
                paint.properties.mark_system_color(name, &system_name);
            }
        }
        paint
    }

    pub(crate) fn component_value(&self, name: &str) -> Option<&Value> {
        self.component_values.get(name)
    }

    /// Replaces a computed property with a layout-resolved CSS pixel value.
    pub(crate) fn set_resolved_px(&mut self, name: &str, value: f32) {
        self.properties.insert(name, ComputedValue::Px(value));
    }

    /// Serializes the computed transform using the element's actual reference box.
    pub(crate) fn set_resolved_transform(&mut self, reference: super::TransformReferenceBox) {
        let Some(ComputedValue::Keyword(value)) = self.get("transform") else {
            return;
        };
        if let Some(value) = super::serialize_transform_matrix(value, reference) {
            self.properties
                .insert(PropertyId::Transform, ComputedValue::Keyword(value));
        }
    }

    /// Resolves CSSOM transform to none when no rendered box exists.
    pub(crate) fn set_unrendered_transform(&mut self) {
        self.properties
            .insert(PropertyId::Transform, ComputedValue::Keyword("none".into()));
    }

    const LOGICAL_CSSOM_ALIASES: &[&str] = &[
        "inline-size",
        "block-size",
        "min-inline-size",
        "min-block-size",
        "max-inline-size",
        "max-block-size",
        "inset-inline-start",
        "inset-inline-end",
        "inset-block-start",
        "inset-block-end",
        "margin-inline-start",
        "margin-inline-end",
        "margin-block-start",
        "margin-block-end",
        "padding-inline-start",
        "padding-inline-end",
        "padding-block-start",
        "padding-block-end",
        "border-inline-start-width",
        "border-inline-end-width",
        "border-block-start-width",
        "border-block-end-width",
        "border-inline-start-style",
        "border-inline-end-style",
        "border-block-start-style",
        "border-block-end-style",
        "border-inline-start-color",
        "border-inline-end-color",
        "border-block-start-color",
        "border-block-end-color",
        "border-start-start-radius",
        "border-start-end-radius",
        "border-end-start-radius",
        "border-end-end-radius",
    ];

    /// Reports whether projecting this CSSOM longhand needs a logical alias.
    pub(crate) fn has_logical_cssom_alias(name: &str) -> bool {
        Self::LOGICAL_CSSOM_ALIASES.contains(&name)
    }

    /// Exposes logical aliases of the final physical values to computed CSSOM.
    /// This runs after layout has resolved used width and height.
    pub(crate) fn populate_logical_cssom(&mut self, flex_or_grid_item: bool) {
        let flow = logical_flow_from_properties(&self.properties);
        for &name in Self::LOGICAL_CSSOM_ALIASES {
            let Some(physical) = flow.physical_name(name) else {
                continue;
            };
            let mut value = self
                .properties
                .get(&physical)
                .cloned()
                .unwrap_or_else(|| {
                if name.starts_with("max-") {
                    ComputedValue::Keyword("none".to_string())
                } else if name.starts_with("min-") || name.ends_with("-size")
                    || name.starts_with("inset-")
                {
                    ComputedValue::Keyword("auto".to_string())
                } else if name.ends_with("-style") {
                    ComputedValue::Keyword("none".to_string())
                } else if name.ends_with("-color") {
                    self.properties.get(&PropertyId::Color).cloned()
                        .unwrap_or_else(|| ComputedValue::Color("black".to_string()))
                } else if name.ends_with("-width") {
                    let style = physical.replace("-width", "-style");
                    let visible = matches!(self.properties.get(&style),
                        Some(ComputedValue::Keyword(keyword)) if !matches!(keyword.as_str(), "none" | "hidden"));
                    ComputedValue::Px(if visible { 3.0 } else { 0.0 })
                } else {
                    ComputedValue::Px(0.0)
                }
            });
            if name.ends_with("-color") && value.css_text().eq_ignore_ascii_case("currentcolor") {
                value = self
                    .properties
                    .get(&PropertyId::Color)
                    .cloned()
                    .unwrap_or_else(|| ComputedValue::Color("black".to_string()));
            }
            if name.starts_with("min-")
                && !flex_or_grid_item
                && matches!(&value, ComputedValue::Keyword(keyword) if keyword == "auto")
            {
                value = ComputedValue::Px(0.0);
            }
            self.properties.insert(name, value);
        }
    }

    pub(crate) fn set_paint_value(&mut self, name: &str, value: String) {
        if name.starts_with("background-position-") {
            let computed = super::parse_style_attribute(&format!("{name}: {value}"))
                .into_iter()
                .find(|declaration| declaration.name.eq_ignore_ascii_case(name))
                .map(|declaration| {
                    compute_value(&declaration.value, name, ResolutionContext::default())
                });
            if let Some(computed @ ComputedValue::LengthPercentage(_)) = computed {
                self.properties.insert(name, computed);
                return;
            }
        }
        let trimmed = value.trim();
        let computed = if let Some(number) = trimmed.strip_suffix("px") {
            number
                .parse::<f32>()
                .ok()
                .map(ComputedValue::Px)
                .unwrap_or_else(|| ComputedValue::Keyword(value.clone()))
        } else if let Some(number) = trimmed.strip_suffix('%') {
            number
                .parse::<f32>()
                .ok()
                .map(ComputedValue::Percentage)
                .unwrap_or_else(|| ComputedValue::Keyword(value.clone()))
        } else if trimmed
            .parse::<f32>()
            .ok()
            .is_some_and(|number| number.is_finite() && number == 0.0)
        {
            ComputedValue::Px(0.0)
        } else {
            ComputedValue::Keyword(value)
        };
        self.properties.insert(name, computed);
    }

    pub(crate) fn font_family_scope_root(&self) -> Option<usize> {
        self.font_family_scope_root
    }

    pub(crate) fn text_decorations(&self) -> &Arc<[PropagatedTextDecoration]> {
        &self.text_decorations
    }
}

/// A stylesheet together with its cascade origin.
#[derive(Debug, Clone)]
pub struct StylesheetInput {
    pub origin: Origin,
    pub stylesheet: Stylesheet,
}

/// Context used when converting CSS values to computed px values.
#[derive(Debug, Clone, Copy)]
struct ResolutionContext {
    /// The parent element's computed font-size in px (used for `em` units).
    parent_font_size: f32,
    /// The root element's computed font-size in px (used for `rem` units).
    root_font_size: f32,
    /// Computed line-height for `lh` on the element being resolved.
    line_height: f32,
    /// Computed line-height of the root element for `rlh`.
    root_line_height: f32,
    /// Font-table and glyph measures for the font-relative length units.
    font_metrics: CssRelativeFontMetrics,
    /// Nearest eligible query-container dimension for each physical axis.
    container_size: [Option<f32>; 2],
    /// Whether the styled element's inline axis is vertical.
    vertical_writing: bool,
    /// Viewport width in px (used for `vw`, `vmin`, `vmax`).
    viewport_width: f32,
    /// Viewport height in px (used for `vh`, `vmin`, `vmax`).
    viewport_height: f32,
}

impl Default for ResolutionContext {
    fn default() -> Self {
        Self {
            parent_font_size: 16.0,
            root_font_size: 16.0,
            line_height: 19.2,
            root_line_height: 19.2,
            font_metrics: CssRelativeFontMetrics::fallback(16.0, false),
            viewport_width: 0.0,
            viewport_height: 0.0,
            container_size: [None, None],
            vertical_writing: false,
        }
    }
}

#[derive(Default)]
struct CssFontResources {
    web_fonts: Option<Arc<WebFontRegistry>>,
}

impl std::fmt::Debug for CssFontResources {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CssFontResources")
            .field("has_web_fonts", &self.web_fonts.is_some())
            .finish()
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
enum StyleAttributeSelectorDependency {
    #[default]
    None,
    SubjectOnly,
    NonLocal,
}

type AttributeSelectorDependency = StyleAttributeSelectorDependency;

/// Bounded result of transition cache processing before animation sampling.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum TransitionCacheOutcome {
    /// The transition clock did not invalidate computed-style caches.
    #[default]
    None,
    /// Running transition light-DOM subtrees were invalidated.
    Subtree,
    /// All ordinary and pseudo computed styles were invalidated.
    Full,
}

impl TransitionCacheOutcome {
    /// Returns the bounded label written to style-clock diagnostics.
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Subtree => "subtree",
            Self::Full => "full",
        }
    }
}

/// Cache entry counts from transition processing before animation invalidation.
/// A `None` outcome reports zeroes rather than the current cache sizes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct TransitionCacheEffects {
    /// Scope selected for transition cache invalidation.
    pub(crate) outcome: TransitionCacheOutcome,
    /// Ordinary entries removed by transition processing.
    pub(crate) ordinary_evicted: usize,
    /// Ordinary entries retained by transition processing.
    pub(crate) ordinary_retained: usize,
    /// Pseudo entries removed by transition processing.
    pub(crate) pseudo_evicted: usize,
    /// Pseudo entries retained by transition processing.
    pub(crate) pseudo_retained: usize,
}

impl TransitionCacheEffects {
    fn from_lengths(
        outcome: TransitionCacheOutcome,
        ordinary_before: usize,
        ordinary_after: usize,
        pseudo_before: usize,
        pseudo_after: usize,
    ) -> Self {
        Self {
            outcome,
            ordinary_evicted: ordinary_before.saturating_sub(ordinary_after),
            ordinary_retained: ordinary_after,
            pseudo_evicted: pseudo_before.saturating_sub(pseudo_after),
            pseudo_retained: pseudo_after,
        }
    }
}

/// Computes styles and caches results per node.
#[derive(Debug, Default)]
pub struct StyleResolver {
    stylesheets: Vec<StylesheetInput>,
    stylesheet_scopes: Vec<StylesheetScope>,
    stylesheet_ids: Vec<usize>,
    next_stylesheet_id: usize,
    layer_orders: HashMap<LayerContextKey, CascadeLayerOrder>,
    rule_indexes: Vec<StylesheetRuleIndex>,
    cache: style_cache::StyleCache<usize>,
    pseudo_cache: style_cache::StyleCache<(usize, PseudoElement)>,
    counter_values: HashMap<(usize, Option<PseudoElement>), HashMap<String, Vec<i32>>>,
    quote_values: HashMap<(usize, Option<PseudoElement>), Vec<String>>,
    selector_match_cache: SelectorMatchCache,
    /// Furthest reach of a registered selector that observes the raw `style` attribute.
    style_attribute_selector_dependency: StyleAttributeSelectorDependency,
    /// Lazily classified selector reach for ordinary HTML `data-*` attributes.
    ///
    /// The cache is dropped whenever a stylesheet is registered. A missing entry
    /// therefore means "not classified for the current stylesheet set", rather
    /// than "no selector observes the attribute".
    data_attribute_selector_dependencies: HashMap<String, AttributeSelectorDependency>,
    /// Bounded per-class selector reach, cleared when stylesheets change.
    class_selector_dependencies: HashMap<String, class_invalidation::ClassReach>,
    /// Bounded subjects that can observe changed hover/active/focus flags.
    user_action_dependencies: user_action::UserActionDependencies,
    /// Whether selectors can observe child-list changes outside the changed
    /// parents' subtrees, cleared when stylesheets change.
    tree_mutation_reach: Option<tree_mutation_invalidation::TreeMutationReach>,
    /// Whether declarations in the current stylesheet set read a given
    /// `data-*` attribute through `attr()`. This is separate from selector
    /// reach because an unobserved attribute can still affect computed values.
    data_attribute_value_dependencies: HashMap<String, bool>,
    visited_paint_state: Option<VisitedPaintState>,
    /// Root element's computed font-size in px (for `rem` unit resolution).
    root_font_size: f32,
    /// Root element's computed line-height in px (for `rlh`).
    root_line_height: f32,
    font_resources: CssFontResources,
    /// `true` when `root_font_size` was explicitly set via `set_root_font_size()`,
    /// preventing auto-update from the computed root element style.
    root_font_size_explicit: bool,
    /// Viewport width in px (for `vw`, `vmin`, `vmax` resolution).
    viewport_width: f32,
    /// Viewport height in px (for `vh`, `vmin`, `vmax` resolution).
    viewport_height: f32,
    /// `true` when the system is in dark mode (affects `prefers-color-scheme` evaluation).
    color_scheme_dark: bool,
    /// Output medium used for `@media` evaluation.
    media_type: MediaType,
    media_environment: MediaEnvironment,
    /// Cache of parsed media query lists keyed by the normalized (trimmed) prelude string.
    ///
    /// Avoids re-parsing the same `@media` prelude string for every node that
    /// is matched against the stylesheet.  The cache is intentionally separate
    /// from the per-node `cache` so it survives `cache.clear()` calls (e.g.
    /// after `set_color_scheme_dark`).  The parsed `Vec<MediaQuery>` is stable:
    /// it depends only on the prelude text, not on viewport dimensions or
    /// color-scheme settings.
    media_query_cache: HashMap<String, Vec<MediaQuery>>,
    /// Parsed `@scope` preludes, including invalid results, keyed by source text.
    scope_prelude_cache: HashMap<String, Option<super::ScopePrelude>>,
    /// Parsed `@container` preludes, including invalid results, keyed by source text.
    container_query_cache: HashMap<String, Option<super::ContainerQuery>>,
    /// Query-container geometry and metadata from the previous layout pass.
    container_contexts: HashMap<usize, ContainerContext>,
    /// Dependency queries reuse a result until either computed-style cache changes.
    container_context_memo: style_cache::ContainerContextMemo,
    #[cfg(test)]
    container_context_scans: std::cell::Cell<usize>,
    /// Winning `@keyframes` rules, grouped by their tree scope and name.
    keyframes: HashMap<Option<usize>, HashMap<String, KeyframesDefinition>>,
    /// Monotonic order assigned to parsed `@keyframes` definitions.
    next_keyframes_source_order: usize,
    /// Winning `@font-face` rules, grouped by tree scope and supported variant.
    font_faces: HashMap<Option<usize>, HashMap<FontFaceKey, FontFaceDefinition>>,
    /// Every active `@font-face` rule in stable source order for CSS FontFaceSet.
    active_font_faces: Vec<(Option<usize>, super::FontFaceRule)>,
    /// Winning document-scoped `@counter-style` definitions.
    counter_styles: HashMap<Option<usize>, HashMap<String, CounterStyleDefinition>>,
    /// Effective document-scoped custom-property registrations. Stylesheet
    /// registrations are collected in document order and script registrations
    /// installed through `CSS.registerProperty()` take precedence.
    registered_custom_properties: BTreeMap<String, RegisteredCustomProperty>,
    script_registered_custom_properties: BTreeMap<String, RegisteredCustomProperty>,
    /// Monotonic order assigned to parsed `@font-face` definitions.
    next_font_face_source_order: usize,
    /// Before/after style snapshots and running CSS transitions.
    transition_timeline: super::transition::TransitionTimeline,
    animation_timeline: Option<RefCell<animation::AnimationTimeline>>,
    /// Node identities whose inline `style` attribute is blocked by the
    /// owning Document's CSP `style-src` policy.
    blocked_inline_style_nodes: HashSet<usize>,
}

#[derive(Debug)]
struct VisitedPaintState {
    selector_match_cache: SelectorMatchCache,
    styles: HashMap<usize, Arc<ComputedStyle>>,
    pseudo_styles: HashMap<(usize, PseudoElement), Arc<ComputedStyle>>,
    affected_nodes: HashSet<usize>,
}

/// Isolated color cascade for one paint pass. This never writes to the
/// resolver's public computed-style caches, so CSSOM cannot observe history.
pub(crate) struct VisitedPaintStylePass<'a> {
    resolver: &'a mut StyleResolver,
    selector_match_cache: SelectorMatchCache,
    styles: HashMap<usize, Arc<ComputedStyle>>,
    pseudo_styles: HashMap<(usize, PseudoElement), Arc<ComputedStyle>>,
    affected_nodes: HashSet<usize>,
}

impl<'a> VisitedPaintStylePass<'a> {
    pub(crate) fn new(
        resolver: &'a mut StyleResolver,
        visited_link_ids: impl IntoIterator<Item = usize>,
    ) -> Self {
        Self::from_state(
            resolver,
            VisitedPaintState {
                selector_match_cache: SelectorMatchCache::for_visited_links(visited_link_ids),
                styles: HashMap::new(),
                pseudo_styles: HashMap::new(),
                affected_nodes: HashSet::new(),
            },
        )
    }

    fn from_state(resolver: &'a mut StyleResolver, state: VisitedPaintState) -> Self {
        Self {
            resolver,
            selector_match_cache: state.selector_match_cache,
            styles: state.styles,
            pseudo_styles: state.pseudo_styles,
            affected_nodes: state.affected_nodes,
        }
    }

    fn into_state(self) -> VisitedPaintState {
        VisitedPaintState {
            selector_match_cache: self.selector_match_cache,
            styles: self.styles,
            pseudo_styles: self.pseudo_styles,
            affected_nodes: self.affected_nodes,
        }
    }

    pub(crate) fn style(&mut self, node: &NodeHandle) -> Arc<ComputedStyle> {
        let key = node.identity();
        if let Some(style) = self.styles.get(&key) {
            return style.clone();
        }

        let ordinary = self.resolver.computed_style_shared(node);
        let inheritance_parent = flattened_assigned_slot(node).or_else(|| node.parent_node());
        let inherited_identity = inheritance_parent.as_ref().map(|parent| {
            if parent.node_type() == NodeType::DocumentFragment {
                parent
                    .shadow_host()
                    .unwrap_or_else(|| parent.clone())
                    .identity()
            } else {
                parent.identity()
            }
        });
        let inherited = inheritance_parent.map(|parent| {
            if parent.node_type() == NodeType::DocumentFragment {
                parent
                    .shadow_host()
                    .map(|host| self.style(&host))
                    .unwrap_or_default()
            } else {
                self.style(&parent)
            }
        });
        if !self.selector_match_cache.contains_visited_link_id(key)
            && !inherited_identity.is_some_and(|id| self.affected_nodes.contains(&id))
        {
            self.styles.insert(key, ordinary.clone());
            return ordinary;
        }
        self.affected_nodes.insert(key);
        let visited = self.cascade(node, inherited.as_deref(), None, &ordinary);
        let paint = Arc::new(ordinary.with_visited_paint_colors(&visited));
        self.styles.insert(key, paint.clone());
        paint
    }

    pub(crate) fn pseudo_style(
        &mut self,
        node: &NodeHandle,
        pseudo: PseudoElement,
    ) -> Option<Arc<ComputedStyle>> {
        let key = (node.identity(), pseudo);
        if let Some(style) = self.pseudo_styles.get(&key) {
            return Some(style.clone());
        }
        let ordinary = self.resolver.computed_pseudo_style_shared(node, pseudo)?;
        let parent = self.style(node);
        if !self.affected_nodes.contains(&node.identity()) {
            return Some(ordinary);
        }
        let visited = self.cascade(node, Some(&parent), Some(pseudo), &ordinary);
        let paint = Arc::new(ordinary.with_visited_paint_colors(&visited));
        self.pseudo_styles.insert(key, paint.clone());
        Some(paint)
    }

    fn cascade(
        &mut self,
        node: &NodeHandle,
        parent: Option<&ComputedStyle>,
        pseudo: Option<PseudoElement>,
        ordinary: &ComputedStyle,
    ) -> ComputedStyle {
        let ordinary_cache = std::mem::replace(
            &mut self.resolver.selector_match_cache,
            std::mem::take(&mut self.selector_match_cache),
        );
        let visited = self.resolver.compute_style_with_pseudo(
            node,
            parent,
            pseudo,
            Some(&ordinary.custom_properties),
        );
        self.selector_match_cache =
            std::mem::replace(&mut self.resolver.selector_match_cache, ordinary_cache);
        visited
    }
}

/// A validated custom-property registration shared by stylesheet and script
/// registration paths.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RegisteredCustomProperty {
    pub(crate) name: String,
    pub(crate) syntax_text: String,
    syntax: RegisteredPropertySyntax,
    pub(crate) inherits: bool,
    pub(crate) initial_value: Option<Value>,
}

#[derive(Debug, Clone, PartialEq)]
enum RegisteredPropertySyntax {
    Universal,
    Alternatives(Vec<RegisteredSyntaxComponent>),
}

#[derive(Debug, Clone, PartialEq)]
struct RegisteredSyntaxComponent {
    kind: RegisteredSyntaxKind,
    multiplier: RegisteredSyntaxMultiplier,
}

#[derive(Debug, Clone, PartialEq)]
enum RegisteredSyntaxKind {
    Length,
    LengthPercentage,
    Number,
    Integer,
    Percentage,
    Color,
    Angle,
    Time,
    Resolution,
    TransformFunction,
    TransformList,
    String,
    Image,
    Url,
    CustomIdent,
    Literal(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RegisteredSyntaxMultiplier {
    Single,
    SpaceList,
    CommaList,
}

/// Validates a registration supplied by `CSS.registerProperty()`.
pub(crate) fn parse_registered_custom_property(
    name: &str,
    syntax: &str,
    inherits: bool,
    initial_value: Option<&str>,
) -> Option<RegisteredCustomProperty> {
    if !is_custom_property_registration_name(name) {
        return None;
    }
    let syntax = parse_registered_property_syntax(syntax)?;
    let initial_value = match initial_value {
        Some(value) => Some(parse_registered_initial_value(value)?),
        None => None,
    };
    if !matches!(syntax, RegisteredPropertySyntax::Universal) && initial_value.is_none() {
        return None;
    }
    if let Some(value) = initial_value.as_ref()
        && (!registered_value_matches_syntax(value, &syntax)
            || !registered_initial_value_is_independent(value))
    {
        return None;
    }
    Some(RegisteredCustomProperty {
        name: name.to_string(),
        syntax_text: syntax_text(syntax.clone()),
        syntax,
        inherits,
        initial_value,
    })
}

/// Parses a stylesheet `@property` rule. Invalid rules have no registration
/// effect and are omitted from CSSOM by the native rule-source filter.
pub(crate) fn registered_custom_property_from_rule(
    rule: &super::AtRule,
) -> Option<RegisteredCustomProperty> {
    if !rule.name.eq_ignore_ascii_case("property") {
        return None;
    }
    let name = rule.prelude.trim();
    if !is_custom_property_registration_name(name) {
        return None;
    }
    let descriptor = |expected: &str| {
        rule.declarations
            .iter()
            .rev()
            .find(|declaration| declaration.name.eq_ignore_ascii_case(expected))
            .map(|declaration| &declaration.value)
    };
    let syntax_value = descriptor("syntax")?;
    let syntax_text = match syntax_value {
        Value::String(value) | Value::Keyword(value) => value.trim(),
        _ => return None,
    };
    let inherits = match descriptor("inherits")? {
        Value::Keyword(value) if value.eq_ignore_ascii_case("true") => true,
        Value::Keyword(value) if value.eq_ignore_ascii_case("false") => false,
        _ => return None,
    };
    let parsed_syntax = parse_registered_property_syntax(syntax_text)?;
    let initial_value = descriptor("initial-value").cloned();
    if !matches!(parsed_syntax, RegisteredPropertySyntax::Universal) && initial_value.is_none() {
        return None;
    }
    if let Some(value) = initial_value.as_ref()
        && (!registered_value_matches_syntax(value, &parsed_syntax)
            || !registered_initial_value_is_independent(value))
    {
        return None;
    }
    Some(RegisteredCustomProperty {
        name: name.to_string(),
        syntax_text: match syntax_value {
            Value::String(value) | Value::Keyword(value) => value.clone(),
            _ => unreachable!("syntax descriptor was matched above"),
        },
        syntax: parsed_syntax,
        inherits,
        initial_value,
    })
}

fn is_custom_property_registration_name(name: &str) -> bool {
    name.starts_with("--") && name.len() > 2 && !name.contains('\0')
}

fn parse_registered_initial_value(input: &str) -> Option<Value> {
    if input.trim().is_empty() || contains_top_level_semicolon(input) {
        return None;
    }
    super::parse_style_attribute(&format!("--omoikane-registration-value: {input}"))
        .into_iter()
        .find(|declaration| declaration.name == "--omoikane-registration-value")
        .map(|declaration| declaration.value)
}

fn parse_registered_property_syntax(input: &str) -> Option<RegisteredPropertySyntax> {
    use super::CssToken;
    if input.trim() == "*" {
        return Some(RegisteredPropertySyntax::Universal);
    }
    let tokens = super::tokenizer::tokenize(input).ok()?;
    let mut alternatives = Vec::new();
    for part in tokens.split(|token| matches!(token, CssToken::Delim('|'))) {
        let start = part
            .iter()
            .position(|token| !matches!(token, CssToken::Whitespace))?;
        let end = part
            .iter()
            .rposition(|token| !matches!(token, CssToken::Whitespace))?;
        let raw = &part[start..=end];
        let (core, multiplier) = match raw.last() {
            Some(CssToken::Delim('+')) => {
                (&raw[..raw.len() - 1], RegisteredSyntaxMultiplier::SpaceList)
            }
            Some(CssToken::Hash(value)) if value.is_empty() => {
                (&raw[..raw.len() - 1], RegisteredSyntaxMultiplier::CommaList)
            }
            _ => (raw, RegisteredSyntaxMultiplier::Single),
        };
        let kind = if let [
            CssToken::Delim('<'),
            CssToken::Ident(name),
            CssToken::Delim('>'),
        ] = core
        {
            match name.as_str() {
                "length" => RegisteredSyntaxKind::Length,
                "length-percentage" => RegisteredSyntaxKind::LengthPercentage,
                "number" => RegisteredSyntaxKind::Number,
                "integer" => RegisteredSyntaxKind::Integer,
                "percentage" => RegisteredSyntaxKind::Percentage,
                "color" => RegisteredSyntaxKind::Color,
                "angle" => RegisteredSyntaxKind::Angle,
                "time" => RegisteredSyntaxKind::Time,
                "resolution" => RegisteredSyntaxKind::Resolution,
                "transform-function" => RegisteredSyntaxKind::TransformFunction,
                "transform-list" if multiplier == RegisteredSyntaxMultiplier::Single => {
                    RegisteredSyntaxKind::TransformList
                }
                "string" => RegisteredSyntaxKind::String,
                "image" => RegisteredSyntaxKind::Image,
                "url" => RegisteredSyntaxKind::Url,
                "custom-ident" => RegisteredSyntaxKind::CustomIdent,
                _ => return None,
            }
        } else if let [CssToken::Ident(name)] = core {
            if name.is_empty()
                || is_css_wide_keyword(&name.to_ascii_lowercase())
                || name.eq_ignore_ascii_case("default")
            {
                return None;
            }
            RegisteredSyntaxKind::Literal(name.clone())
        } else {
            return None;
        };
        alternatives.push(RegisteredSyntaxComponent { kind, multiplier });
    }
    (!alternatives.is_empty()).then_some(RegisteredPropertySyntax::Alternatives(alternatives))
}

fn serialize_registered_identifier(value: &str) -> String {
    let mut result = String::new();
    for (index, character) in value.chars().enumerate() {
        if character == '\0' {
            result.push('\u{fffd}');
        } else if character.is_ascii_control()
            || (character.is_ascii_digit()
                && (index == 0 || (index == 1 && value.starts_with('-'))))
        {
            result.push_str(&format!("\\{:x} ", character as u32));
        } else if character.is_ascii_alphanumeric()
            || character == '_'
            || character == '-'
            || !character.is_ascii()
        {
            if value == "-" {
                result.push('\\');
            }
            result.push(character);
        } else {
            result.push('\\');
            result.push(character);
        }
    }
    result
}

impl RegisteredCustomProperty {
    /// Serializes the initial descriptor, retaining escapes on literal identifiers.
    pub(crate) fn initial_value_css_text(&self) -> Option<String> {
        self.initial_value.as_ref().map(|value| {
            if let Value::Keyword(keyword) = value
                && let RegisteredPropertySyntax::Alternatives(parts) = &self.syntax
                && parts.iter().any(|part| matches!(&part.kind, RegisteredSyntaxKind::Literal(name) if name == keyword))
            {
                serialize_registered_identifier(keyword)
            } else {
                super::serialize_specified_value(value)
            }
        })
    }
}

fn valid_registered_literal(value: &str) -> bool {
    !value.is_empty()
        && !value.chars().any(char::is_whitespace)
        && !value.contains([',', '<', '>', '+', '#'])
        && !matches!(
            value.to_ascii_lowercase().as_str(),
            "initial" | "inherit" | "unset" | "revert" | "revert-layer" | "revert-rule" | "default"
        )
}

fn syntax_text(syntax: RegisteredPropertySyntax) -> String {
    match syntax {
        RegisteredPropertySyntax::Universal => "*".to_string(),
        RegisteredPropertySyntax::Alternatives(parts) => parts
            .into_iter()
            .map(|part| {
                let core = match part.kind {
                    RegisteredSyntaxKind::Length => "<length>".to_string(),
                    RegisteredSyntaxKind::LengthPercentage => "<length-percentage>".to_string(),
                    RegisteredSyntaxKind::Number => "<number>".to_string(),
                    RegisteredSyntaxKind::Integer => "<integer>".to_string(),
                    RegisteredSyntaxKind::Percentage => "<percentage>".to_string(),
                    RegisteredSyntaxKind::Color => "<color>".to_string(),
                    RegisteredSyntaxKind::Angle => "<angle>".to_string(),
                    RegisteredSyntaxKind::Time => "<time>".to_string(),
                    RegisteredSyntaxKind::Resolution => "<resolution>".to_string(),
                    RegisteredSyntaxKind::TransformFunction => "<transform-function>".to_string(),
                    RegisteredSyntaxKind::TransformList => "<transform-list>".to_string(),
                    RegisteredSyntaxKind::String => "<string>".to_string(),
                    RegisteredSyntaxKind::Image => "<image>".to_string(),
                    RegisteredSyntaxKind::Url => "<url>".to_string(),
                    RegisteredSyntaxKind::CustomIdent => "<custom-ident>".to_string(),
                    RegisteredSyntaxKind::Literal(value) => serialize_registered_identifier(&value),
                };
                match part.multiplier {
                    RegisteredSyntaxMultiplier::Single => core,
                    RegisteredSyntaxMultiplier::SpaceList => format!("{core}+"),
                    RegisteredSyntaxMultiplier::CommaList => format!("{core}#"),
                }
            })
            .collect::<Vec<_>>()
            .join(" | "),
    }
}

fn registered_value_matches_syntax(value: &Value, syntax: &RegisteredPropertySyntax) -> bool {
    if matches!(value, Value::Keyword(keyword) if is_css_wide_keyword(&keyword.to_ascii_lowercase()))
    {
        return false;
    }
    match syntax {
        RegisteredPropertySyntax::Universal => true,
        RegisteredPropertySyntax::Alternatives(alternatives) => alternatives
            .iter()
            .any(|component| registered_value_matches_component(value, component)),
    }
}

fn registered_value_matches_component(
    value: &Value,
    component: &RegisteredSyntaxComponent,
) -> bool {
    match component.multiplier {
        RegisteredSyntaxMultiplier::Single => registered_value_matches_kind(value, &component.kind),
        RegisteredSyntaxMultiplier::SpaceList => match value {
            Value::List(values) => {
                !values.is_empty()
                    && values
                        .iter()
                        .all(|value| registered_value_matches_kind(value, &component.kind))
            }
            value => registered_value_matches_kind(value, &component.kind),
        },
        RegisteredSyntaxMultiplier::CommaList => match value {
            Value::CommaList(values) => {
                !values.is_empty()
                    && values
                        .iter()
                        .all(|value| registered_value_matches_kind(value, &component.kind))
            }
            value => registered_value_matches_kind(value, &component.kind),
        },
    }
}

fn registered_value_matches_kind(value: &Value, kind: &RegisteredSyntaxKind) -> bool {
    match kind {
        RegisteredSyntaxKind::Length => match value {
            Value::Length(_, unit) => is_css_length_unit(unit),
            Value::Number(value) => *value == 0.0,
            Value::Function { name, .. } if is_length_percentage_math_function(name) => {
                matches!(
                    compute_value(value, "--registered", ResolutionContext::default()),
                    ComputedValue::Px(_)
                )
            }
            _ => false,
        },
        RegisteredSyntaxKind::LengthPercentage => match value {
            Value::Length(_, unit) => is_css_length_unit(unit),
            Value::Number(value) => *value == 0.0,
            Value::Percentage(_) => true,
            Value::Function { name, .. } if is_length_percentage_math_function(name) => matches!(
                compute_value(value, "--registered", ResolutionContext::default()),
                ComputedValue::Px(_)
                    | ComputedValue::Percentage(_)
                    | ComputedValue::LengthPercentage(_)
            ),
            _ => false,
        },
        RegisteredSyntaxKind::Number => match value {
            Value::Number(_) => true,
            Value::Function { .. } => {
                typed_math::infer(value, None) == Some(typed_math::NumericType::NUMBER)
            }
            _ => false,
        },
        RegisteredSyntaxKind::Integer => match value {
            Value::Number(value) => value.fract() == 0.0,
            Value::Function { .. } => {
                typed_math::infer(value, None) == Some(typed_math::NumericType::NUMBER)
            }
            _ => false,
        },
        RegisteredSyntaxKind::Percentage => match value {
            Value::Percentage(_) => true,
            Value::Function { .. } => {
                typed_math::infer(value, None) == Some(typed_math::NumericType::PERCENTAGE)
            }
            _ => false,
        },
        RegisteredSyntaxKind::Color => is_valid_css_color_text(&render_value(value)),
        RegisteredSyntaxKind::Angle => {
            typed_math::infer(value, None) == Some(typed_math::NumericType::ANGLE)
        }
        RegisteredSyntaxKind::Time => {
            typed_math::infer(value, None) == Some(typed_math::NumericType::TIME)
        }
        RegisteredSyntaxKind::Resolution => {
            typed_math::infer(value, None) == Some(typed_math::NumericType::RESOLUTION)
                && !matches!(value, Value::Length(number, _) if *number < 0.0)
        }
        RegisteredSyntaxKind::TransformFunction => matches!(value, Value::Function { .. }),
        RegisteredSyntaxKind::TransformList => match value {
            Value::Function { .. } => true,
            Value::List(values) => {
                !values.is_empty()
                    && values
                        .iter()
                        .all(|value| matches!(value, Value::Function { .. }))
            }
            _ => false,
        },
        RegisteredSyntaxKind::String => matches!(value, Value::String(_)),
        RegisteredSyntaxKind::Image => {
            matches!(value, Value::Function { name, .. } if name.eq_ignore_ascii_case("url") || name.to_ascii_lowercase().ends_with("gradient"))
                || matches!(value, Value::Keyword(keyword) if keyword.to_ascii_lowercase().starts_with("url("))
        }
        RegisteredSyntaxKind::Url => {
            matches!(value, Value::Function { name, .. } if name.eq_ignore_ascii_case("url"))
                || matches!(value, Value::Keyword(keyword) if keyword.to_ascii_lowercase().starts_with("url("))
        }
        RegisteredSyntaxKind::CustomIdent => {
            matches!(value, Value::Keyword(keyword) if valid_registered_literal(keyword))
        }
        RegisteredSyntaxKind::Literal(expected) => {
            matches!(value, Value::Keyword(keyword) if keyword == expected)
        }
    }
}

fn is_css_length_unit(unit: &str) -> bool {
    matches!(
        unit.to_ascii_lowercase().as_str(),
        "px" | "cm"
            | "mm"
            | "q"
            | "in"
            | "pt"
            | "pc"
            | "em"
            | "rem"
            | "ex"
            | "ch"
            | "cap"
            | "ic"
            | "lh"
            | "rlh"
            | "vw"
            | "vh"
            | "vmin"
            | "vmax"
            | "svw"
            | "svh"
            | "lvw"
            | "lvh"
            | "dvw"
            | "dvh"
            | "cqw"
            | "cqh"
            | "cqi"
            | "cqb"
            | "cqmin"
            | "cqmax"
    )
}

fn registered_initial_value_is_independent(value: &Value) -> bool {
    if value_contains_var_function(value) {
        return false;
    }
    match value {
        Value::Length(_, unit) => !matches!(
            unit.to_ascii_lowercase().as_str(),
            "em" | "rem" | "ex" | "ch" | "cap" | "ic" | "lh" | "rlh"
        ),
        Value::Function { arguments, .. }
        | Value::List(arguments)
        | Value::CommaList(arguments) => arguments
            .iter()
            .all(registered_initial_value_is_independent),
        _ => true,
    }
}

#[derive(Debug, Clone)]
struct KeyframeStep {
    offset: f32,
    declarations: Vec<Declaration>,
}

#[derive(Debug, Clone)]
struct KeyframesDefinition {
    origin: Origin,
    layer_order: Vec<usize>,
    source_order: usize,
    steps: Vec<KeyframeStep>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct FontFaceKey {
    family: String,
    descriptors: crate::font::WebFontDescriptors,
}

#[derive(Debug, Clone)]
struct FontFaceDefinition {
    origin: Origin,
    layer_order: Vec<usize>,
    source_order: usize,
    rule: super::FontFaceRule,
}

#[derive(Debug, Clone)]
struct CounterStyleDefinition {
    origin: Origin,
    layer_order: Vec<usize>,
    source_order: usize,
    system: CounterStyleSystem,
    symbols: Vec<String>,
    prefix: String,
    suffix: String,
}

#[derive(Debug, Clone)]
enum CounterStyleSystem {
    Cyclic,
    Extends(String),
}

#[derive(Debug, Clone)]
struct StylesheetScope {
    root: Option<NodeHandle>,
    implicit_scope_root: Option<NodeHandle>,
    encapsulation_order: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct LayerContextKey {
    origin: Origin,
    scope_root: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum LayerSegment {
    Named(String),
    Anonymous {
        stylesheet_id: usize,
        parser_id: usize,
    },
}

type LayerPath = Vec<LayerSegment>;

#[derive(Debug, Default)]
struct CascadeLayerOrder {
    children: HashMap<LayerPath, Vec<LayerSegment>>,
}

impl CascadeLayerOrder {
    fn register_stylesheet(
        &mut self,
        rules: &[Rule],
        stylesheet_id: usize,
        viewport_width: f32,
        viewport_height: f32,
        color_scheme_dark: bool,
        media_type: MediaType,
        media_environment: &MediaEnvironment,
    ) {
        self.register_rules(
            rules,
            stylesheet_id,
            &[],
            viewport_width,
            viewport_height,
            color_scheme_dark,
            media_type,
            media_environment,
        );
    }

    fn register_rules(
        &mut self,
        rules: &[Rule],
        stylesheet_id: usize,
        parent: &[LayerSegment],
        viewport_width: f32,
        viewport_height: f32,
        color_scheme_dark: bool,
        media_type: MediaType,
        media_environment: &MediaEnvironment,
    ) {
        for rule in rules {
            let Rule::At(at_rule) = rule else {
                continue;
            };
            if at_rule.block.is_some()
                && !layer_group_rule_is_active(
                    at_rule,
                    viewport_width,
                    viewport_height,
                    media_environment,
                )
            {
                continue;
            }
            if at_rule.name.eq_ignore_ascii_case("layer") {
                if let Some(block) = at_rule.block.as_deref() {
                    let Some(path) = layer_block_path(at_rule, stylesheet_id, parent) else {
                        continue;
                    };
                    self.register_path(&path);
                    self.register_rules(
                        block,
                        stylesheet_id,
                        &path,
                        viewport_width,
                        viewport_height,
                        color_scheme_dark,
                        media_type,
                        media_environment,
                    );
                } else if let Some(names) = super::parse_layer_name_list(&at_rule.prelude) {
                    for name in names {
                        let mut path = parent.to_vec();
                        path.extend(name.into_iter().map(LayerSegment::Named));
                        self.register_path(&path);
                    }
                }
            } else if let Some(block) = at_rule.block.as_deref() {
                self.register_rules(
                    block,
                    stylesheet_id,
                    parent,
                    viewport_width,
                    viewport_height,
                    color_scheme_dark,
                    media_type,
                    media_environment,
                );
            }
        }
    }

    fn register_path(&mut self, path: &[LayerSegment]) {
        let mut parent = Vec::new();
        for segment in path {
            let siblings = self.children.entry(parent.clone()).or_default();
            if !siblings.contains(segment) {
                siblings.push(segment.clone());
            }
            parent.push(segment.clone());
        }
    }

    fn rank(&self, path: Option<&LayerPath>) -> Vec<usize> {
        let Some(path) = path else {
            return vec![usize::MAX];
        };
        let mut parent = Vec::new();
        let mut rank = Vec::with_capacity(path.len() + 1);
        for segment in path {
            let position = self
                .children
                .get(&parent)
                .and_then(|siblings| siblings.iter().position(|candidate| candidate == segment))
                .unwrap_or(usize::MAX - 1);
            rank.push(position);
            parent.push(segment.clone());
        }
        // Rules directly in a layer form an implicit final sublayer after all
        // explicit children of that layer.
        rank.push(usize::MAX);
        rank
    }
}

fn layer_group_rule_is_active(
    at_rule: &super::AtRule,
    viewport_width: f32,
    viewport_height: f32,
    media_environment: &MediaEnvironment,
) -> bool {
    if at_rule.name.eq_ignore_ascii_case("media") {
        return parse_media_query_list(&at_rule.prelude)
            .unwrap_or_default()
            .iter()
            .any(|query| {
                evaluate_media_query_with_environment(
                    query,
                    viewport_width,
                    viewport_height,
                    media_environment,
                )
            });
    }
    if at_rule.name.eq_ignore_ascii_case("supports") {
        return super::supports_condition_matches(&at_rule.prelude);
    }
    true
}

fn layer_block_path(
    at_rule: &super::AtRule,
    stylesheet_id: usize,
    parent: &[LayerSegment],
) -> Option<LayerPath> {
    if at_rule.prelude.trim().is_empty() {
        let parser_id = at_rule.anonymous_layer_id?;
        let mut path = parent.to_vec();
        path.push(LayerSegment::Anonymous {
            stylesheet_id,
            parser_id,
        });
        return Some(path);
    }

    let mut names = super::parse_layer_name_list(&at_rule.prelude)?;
    if names.len() != 1 {
        return None;
    }
    let mut path = parent.to_vec();
    path.extend(names.pop()?.into_iter().map(LayerSegment::Named));
    Some(path)
}

/// Geometry and computed containment properties captured after a layout pass.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ContainerContext {
    pub width: f32,
    pub height: f32,
    pub container_type: String,
    pub eligible: bool,
    pub vertical_writing: bool,
    pub names: Vec<String>,
    pub units: super::ContainerUnitContext,
    pub style: ContainerStyleSnapshot,
}

/// Owned custom values and their registered syntax captured for a query container.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ContainerStyleSnapshot {
    values: BTreeMap<String, Value>,
    syntax: BTreeMap<String, RegisteredPropertySyntax>,
}

impl ContainerStyleSnapshot {
    /// Compares custom-property values using explicit container units and query scope.
    pub(crate) fn matches(
        &self,
        name: &str,
        value: &Value,
        units: super::ContainerUnitContext,
        node: &NodeHandle,
        scope: Option<&NodeHandle>,
    ) -> bool {
        let Some(actual) = self.values.get(name) else {
            return false;
        };
        let value = sibling_functions::annotate(value, &sibling_functions::scope_ancestry(scope));
        let Some(value) = resolve_value_with_custom_properties(&value, &self.values) else {
            return false;
        };
        let Some(syntax) = self.syntax.get(name) else {
            return render_value(&value) == render_value(actual);
        };
        if matches!(syntax, RegisteredPropertySyntax::Universal) {
            return render_value(&value) == render_value(actual);
        }
        let value = sibling_functions::resolve(&value, node);
        if !registered_value_matches_syntax(&value, syntax) {
            return false;
        }
        let ctx = ResolutionContext {
            parent_font_size: units.font_size,
            root_font_size: units.root_font_size,
            line_height: units.line_height,
            root_line_height: units.root_line_height,
            font_metrics: units.metrics,
            ..ResolutionContext::default()
        };
        compute_registered_value(&value, syntax, ctx).css_text()
            == compute_registered_value(actual, syntax, ctx).css_text()
    }
}

/// Deterministic post-load instant used for static screenshots.
const STATIC_ANIMATION_TIME_SECONDS: f32 = 1.2;

static UNSUPPORTED_CSS_LOGGED: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
static UNSUPPORTED_CSS_CONFIG: OnceLock<UnsupportedCssConfig> = OnceLock::new();
static SQLITE_LOG_ERRORS: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
static UNSUPPORTED_CSS_TOP_N_LAST_DIGEST: OnceLock<Mutex<HashMap<String, u64>>> = OnceLock::new();
const MAX_UNSUPPORTED_LOG_KEYS: usize = 4096;
const MAX_UNSUPPORTED_LOG_VALUE_LEN: usize = 256;
const MAX_SQLITE_LOG_ERRORS: usize = 1024;
const DEFAULT_UNSUPPORTED_CSS_TOP_N: usize = 20;

thread_local! {
    static SQLITE_CONNECTIONS: RefCell<HashMap<String, Connection>> = RefCell::new(HashMap::new());
}

#[derive(Debug, Clone)]
struct UnsupportedCssConfig {
    logging_enabled: bool,
    sqlite_path: Option<String>,
    top_n: Option<usize>,
}

fn selector_observes_style_attribute(selector: &Selector) -> bool {
    selector.parts.iter().any(|part| {
        part.simples.iter().any(|simple| match simple {
            SimpleSelector::Attribute { name, .. } => name.eq_ignore_ascii_case("style"),
            SimpleSelector::Is(selectors)
            | SimpleSelector::Where(selectors)
            | SimpleSelector::Not(selectors)
            | SimpleSelector::NthChild { selectors, .. } => {
                selectors.iter().any(selector_observes_style_attribute)
            }
            SimpleSelector::Has(selectors) => selectors
                .iter()
                .any(|relative| selector_observes_style_attribute(&relative.selector)),
            SimpleSelector::PseudoClass(name) | SimpleSelector::PseudoElement(name) => {
                functional_selector_observes_style_attribute(name)
            }
            _ => false,
        })
    })
}

fn functional_selector_observes_style_attribute(value: &str) -> bool {
    let Some((_, rest)) = value.split_once('(') else {
        return false;
    };
    let Some(argument) = rest.strip_suffix(')') else {
        return false;
    };
    selector_text_observes_style_attribute(argument)
        || selector_tokens_observe_style_attribute(argument)
}

fn selector_text_observes_style_attribute(input: &str) -> bool {
    super::parse_selector_list(input)
        .is_ok_and(|selectors| selectors.iter().any(selector_observes_style_attribute))
}

fn selector_tokens_observe_style_attribute(input: &str) -> bool {
    let Ok(tokens) = super::tokenize(input) else {
        return false;
    };
    tokens.iter().enumerate().any(|(index, token)| {
        (*token == CssToken::BracketOpen)
            .then(|| attribute_selector_local_name(&tokens[index + 1..]))
            .flatten()
            .is_some_and(|name| name.eq_ignore_ascii_case("style"))
    })
}

fn attribute_selector_local_name(tokens: &[CssToken]) -> Option<&str> {
    let mut significant = tokens
        .iter()
        .filter(|token| **token != CssToken::Whitespace);
    match significant.next()? {
        CssToken::Ident(name) => match (significant.next(), significant.next()) {
            (Some(CssToken::Delim('|')), Some(CssToken::Ident(local_name))) => Some(local_name),
            _ => Some(name),
        },
        CssToken::Delim('|') => match significant.next() {
            Some(CssToken::Ident(name)) => Some(name),
            _ => None,
        },
        CssToken::Delim('*') => match (significant.next(), significant.next()) {
            (Some(CssToken::Delim('|')), Some(CssToken::Ident(name))) => Some(name),
            _ => None,
        },
        _ => None,
    }
}

fn selector_style_attribute_dependency(selector: &Selector) -> StyleAttributeSelectorDependency {
    let Some(subject_index) = selector.parts.len().checked_sub(1) else {
        return StyleAttributeSelectorDependency::None;
    };
    let mut dependency = StyleAttributeSelectorDependency::None;
    for (part_index, part) in selector.parts.iter().enumerate() {
        for simple in &part.simples {
            dependency = dependency.max(simple_style_attribute_dependency(
                simple,
                part_index == subject_index,
            ));
        }
    }
    if dependency == StyleAttributeSelectorDependency::SubjectOnly
        && (selector_contains_has(selector) || selector_contains_shadow_pseudo(selector))
    {
        StyleAttributeSelectorDependency::NonLocal
    } else {
        dependency
    }
}

fn selector_contains_has(selector: &Selector) -> bool {
    selector.parts.iter().any(|part| {
        part.simples.iter().any(|simple| match simple {
            SimpleSelector::Has(_) => true,
            SimpleSelector::Is(selectors)
            | SimpleSelector::Where(selectors)
            | SimpleSelector::Not(selectors)
            | SimpleSelector::NthChild { selectors, .. } => {
                selectors.iter().any(selector_contains_has)
            }
            _ => false,
        })
    })
}

fn selector_contains_shadow_pseudo(selector: &Selector) -> bool {
    if selector_uses_shadow_pseudo(selector) {
        return true;
    }
    selector.parts.iter().any(|part| {
        part.simples.iter().any(|simple| match simple {
            SimpleSelector::Is(selectors)
            | SimpleSelector::Where(selectors)
            | SimpleSelector::Not(selectors)
            | SimpleSelector::NthChild { selectors, .. } => {
                selectors.iter().any(selector_contains_shadow_pseudo)
            }
            SimpleSelector::Has(selectors) => selectors
                .iter()
                .any(|relative| selector_contains_shadow_pseudo(&relative.selector)),
            _ => false,
        })
    })
}

fn simple_style_attribute_dependency(
    simple: &SimpleSelector,
    in_subject: bool,
) -> StyleAttributeSelectorDependency {
    match simple {
        SimpleSelector::Attribute { name, .. } if name.eq_ignore_ascii_case("style") => {
            if in_subject {
                StyleAttributeSelectorDependency::SubjectOnly
            } else {
                StyleAttributeSelectorDependency::NonLocal
            }
        }
        SimpleSelector::Is(selectors)
        | SimpleSelector::Where(selectors)
        | SimpleSelector::Not(selectors) => selectors.iter().fold(
            StyleAttributeSelectorDependency::None,
            |dependency, selector| {
                let nested = selector_style_attribute_dependency(selector);
                let nested = match nested {
                    StyleAttributeSelectorDependency::None => nested,
                    StyleAttributeSelectorDependency::SubjectOnly
                        if in_subject && selector.parts.len() == 1 =>
                    {
                        nested
                    }
                    _ => StyleAttributeSelectorDependency::NonLocal,
                };
                dependency.max(nested)
            },
        ),
        SimpleSelector::NthChild { selectors, .. } => {
            if selectors.iter().any(selector_observes_style_attribute) {
                StyleAttributeSelectorDependency::NonLocal
            } else {
                StyleAttributeSelectorDependency::None
            }
        }
        SimpleSelector::Has(selectors) => {
            if selectors
                .iter()
                .any(|relative| selector_observes_style_attribute(&relative.selector))
            {
                StyleAttributeSelectorDependency::NonLocal
            } else {
                StyleAttributeSelectorDependency::None
            }
        }
        SimpleSelector::PseudoClass(name) | SimpleSelector::PseudoElement(name)
            if functional_selector_observes_style_attribute(name) =>
        {
            StyleAttributeSelectorDependency::NonLocal
        }
        _ => StyleAttributeSelectorDependency::None,
    }
}

fn rules_style_attribute_dependency(rules: &[Rule]) -> StyleAttributeSelectorDependency {
    rules.iter().fold(
        StyleAttributeSelectorDependency::None,
        |dependency, rule| {
            let rule_dependency = match rule {
                Rule::Style(rule) => rule
                    .selectors
                    .iter()
                    .map(selector_style_attribute_dependency)
                    .chain(std::iter::once(rules_style_attribute_dependency(
                        &rule.rules,
                    )))
                    .max()
                    .unwrap_or_default(),
                Rule::At(rule) => {
                    let scope_observes = rule.name.eq_ignore_ascii_case("scope")
                        && (super::parse_scope_prelude(&rule.prelude).is_some_and(|scope| {
                            scope
                                .start
                                .iter()
                                .chain(scope.end.iter())
                                .flatten()
                                .any(selector_observes_style_attribute)
                        }) || selector_tokens_observe_style_attribute(&rule.prelude));
                    let scope_dependency = if scope_observes {
                        StyleAttributeSelectorDependency::NonLocal
                    } else {
                        StyleAttributeSelectorDependency::None
                    };
                    scope_dependency.max(
                        rule.block
                            .as_deref()
                            .map(rules_style_attribute_dependency)
                            .unwrap_or_default(),
                    )
                }
                Rule::FontFace(_) => StyleAttributeSelectorDependency::None,
            };
            dependency.max(rule_dependency)
        },
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AttributeTokenObservation {
    Absent,
    Named,
    Unknown,
}

/// Looks for one attribute name in raw selector text retained by functional
/// pseudos and `@scope`. Unknown bracket syntax is deliberately not treated as
/// absence: a future parser extension could make it observable.
fn selector_tokens_observe_attribute(
    input: &str,
    attribute_name: &str,
) -> AttributeTokenObservation {
    let Ok(tokens) = super::tokenize(input) else {
        return AttributeTokenObservation::Unknown;
    };
    let mut observation = AttributeTokenObservation::Absent;
    for (index, token) in tokens.iter().enumerate() {
        if *token != CssToken::BracketOpen {
            continue;
        }
        let Some(close_offset) = tokens[index + 1..]
            .iter()
            .position(|token| *token == CssToken::BracketClose)
        else {
            return AttributeTokenObservation::Unknown;
        };
        let contents = &tokens[index + 1..index + 1 + close_offset];
        match attribute_selector_local_name(contents) {
            Some(name) if name.eq_ignore_ascii_case(attribute_name) => {
                return AttributeTokenObservation::Named;
            }
            Some(_) => {}
            None => observation = AttributeTokenObservation::Unknown,
        }
    }
    observation
}

fn functional_selector_observes_data_attribute(value: &str, attribute_name: &str) -> bool {
    let Some((_, rest)) = value.split_once('(') else {
        return false;
    };
    let Some(argument) = rest.strip_suffix(')') else {
        return true;
    };
    selector_tokens_observe_attribute(argument, attribute_name) != AttributeTokenObservation::Absent
}

fn selector_observes_data_attribute(selector: &Selector, attribute_name: &str) -> bool {
    selector.parts.iter().any(|part| {
        part.simples.iter().any(|simple| match simple {
            SimpleSelector::Attribute { name, .. } => name.eq_ignore_ascii_case(attribute_name),
            SimpleSelector::Is(selectors)
            | SimpleSelector::Where(selectors)
            | SimpleSelector::Not(selectors)
            | SimpleSelector::NthChild { selectors, .. } => selectors
                .iter()
                .any(|selector| selector_observes_data_attribute(selector, attribute_name)),
            SimpleSelector::Has(selectors) => selectors.iter().any(|relative| {
                selector_observes_data_attribute(&relative.selector, attribute_name)
            }),
            SimpleSelector::PseudoClass(name) | SimpleSelector::PseudoElement(name) => {
                functional_selector_observes_data_attribute(name, attribute_name)
            }
            _ => false,
        })
    })
}

fn selector_data_attribute_dependency(
    selector: &Selector,
    attribute_name: &str,
) -> AttributeSelectorDependency {
    let Some(subject_index) = selector.parts.len().checked_sub(1) else {
        return AttributeSelectorDependency::None;
    };
    let mut dependency = AttributeSelectorDependency::None;
    for (part_index, part) in selector.parts.iter().enumerate() {
        for simple in &part.simples {
            dependency = dependency.max(simple_data_attribute_dependency(
                simple,
                attribute_name,
                part_index == subject_index,
            ));
        }
    }
    if dependency == AttributeSelectorDependency::SubjectOnly
        && (selector_contains_has(selector) || selector_contains_shadow_pseudo(selector))
    {
        AttributeSelectorDependency::NonLocal
    } else {
        dependency
    }
}

fn simple_data_attribute_dependency(
    simple: &SimpleSelector,
    attribute_name: &str,
    in_subject: bool,
) -> AttributeSelectorDependency {
    match simple {
        SimpleSelector::Attribute { name, .. } if name.eq_ignore_ascii_case(attribute_name) => {
            if in_subject {
                AttributeSelectorDependency::SubjectOnly
            } else {
                AttributeSelectorDependency::NonLocal
            }
        }
        SimpleSelector::Is(selectors)
        | SimpleSelector::Where(selectors)
        | SimpleSelector::Not(selectors) => {
            selectors
                .iter()
                .fold(AttributeSelectorDependency::None, |dependency, selector| {
                    let nested = selector_data_attribute_dependency(selector, attribute_name);
                    let nested = match nested {
                        AttributeSelectorDependency::None => nested,
                        AttributeSelectorDependency::SubjectOnly
                            if in_subject && selector.parts.len() == 1 =>
                        {
                            nested
                        }
                        _ => AttributeSelectorDependency::NonLocal,
                    };
                    dependency.max(nested)
                })
        }
        SimpleSelector::NthChild { selectors, .. } => {
            if selectors
                .iter()
                .any(|selector| selector_observes_data_attribute(selector, attribute_name))
            {
                AttributeSelectorDependency::NonLocal
            } else {
                AttributeSelectorDependency::None
            }
        }
        SimpleSelector::Has(selectors) => {
            if selectors.iter().any(|relative| {
                selector_observes_data_attribute(&relative.selector, attribute_name)
            }) {
                AttributeSelectorDependency::NonLocal
            } else {
                AttributeSelectorDependency::None
            }
        }
        SimpleSelector::PseudoClass(name) | SimpleSelector::PseudoElement(name)
            if functional_selector_observes_data_attribute(name, attribute_name) =>
        {
            AttributeSelectorDependency::NonLocal
        }
        _ => AttributeSelectorDependency::None,
    }
}

fn rules_data_attribute_dependency(
    rules: &[Rule],
    attribute_name: &str,
    inside_nested_style: bool,
) -> AttributeSelectorDependency {
    rules
        .iter()
        .fold(AttributeSelectorDependency::None, |dependency, rule| {
            let rule_dependency = match rule {
                Rule::Style(rule) => {
                    let selector_dependency = rule
                        .selectors
                        .iter()
                        .map(|selector| {
                            selector_data_attribute_dependency(selector, attribute_name)
                        })
                        .max()
                        .unwrap_or_default();
                    let nested_dependency =
                        rules_data_attribute_dependency(&rule.rules, attribute_name, true);
                    let dependency = selector_dependency.max(nested_dependency);
                    if inside_nested_style && dependency != AttributeSelectorDependency::None {
                        AttributeSelectorDependency::NonLocal
                    } else if selector_dependency != AttributeSelectorDependency::None
                        && !rule.rules.is_empty()
                    {
                        // A nested selector can project an outer subject's
                        // attribute dependency onto a sibling or ancestor.
                        AttributeSelectorDependency::NonLocal
                    } else {
                        dependency
                    }
                }
                Rule::At(rule) => {
                    let scope_observes = rule.name.eq_ignore_ascii_case("scope")
                        && (super::parse_scope_prelude(&rule.prelude).is_some_and(|scope| {
                            scope
                                .start
                                .iter()
                                .chain(scope.end.iter())
                                .flatten()
                                .any(|selector| {
                                    selector_observes_data_attribute(selector, attribute_name)
                                })
                        }) || selector_tokens_observe_attribute(&rule.prelude, attribute_name)
                            != AttributeTokenObservation::Absent);
                    let scope_dependency = if scope_observes {
                        AttributeSelectorDependency::NonLocal
                    } else {
                        AttributeSelectorDependency::None
                    };
                    scope_dependency.max(
                        rule.block
                            .as_deref()
                            .map(|rules| {
                                rules_data_attribute_dependency(
                                    rules,
                                    attribute_name,
                                    inside_nested_style,
                                )
                            })
                            .unwrap_or_default(),
                    )
                }
                Rule::FontFace(_) => AttributeSelectorDependency::None,
            };
            dependency.max(rule_dependency)
        })
}

fn value_reads_data_attribute(value: &Value, attribute_name: &str) -> bool {
    match value {
        Value::Function { name, arguments } if name.eq_ignore_ascii_case("attr") => {
            let Some(first) = arguments.first() else {
                return true;
            };
            match first {
                Value::Keyword(name) | Value::String(name) => {
                    name.trim().eq_ignore_ascii_case(attribute_name)
                }
                _ => true,
            }
        }
        Value::Function { arguments, .. }
        | Value::List(arguments)
        | Value::CommaList(arguments) => arguments
            .iter()
            .any(|value| value_reads_data_attribute(value, attribute_name)),
        _ => false,
    }
}

fn declarations_read_data_attribute(declarations: &[Declaration], attribute_name: &str) -> bool {
    declarations
        .iter()
        .any(|declaration| value_reads_data_attribute(&declaration.value, attribute_name))
}

fn rules_read_data_attribute(rules: &[Rule], attribute_name: &str) -> bool {
    rules.iter().any(|rule| match rule {
        Rule::Style(rule) => {
            declarations_read_data_attribute(&rule.declarations, attribute_name)
                || rules_read_data_attribute(&rule.rules, attribute_name)
        }
        Rule::At(rule) => {
            declarations_read_data_attribute(&rule.declarations, attribute_name)
                || rule
                    .block
                    .as_deref()
                    .is_some_and(|rules| rules_read_data_attribute(rules, attribute_name))
        }
        Rule::FontFace(_) => false,
    })
}

fn parsed_inline_style(source: Option<&str>) -> Vec<Declaration> {
    source.map(super::parse_style_attribute).unwrap_or_default()
}

fn is_local_inline_property(name: &str) -> bool {
    matches!(
        canonical_property_name(name),
        "background-image" | "bottom" | "top" | "right" | "left" | "transform" | "display"
    )
}

fn is_flow_relative_inset(name: &str) -> bool {
    name.starts_with("inset-block-") || name.starts_with("inset-inline-")
}

fn inline_style_change_is_subtree_local(previous: Option<&str>, current: Option<&str>) -> bool {
    let previous = parsed_inline_style(previous);
    let current = parsed_inline_style(current);
    if previous
        .iter()
        .chain(&current)
        .any(|declaration| is_flow_relative_inset(&declaration.name))
    {
        return false;
    }
    previous
        .iter()
        .filter(|declaration| !is_local_inline_property(&declaration.name))
        .eq(current
            .iter()
            .filter(|declaration| !is_local_inline_property(&declaration.name)))
}

fn inline_invalidation_subtree(node: &NodeHandle) -> Option<HashSet<usize>> {
    invalidation_subtrees([node.clone()])
}

fn invalidation_subtrees(roots: impl IntoIterator<Item = NodeHandle>) -> Option<HashSet<usize>> {
    let mut pending = roots.into_iter().collect::<Vec<_>>();
    if pending
        .iter()
        .any(|node| node.containing_shadow_root().is_some())
    {
        return None;
    }
    let mut identities = HashSet::new();
    while let Some(current) = pending.pop() {
        if !identities.insert(current.identity()) {
            continue;
        }
        if current.shadow_root().is_some() || current.assigned_slot().is_some() {
            return None;
        }
        pending.extend(current.child_nodes());
    }
    Some(identities)
}

impl StyleResolver {
    pub(crate) fn replace_counter_values(
        &mut self,
        values: HashMap<(usize, Option<PseudoElement>), HashMap<String, Vec<i32>>>,
    ) {
        self.counter_values = values;
    }

    pub(crate) fn counter_values(
        &self,
        node: &NodeHandle,
        pseudo: PseudoElement,
        name: &str,
    ) -> Option<&[i32]> {
        self.counter_values
            .get(&(node.identity(), Some(pseudo)))?
            .get(name)
            .map(Vec::as_slice)
    }

    pub(crate) fn replace_quote_values(
        &mut self,
        values: HashMap<(usize, Option<PseudoElement>), Vec<String>>,
    ) {
        self.quote_values = values;
    }

    pub(crate) fn quote_values(
        &self,
        node: &NodeHandle,
        pseudo: PseudoElement,
    ) -> Option<&[String]> {
        self.quote_values
            .get(&(node.identity(), Some(pseudo)))
            .map(Vec::as_slice)
    }
    /// Creates a new style resolver.
    pub fn new() -> Self {
        Self::default()
    }

    /// Supplies the loaded web fonts used when resolving font-relative lengths.
    pub(crate) fn set_web_fonts(&mut self, fonts: Arc<WebFontRegistry>) {
        self.font_resources.web_fonts = Some(fonts);
        self.invalidate_style_cache();
    }

    /// Captures owned values and only the registration syntax used by this container.
    pub(crate) fn container_style_snapshot(&self, style: &ComputedStyle) -> ContainerStyleSnapshot {
        ContainerStyleSnapshot {
            values: style.custom_properties.clone(),
            syntax: style
                .custom_properties
                .keys()
                .filter_map(|name| {
                    self.registered_custom_properties
                        .get(name)
                        .map(|registration| (name.clone(), registration.syntax.clone()))
                })
                .collect(),
        }
    }

    pub(crate) fn container_unit_context(
        &self,
        style: &ComputedStyle,
    ) -> super::ContainerUnitContext {
        let font_size = match style.get("font-size") {
            Some(ComputedValue::Px(size)) => *size,
            _ => 16.0,
        };
        super::ContainerUnitContext {
            font_size,
            root_font_size: self.root_font_size(),
            line_height: used_line_height(Some(style), font_size),
            root_line_height: self.root_line_height(),
            metrics: self.css_font_metrics(
                &style.properties,
                style.font_family_scope_root(),
                font_size,
            ),
        }
    }

    fn container_size(&self, node: &NodeHandle) -> [Option<f32>; 2] {
        let mut result = [None, None];
        let mut ancestor = node.flat_tree_parent();
        while let Some(node) = ancestor {
            if let Some(context) = self.container_contexts.get(&node.identity())
                && context.eligible
            {
                let inline_axis = usize::from(context.vertical_writing);
                let supports_inline =
                    matches!(context.container_type.as_str(), "inline-size" | "size");
                let supports_block = context.container_type == "size";
                if supports_inline && result[inline_axis].is_none() {
                    result[inline_axis] = Some(if inline_axis == 0 {
                        context.width
                    } else {
                        context.height
                    });
                }
                let block_axis = 1 - inline_axis;
                if supports_block && result[block_axis].is_none() {
                    result[block_axis] = Some(if block_axis == 0 {
                        context.width
                    } else {
                        context.height
                    });
                }
                if result.iter().all(Option::is_some) {
                    break;
                }
            }
            ancestor = node.flat_tree_parent();
        }
        result
    }

    fn css_font_metrics(
        &self,
        properties: &PropertyMap,
        scope_root: Option<usize>,
        size_px: f32,
    ) -> CssRelativeFontMetrics {
        let keyword = |name| match properties.get(name) {
            Some(ComputedValue::Keyword(value) | ComputedValue::String(value)) => {
                Some(value.as_str())
            }
            _ => None,
        };
        let family = keyword("font-family").map(FontFamilyKey::new);
        let weight = match properties.get(&PropertyId::FontWeight) {
            Some(ComputedValue::Number(value)) => FontWeight((*value as u16).clamp(1, 1000)),
            _ => keyword("font-weight")
                .map(FontWeight::parse)
                .unwrap_or_default(),
        };
        let stretch = keyword("font-stretch")
            .map(FontStretch::parse)
            .unwrap_or_default();
        let variant =
            FontVariantKey::from_css(weight, keyword("font-style").unwrap_or("normal"), stretch);
        let vertical = keyword("writing-mode")
            .is_some_and(|value| matches!(value, "vertical-rl" | "vertical-lr"));
        let upright_zero = vertical && keyword("text-orientation") == Some("upright");
        let system_fonts = load_default_text_fonts_shared();
        let selected = select_text_font(
            "style",
            family,
            scope_root,
            variant,
            self.font_resources.web_fonts.as_deref(),
            &system_fonts,
        );
        let font: Option<&Font> = selected
            .as_ref()
            .map(AsRef::as_ref)
            .or_else(|| system_fonts.first().map(AsRef::as_ref));
        font.map_or_else(
            || CssRelativeFontMetrics::fallback(size_px, upright_zero),
            |font| font.css_relative_metrics(size_px, vertical, upright_zero),
        )
    }

    /// Whether size queries or size-container units need a geometry snapshot.
    pub(crate) fn needs_container_contexts(&self) -> bool {
        self.container_context_memo
            .get_or_compute(&self.cache, &self.pseudo_cache, || {
                #[cfg(test)]
                self.container_context_scans
                    .set(self.container_context_scans.get() + 1);
                self.cache
                    .values()
                    .chain(self.pseudo_cache.values())
                    .any(|style| {
                        style
                            .component_values
                            .values()
                            .any(value_uses_container_units)
                    })
                    || self
                        .stylesheets
                        .iter()
                        .any(|input| contains_at_rule_named(&input.stylesheet.rules, "container"))
            })
    }

    #[cfg(test)]
    fn container_context_scan_count(&self) -> usize {
        self.container_context_scans.get()
    }

    /// Returns the number of distinct `@media` prelude strings currently held
    /// in the parse cache.
    ///
    /// Primarily useful for testing and diagnostic purposes.
    #[cfg(test)]
    pub(crate) fn media_query_cache_len(&self) -> usize {
        self.media_query_cache.len()
    }

    /// Sets the root element's computed font-size in px.
    ///
    /// This value is used to resolve `rem` units. Defaults to 16px when not set.
    /// Calling this explicitly prevents the resolver from auto-deriving the root
    /// font size from the computed style of the root element.
    pub fn set_root_font_size(&mut self, px: f32) {
        self.root_font_size = px;
        self.root_font_size_explicit = true;
        self.cache.clear();
        self.pseudo_cache.clear();
        self.selector_match_cache = SelectorMatchCache::default();
    }

    /// Returns the root font size used to resolve `rem` units.
    pub(crate) fn root_font_size(&self) -> f32 {
        if self.root_font_size > 0.0 {
            self.root_font_size
        } else {
            16.0
        }
    }

    fn root_line_height(&self) -> f32 {
        if self.root_line_height > 0.0 {
            self.root_line_height
        } else {
            self.root_font_size() * 1.2
        }
    }

    /// Advances the CSS transition sampling clock without moving it backwards.
    pub(crate) fn set_transition_time_ms(&mut self, time_ms: f64) -> bool {
        self.sample_transition_time_ms(time_ms).0
    }

    fn sample_transition_time_ms(&mut self, time_ms: f64) -> (bool, TransitionCacheEffects) {
        let ordinary_before = self.cache.len();
        let pseudo_before = self.pseudo_cache.len();
        if !self.transition_timeline.set_time_ms(time_ms) {
            return (false, TransitionCacheEffects::default());
        }
        self.cache.clear();
        self.pseudo_cache.clear();
        (
            true,
            TransitionCacheEffects::from_lengths(
                TransitionCacheOutcome::Full,
                ordinary_before,
                self.cache.len(),
                pseudo_before,
                self.pseudo_cache.len(),
            ),
        )
    }

    /// Advances the transition clock and discards only caches that can observe
    /// a sampled value. The lookup must return nodes from the resolver's
    /// document; a missing node conservatively falls back to a full clear.
    fn sample_transition_time_ms_with_node_lookup<F>(
        &mut self,
        time_ms: f64,
        lookup: F,
    ) -> (bool, TransitionCacheEffects)
    where
        F: FnMut(usize) -> Option<NodeHandle>,
    {
        if !self.transition_timeline.sample_will_advance(time_ms) {
            let _ = self.transition_timeline.set_time_ms(time_ms);
            return (false, TransitionCacheEffects::default());
        }
        // Capture running nodes before moving the clock. Sampling the final
        // frame removes a completed transition only after its style is read.
        let scopes = self.transition_timeline.running_cache_scopes();
        let ordinary_before = self.cache.len();
        let pseudo_before = self.pseudo_cache.len();
        if !self.transition_timeline.set_time_ms(time_ms) {
            return (false, TransitionCacheEffects::default());
        }
        let outcome = self.invalidate_transition_cache_scopes(scopes, lookup);
        (
            true,
            TransitionCacheEffects::from_lengths(
                outcome,
                ordinary_before,
                self.cache.len(),
                pseudo_before,
                self.pseudo_cache.len(),
            ),
        )
    }

    fn invalidate_transition_cache_scopes<F>(
        &mut self,
        scopes: Vec<(usize, TransitionCacheScope)>,
        mut lookup: F,
    ) -> TransitionCacheOutcome
    where
        F: FnMut(usize) -> Option<NodeHandle>,
    {
        if scopes
            .iter()
            .any(|(_, scope)| *scope == TransitionCacheScope::Full)
        {
            self.cache.clear();
            self.pseudo_cache.clear();
            return TransitionCacheOutcome::Full;
        }

        let mut root_ids = HashSet::new();
        let mut roots = Vec::new();
        for (node_id, scope) in scopes {
            let Some(node) = lookup(node_id).filter(|node| node.identity() == node_id) else {
                self.cache.clear();
                self.pseudo_cache.clear();
                return TransitionCacheOutcome::Full;
            };
            match scope {
                TransitionCacheScope::Subtree => {
                    if root_ids.insert(node_id) {
                        roots.push(node);
                    }
                }
                TransitionCacheScope::Full => unreachable!("handled before node lookup"),
            }
        }
        let Some(identities) = invalidation_subtrees(roots) else {
            self.cache.clear();
            self.pseudo_cache.clear();
            return TransitionCacheOutcome::Full;
        };
        self.cache
            .retain(|identity, _| !identities.contains(identity));
        self.pseudo_cache
            .retain(|key, _| !identities.contains(&key.0));
        TransitionCacheOutcome::Subtree
    }

    /// Moves transition state into a replacement resolver after stylesheet
    /// invalidation, preserving before-change values and running transitions.
    pub(crate) fn take_transition_timeline(&mut self) -> super::transition::TransitionTimeline {
        std::mem::take(&mut self.transition_timeline)
    }

    pub(crate) fn install_transition_timeline(
        &mut self,
        timeline: super::transition::TransitionTimeline,
    ) {
        self.transition_timeline = timeline;
        self.cache.clear();
        self.pseudo_cache.clear();
    }

    pub(crate) fn take_transition_events(
        &mut self,
    ) -> Vec<super::transition::TransitionEventRecord> {
        self.transition_timeline.take_events()
    }

    pub(crate) fn finish_transition_sample(&mut self, active_node_ids: &HashSet<usize>) {
        self.transition_timeline.retain_nodes(active_node_ids);
    }

    pub(crate) fn running_transition_node_ids(&self) -> Vec<usize> {
        self.transition_timeline.running_node_ids()
    }

    /// Whether the next transition sample can use localized eviction and needs
    /// document node handles. A false result does not permit skipping sampling.
    pub(crate) fn transition_sample_needs_node_lookup(&self, time_ms: f64) -> bool {
        self.transition_timeline.sample_needs_node_lookup(time_ms)
    }

    pub(crate) fn has_running_transitions(&self) -> bool {
        self.transition_timeline.has_running_transitions()
    }

    pub(crate) fn running_transitions_require_layout(&self) -> bool {
        self.transition_timeline
            .running_transitions_require_layout()
    }

    /// Returns bounded transition-property counts for opt-in diagnostics.
    pub(crate) fn running_transition_property_counts(&self) -> super::TransitionPropertyCounts {
        self.transition_timeline.running_property_counts()
    }

    pub(crate) fn cancel_detached_transitions(&mut self, active_node_ids: &HashSet<usize>) {
        self.transition_timeline
            .cancel_detached_transitions(active_node_ids);
    }

    fn data_attribute_selector_dependency(
        &mut self,
        attribute_name: &str,
    ) -> AttributeSelectorDependency {
        let attribute_name = attribute_name.to_ascii_lowercase();
        if let Some(dependency) = self
            .data_attribute_selector_dependencies
            .get(&attribute_name)
        {
            return *dependency;
        }
        let dependency = self
            .stylesheets
            .iter()
            .map(|input| {
                rules_data_attribute_dependency(&input.stylesheet.rules, &attribute_name, false)
            })
            .max()
            .unwrap_or_default();
        self.data_attribute_selector_dependencies
            .insert(attribute_name, dependency);
        dependency
    }

    fn data_attribute_value_dependency(&mut self, node: &NodeHandle, attribute_name: &str) -> bool {
        let attribute_name = attribute_name.to_ascii_lowercase();
        let stylesheet_dependency =
            if let Some(dependency) = self.data_attribute_value_dependencies.get(&attribute_name) {
                *dependency
            } else {
                let dependency = self.stylesheets.iter().any(|input| {
                    rules_read_data_attribute(&input.stylesheet.rules, &attribute_name)
                });
                self.data_attribute_value_dependencies
                    .insert(attribute_name.clone(), dependency);
                dependency
            };
        stylesheet_dependency
            || parsed_inline_style(node.get_attribute("style").as_deref())
                .iter()
                .any(|declaration| value_reads_data_attribute(&declaration.value, &attribute_name))
    }

    /// Drops values derived from the current DOM while retaining parsed
    /// stylesheets, rule indexes, and condition-prelude parse caches.
    pub(crate) fn invalidate_style_cache(&mut self) {
        self.cache.clear();
        self.pseudo_cache.clear();
        self.selector_match_cache = SelectorMatchCache::default();
    }

    /// Invalidates computed styles after one element's inline `style` mutation.
    ///
    /// Returns a bounded diagnostic outcome. A `"local*"` outcome means cached
    /// styles outside the element's light-DOM subtree were retained; every
    /// `"fallback_*"` outcome means all computed and pseudo styles were cleared.
    /// Selectors are always invalidated.
    pub(crate) fn invalidate_inline_style_subtree(
        &mut self,
        node: &NodeHandle,
        previous: Option<&str>,
        current: Option<&str>,
        has_non_scalar_dom_string: bool,
        csp_block_state_changed: bool,
    ) -> &'static str {
        self.selector_match_cache = SelectorMatchCache::default();
        let is_document_element = node.node_type() == NodeType::Element
            && node
                .parent_node()
                .is_some_and(|parent| parent.node_type() == NodeType::Document);
        let fallback = if self.style_attribute_selector_dependency
            == StyleAttributeSelectorDependency::NonLocal
        {
            Some("fallback_raw_style_selector")
        } else if self.has_running_transitions() {
            Some("fallback_active_transition")
        } else if self.has_running_animations() {
            Some("fallback_active_animation")
        } else if is_document_element {
            Some("fallback_document_root")
        } else if has_non_scalar_dom_string {
            Some("fallback_non_scalar_dom_string")
        } else if csp_block_state_changed {
            Some("fallback_csp_block_state_change")
        } else if !inline_style_change_is_subtree_local(previous, current) {
            Some("fallback_unsupported_declaration_change")
        } else {
            None
        };
        if let Some(reason) = fallback {
            self.cache.clear();
            self.pseudo_cache.clear();
            return reason;
        }
        let Some(identities) = inline_invalidation_subtree(node) else {
            self.cache.clear();
            self.pseudo_cache.clear();
            return "fallback_shadow_or_slot";
        };
        self.cache
            .retain(|identity, _| !identities.contains(identity));
        self.pseudo_cache
            .retain(|key, _| !identities.contains(&key.0));
        if self.style_attribute_selector_dependency == StyleAttributeSelectorDependency::SubjectOnly
        {
            "local_raw_style_subject"
        } else {
            "local"
        }
    }

    /// Invalidates computed styles after one ordinary HTML `data-*` mutation.
    ///
    /// Selectors are always invalidated. Computed styles are retained only when
    /// neither selectors nor `attr()` declarations observe the attribute.
    pub(crate) fn invalidate_data_attribute_subtree(
        &mut self,
        node: &NodeHandle,
        attribute_name: &str,
        has_non_scalar_dom_string: bool,
    ) -> &'static str {
        self.selector_match_cache = SelectorMatchCache::default();
        let dependency = self.data_attribute_selector_dependency(attribute_name);
        let value_dependency = self.data_attribute_value_dependency(node, attribute_name);
        let is_document_element = node.node_type() == NodeType::Element
            && node
                .parent_node()
                .is_some_and(|parent| parent.node_type() == NodeType::Document);
        let fallback = if dependency == AttributeSelectorDependency::NonLocal {
            Some("fallback_data_attribute_selector")
        } else if is_document_element {
            Some("fallback_document_root")
        } else if has_non_scalar_dom_string {
            Some("fallback_non_scalar_dom_string")
        } else {
            None
        };
        if let Some(reason) = fallback {
            self.cache.clear();
            self.pseudo_cache.clear();
            return reason;
        }
        let Some(identities) = inline_invalidation_subtree(node) else {
            self.cache.clear();
            self.pseudo_cache.clear();
            return "fallback_shadow_or_slot";
        };
        if dependency == AttributeSelectorDependency::None && !value_dependency {
            return "local_data_attribute_inert";
        }
        if self.has_running_transitions() {
            self.cache.clear();
            self.pseudo_cache.clear();
            return "fallback_active_transition";
        }
        if self.has_running_animations() {
            self.cache.clear();
            self.pseudo_cache.clear();
            return "fallback_active_animation";
        }
        self.cache
            .retain(|identity, _| !identities.contains(identity));
        self.pseudo_cache
            .retain(|key, _| !identities.contains(&key.0));
        if dependency == AttributeSelectorDependency::SubjectOnly {
            "local_data_attribute_subject"
        } else {
            "local_data_attribute_none"
        }
    }

    /// Installs the CSP-filtered set of inline style attributes for the next
    /// computed-style pass. The attribute remains observable through CSSOM;
    /// only its cascade contribution is removed.
    pub(crate) fn set_blocked_inline_style_nodes(&mut self, nodes: HashSet<usize>) {
        if self.blocked_inline_style_nodes != nodes {
            self.blocked_inline_style_nodes = nodes;
            self.invalidate_style_cache();
        }
    }

    /// Updates one CSP-filtered inline-style entry after a `style` attribute
    /// mutation. The caller invalidates the owning document's computed-style
    /// cache separately, so this operation does not rescan or clear unrelated
    /// resolver state. Returns whether the blocked state changed.
    pub(crate) fn set_blocked_inline_style_node(&mut self, node_id: usize, blocked: bool) -> bool {
        if blocked {
            self.blocked_inline_style_nodes.insert(node_id)
        } else {
            self.blocked_inline_style_nodes.remove(&node_id)
        }
    }

    #[cfg(test)]
    pub(crate) fn invalidate_style_cache_for_test(&mut self) {
        self.invalidate_style_cache();
    }

    /// Sets the viewport dimensions in px.
    ///
    /// These values are used to resolve `vw`, `vh`, `vmin`, and `vmax` units.
    pub fn set_viewport(&mut self, width: f32, height: f32) {
        let layer_order_changed = self.viewport_width != width || self.viewport_height != height;
        self.viewport_width = width;
        self.viewport_height = height;
        if layer_order_changed {
            self.rebuild_layer_orders();
            self.rebuild_keyframes();
            self.rebuild_font_faces();
            self.rebuild_counter_styles();
            self.rebuild_registered_custom_properties();
        }
        self.cache.clear();
        self.pseudo_cache.clear();
        self.selector_match_cache = SelectorMatchCache::default();
    }

    /// Sets whether the system is in dark mode.
    ///
    /// Defaults to `false` (light mode). An active forced-color palette can
    /// override this preference when its Canvas is clearly dark or light.
    /// Clears the style cache so that subsequent calls
    /// to [`StyleResolver::computed_style`] reflect the new scheme.
    pub fn set_color_scheme_dark(&mut self, dark: bool) {
        self.media_environment.color_scheme_dark = dark;
        let effective_dark = self.media_environment.preferred_color_scheme_dark();
        let layer_order_changed = self.color_scheme_dark != effective_dark;
        self.color_scheme_dark = effective_dark;
        if layer_order_changed {
            self.rebuild_layer_orders();
            self.rebuild_keyframes();
            self.rebuild_font_faces();
            self.rebuild_counter_styles();
            self.rebuild_registered_custom_properties();
        }
        self.cache.clear();
        self.pseudo_cache.clear();
        self.selector_match_cache = SelectorMatchCache::default();
    }

    /// Selects the output medium used by conditional rules.
    ///
    /// Screen rendering is the default. Paged layout switches this to
    /// [`MediaType::Print`] before resolving document and page styles.
    pub fn set_media_type(&mut self, media_type: MediaType) {
        if self.media_type == media_type {
            return;
        }
        self.media_type = media_type;
        self.media_environment.media_type = media_type;
        self.rebuild_layer_orders();
        self.rebuild_keyframes();
        self.rebuild_font_faces();
        self.rebuild_counter_styles();
        self.rebuild_registered_custom_properties();
        self.invalidate_style_cache();
    }

    /// Installs an owned presentation-settings snapshot for all conditional rules.
    /// Rebuilds conditional registries and invalidates cached styles.
    pub fn set_media_environment(&mut self, environment: MediaEnvironment) {
        if self.media_environment == environment {
            return;
        }
        self.color_scheme_dark = environment.preferred_color_scheme_dark();
        self.media_type = environment.media_type;
        self.media_environment = environment;
        self.rebuild_layer_orders();
        self.rebuild_keyframes();
        self.rebuild_font_faces();
        self.rebuild_counter_styles();
        self.rebuild_registered_custom_properties();
        self.invalidate_style_cache();
    }

    /// Returns the output medium currently used by the resolver.
    pub fn media_type(&self) -> MediaType {
        self.media_type
    }

    /// Installs the query-container snapshot for the next style pass.
    pub(crate) fn set_container_contexts(
        &mut self,
        contexts: HashMap<usize, ContainerContext>,
    ) -> bool {
        if self.container_contexts == contexts {
            return false;
        }
        self.container_contexts = contexts;
        self.cache.clear();
        self.pseudo_cache.clear();
        self.selector_match_cache = SelectorMatchCache::default();
        true
    }

    /// Captures the container sizes used by the current layout pass.
    pub(crate) fn container_contexts_snapshot(&self) -> HashMap<usize, ContainerContext> {
        self.container_contexts.clone()
    }

    /// Adds a stylesheet with its origin.
    pub fn add_stylesheet(&mut self, origin: Origin, stylesheet: Stylesheet) {
        let stylesheet_id = self.register_stylesheet_layers(origin, None, &stylesheet);
        self.rule_indexes
            .push(StylesheetRuleIndex::build(&stylesheet));
        self.stylesheets
            .push(StylesheetInput { origin, stylesheet });
        self.stylesheet_ids.push(stylesheet_id);
        self.stylesheet_scopes.push(StylesheetScope {
            root: None,
            implicit_scope_root: None,
            encapsulation_order: 0,
        });
        self.register_latest_stylesheet_keyframes();
        self.register_latest_stylesheet_font_faces();
        self.register_latest_stylesheet_counter_styles();
        self.rebuild_registered_custom_properties();
        self.cache.clear();
        self.pseudo_cache.clear();
        self.selector_match_cache = SelectorMatchCache::default();
    }

    /// Adds an inline stylesheet and records its owner element's parent for
    /// an omitted `@scope` start boundary.
    pub(crate) fn add_stylesheet_with_implicit_scope_root(
        &mut self,
        origin: Origin,
        stylesheet: Stylesheet,
        implicit_scope_root: NodeHandle,
    ) {
        let stylesheet_id = self.register_stylesheet_layers(origin, None, &stylesheet);
        self.rule_indexes
            .push(StylesheetRuleIndex::build(&stylesheet));
        self.stylesheets
            .push(StylesheetInput { origin, stylesheet });
        self.stylesheet_ids.push(stylesheet_id);
        self.stylesheet_scopes.push(StylesheetScope {
            root: None,
            implicit_scope_root: Some(implicit_scope_root),
            encapsulation_order: 0,
        });
        self.register_latest_stylesheet_keyframes();
        self.register_latest_stylesheet_font_faces();
        self.register_latest_stylesheet_counter_styles();
        self.rebuild_registered_custom_properties();
        self.cache.clear();
        self.pseudo_cache.clear();
        self.selector_match_cache = SelectorMatchCache::default();
    }

    /// Adds an author stylesheet owned by a ShadowRoot tree scope.
    pub fn add_scoped_stylesheet(
        &mut self,
        origin: Origin,
        stylesheet: Stylesheet,
        scope: NodeHandle,
    ) {
        let encapsulation_order = self
            .stylesheet_scopes
            .iter()
            .find(|input| input.root.as_ref() == Some(&scope))
            .map(|input| input.encapsulation_order)
            .unwrap_or_else(|| {
                self.stylesheet_scopes
                    .iter()
                    .map(|input| input.encapsulation_order)
                    .max()
                    .unwrap_or(0)
                    + 1
            });
        self.add_scoped_stylesheet_in_order(origin, stylesheet, scope, encapsulation_order);
    }

    /// Adds a ShadowRoot stylesheet with a tree-of-trees order computed by the
    /// document traversal, independent of where its first `<style>` occurs.
    pub(crate) fn add_scoped_stylesheet_in_order(
        &mut self,
        origin: Origin,
        stylesheet: Stylesheet,
        scope: NodeHandle,
        encapsulation_order: usize,
    ) {
        self.add_scoped_stylesheet_in_order_with_implicit_scope_root(
            origin,
            stylesheet,
            scope,
            encapsulation_order,
            None,
        );
    }

    /// Adds a shadow-tree stylesheet while retaining the implicit scope root
    /// of a directly-owned `<style>` element.  A style element whose parent is
    /// the shadow root itself is implicitly scoped to the shadow host rather
    /// than to the detached ShadowRoot fragment.
    pub(crate) fn add_scoped_stylesheet_in_order_with_implicit_scope_root(
        &mut self,
        origin: Origin,
        stylesheet: Stylesheet,
        scope: NodeHandle,
        encapsulation_order: usize,
        implicit_scope_root: Option<NodeHandle>,
    ) {
        let scope_root = Some(scope.identity());
        let stylesheet_id = self.register_stylesheet_layers(origin, scope_root, &stylesheet);
        self.rule_indexes
            .push(StylesheetRuleIndex::build(&stylesheet));
        self.stylesheets
            .push(StylesheetInput { origin, stylesheet });
        self.stylesheet_ids.push(stylesheet_id);
        self.stylesheet_scopes.push(StylesheetScope {
            root: Some(scope),
            implicit_scope_root,
            encapsulation_order,
        });
        self.register_latest_stylesheet_keyframes();
        self.register_latest_stylesheet_font_faces();
        self.register_latest_stylesheet_counter_styles();
        self.rebuild_registered_custom_properties();
        self.cache.clear();
        self.pseudo_cache.clear();
        self.selector_match_cache = SelectorMatchCache::default();
    }

    fn register_stylesheet_layers(
        &mut self,
        origin: Origin,
        scope_root: Option<usize>,
        stylesheet: &Stylesheet,
    ) -> usize {
        self.data_attribute_selector_dependencies.clear();
        self.class_selector_dependencies.clear();
        self.user_action_dependencies.clear();
        self.tree_mutation_reach = None;
        self.data_attribute_value_dependencies.clear();
        self.style_attribute_selector_dependency = self
            .style_attribute_selector_dependency
            .max(rules_style_attribute_dependency(&stylesheet.rules));
        let stylesheet_id = self.next_stylesheet_id;
        self.next_stylesheet_id += 1;
        self.layer_orders
            .entry(LayerContextKey { origin, scope_root })
            .or_default()
            .register_stylesheet(
                &stylesheet.rules,
                stylesheet_id,
                self.viewport_width,
                self.viewport_height,
                self.color_scheme_dark,
                self.media_type,
                &self.media_environment,
            );
        stylesheet_id
    }

    fn rebuild_layer_orders(&mut self) {
        let mut orders: HashMap<LayerContextKey, CascadeLayerOrder> = HashMap::new();
        for (position, (input, scope)) in self
            .stylesheets
            .iter()
            .zip(&self.stylesheet_scopes)
            .enumerate()
        {
            let key = LayerContextKey {
                origin: input.origin,
                scope_root: scope.root.as_ref().map(NodeHandle::identity),
            };
            orders.entry(key).or_default().register_stylesheet(
                &input.stylesheet.rules,
                self.stylesheet_ids[position],
                self.viewport_width,
                self.viewport_height,
                self.color_scheme_dark,
                self.media_type,
                &self.media_environment,
            );
        }
        self.layer_orders = orders;
    }

    fn rebuild_keyframes(&mut self) {
        let mut keyframes = HashMap::new();
        let mut source_order = 0;
        for (position, (input, scope)) in self
            .stylesheets
            .iter()
            .zip(&self.stylesheet_scopes)
            .enumerate()
        {
            let layer_context = LayerContextKey {
                origin: input.origin,
                scope_root: scope.root.as_ref().map(NodeHandle::identity),
            };
            let layer_order = self
                .layer_orders
                .get(&layer_context)
                .expect("stylesheet layer order should be registered");
            collect_keyframes(
                &input.stylesheet.rules,
                input.origin,
                self.stylesheet_ids[position],
                layer_context.scope_root,
                &sibling_functions::scope_ancestry(scope.root.as_ref()),
                layer_order,
                None,
                &mut source_order,
                &mut keyframes,
                self.viewport_width,
                self.viewport_height,
                self.color_scheme_dark,
                self.media_type,
                &self.media_environment,
            );
        }
        self.keyframes = keyframes;
        self.next_keyframes_source_order = source_order;
    }

    fn register_latest_stylesheet_keyframes(&mut self) {
        let position = self.stylesheets.len() - 1;
        let input = &self.stylesheets[position];
        let scope = &self.stylesheet_scopes[position];
        let layer_context = LayerContextKey {
            origin: input.origin,
            scope_root: scope.root.as_ref().map(NodeHandle::identity),
        };
        let layer_order = self
            .layer_orders
            .get(&layer_context)
            .expect("stylesheet layer order should be registered");
        collect_keyframes(
            &input.stylesheet.rules,
            input.origin,
            self.stylesheet_ids[position],
            layer_context.scope_root,
            &sibling_functions::scope_ancestry(scope.root.as_ref()),
            layer_order,
            None,
            &mut self.next_keyframes_source_order,
            &mut self.keyframes,
            self.viewport_width,
            self.viewport_height,
            self.color_scheme_dark,
            self.media_type,
            &self.media_environment,
        );
    }

    fn rebuild_font_faces(&mut self) {
        let mut font_faces = HashMap::new();
        let mut active_font_faces = Vec::new();
        let mut source_order = 0;
        for (position, (input, scope)) in self
            .stylesheets
            .iter()
            .zip(&self.stylesheet_scopes)
            .enumerate()
        {
            let layer_context = LayerContextKey {
                origin: input.origin,
                scope_root: scope.root.as_ref().map(NodeHandle::identity),
            };
            let layer_order = self
                .layer_orders
                .get(&layer_context)
                .expect("stylesheet layer order should be registered");
            collect_font_faces(
                &input.stylesheet.rules,
                input.origin,
                self.stylesheet_ids[position],
                layer_context.scope_root,
                layer_order,
                None,
                &mut source_order,
                &mut font_faces,
                &mut active_font_faces,
                self.viewport_width,
                self.viewport_height,
                self.color_scheme_dark,
                self.media_type,
                &self.media_environment,
            );
        }
        self.font_faces = font_faces;
        self.active_font_faces = active_font_faces;
        self.next_font_face_source_order = source_order;
    }

    fn register_latest_stylesheet_font_faces(&mut self) {
        let position = self.stylesheets.len() - 1;
        let input = &self.stylesheets[position];
        let scope = &self.stylesheet_scopes[position];
        let layer_context = LayerContextKey {
            origin: input.origin,
            scope_root: scope.root.as_ref().map(NodeHandle::identity),
        };
        let layer_order = self
            .layer_orders
            .get(&layer_context)
            .expect("stylesheet layer order should be registered");
        collect_font_faces(
            &input.stylesheet.rules,
            input.origin,
            self.stylesheet_ids[position],
            layer_context.scope_root,
            layer_order,
            None,
            &mut self.next_font_face_source_order,
            &mut self.font_faces,
            &mut self.active_font_faces,
            self.viewport_width,
            self.viewport_height,
            self.color_scheme_dark,
            self.media_type,
            &self.media_environment,
        );
    }

    fn rebuild_counter_styles(&mut self) {
        let mut counter_styles = HashMap::new();
        let mut source_order = 0;
        for (position, (input, scope)) in self
            .stylesheets
            .iter()
            .zip(&self.stylesheet_scopes)
            .enumerate()
        {
            let layer_context = LayerContextKey {
                origin: input.origin,
                scope_root: scope.root.as_ref().map(NodeHandle::identity),
            };
            let layer_order = self
                .layer_orders
                .get(&layer_context)
                .expect("stylesheet layer order should be registered");
            collect_counter_styles(
                &input.stylesheet.rules,
                input.origin,
                self.stylesheet_ids[position],
                layer_context.scope_root,
                layer_order,
                None,
                &mut source_order,
                &mut counter_styles,
                self.viewport_width,
                self.viewport_height,
                self.color_scheme_dark,
                self.media_type,
                &self.media_environment,
            );
        }
        self.counter_styles = counter_styles;
    }

    fn register_latest_stylesheet_counter_styles(&mut self) {
        let position = self.stylesheets.len() - 1;
        let input = &self.stylesheets[position];
        let scope = &self.stylesheet_scopes[position];
        let layer_context = LayerContextKey {
            origin: input.origin,
            scope_root: scope.root.as_ref().map(NodeHandle::identity),
        };
        let layer_order = self
            .layer_orders
            .get(&layer_context)
            .expect("stylesheet layer order should be registered");
        let source_order = self
            .counter_styles
            .values()
            .flat_map(|styles| styles.values())
            .map(|style| style.source_order)
            .max()
            .map_or(0, |order| order + 1);
        let mut source_order = source_order;
        collect_counter_styles(
            &input.stylesheet.rules,
            input.origin,
            self.stylesheet_ids[position],
            layer_context.scope_root,
            layer_order,
            None,
            &mut source_order,
            &mut self.counter_styles,
            self.viewport_width,
            self.viewport_height,
            self.color_scheme_dark,
            self.media_type,
            &self.media_environment,
        );
    }

    pub(crate) fn format_counter_value(&self, value: i32, style: &str) -> Option<String> {
        let style = style.to_ascii_lowercase();
        let definitions = self
            .counter_styles
            .values()
            .find_map(|styles| styles.get(&style));
        let definition = definitions?;
        let mut result = match &definition.system {
            CounterStyleSystem::Cyclic => {
                let index = value
                    .saturating_sub(1)
                    .rem_euclid(definition.symbols.len() as i32)
                    as usize;
                definition.symbols.get(index)?.clone()
            }
            CounterStyleSystem::Extends(base) => self.format_counter_value(value, base)?,
        };
        result = format!("{}{}{}", definition.prefix, result, definition.suffix);
        Some(result)
    }

    /// Installs the script registrations for this document. Script
    /// registrations override stylesheet registrations with the same name.
    pub(crate) fn set_script_registered_custom_properties(
        &mut self,
        registrations: BTreeMap<String, RegisteredCustomProperty>,
    ) {
        if self.script_registered_custom_properties == registrations {
            return;
        }
        self.script_registered_custom_properties = registrations;
        self.rebuild_registered_custom_properties();
        self.invalidate_style_cache();
    }

    fn rebuild_registered_custom_properties(&mut self) {
        let mut registrations = BTreeMap::new();
        for (input, scope) in self.stylesheets.iter().zip(&self.stylesheet_scopes) {
            // `@property` is document-scoped and rules in shadow trees do not
            // register names in the outer document.
            if scope.root.is_some() {
                continue;
            }
            collect_registered_custom_properties(
                &input.stylesheet.rules,
                &mut registrations,
                self.viewport_width,
                self.viewport_height,
                self.color_scheme_dark,
                self.media_type,
                &self.media_environment,
            );
        }
        registrations.extend(self.script_registered_custom_properties.clone());
        self.registered_custom_properties = registrations;
    }

    /// Returns the active `@font-face` winner for each supported variant and
    /// tree scope in stable source order.
    pub(crate) fn resolved_font_face_rules(&self) -> Vec<(Option<usize>, super::FontFaceRule)> {
        let mut definitions = self
            .font_faces
            .iter()
            .flat_map(|(scope_root, rules)| {
                rules.values().map(|definition| (*scope_root, definition))
            })
            .collect::<Vec<_>>();
        definitions.sort_by_key(|(_, definition)| definition.source_order);
        definitions
            .into_iter()
            .map(|(scope_root, definition)| (scope_root, definition.rule.clone()))
            .collect()
    }

    /// Returns every conditionally active `@font-face` rule for CSS FontFaceSet
    /// enumeration, including lower-precedence rules that do not win drawing.
    pub(crate) fn active_font_face_rules(&self) -> &[(Option<usize>, super::FontFaceRule)] {
        &self.active_font_faces
    }

    /// Reads whether an existing ordinary-style cache entry has `display: none`.
    ///
    /// Returns `None` without resolving style when `node` has no cached entry.
    pub(crate) fn cached_display_none(&self, node: &NodeHandle) -> Option<bool> {
        self.cache
            .get(&node.identity())
            .map(|style| style.is_display_none())
    }

    /// Resolves computed style for `node`, using the cache when possible.
    pub fn computed_style(&mut self, node: &NodeHandle) -> ComputedStyle {
        self.computed_style_shared(node).as_ref().clone()
    }

    /// Shares an immutable cached style with layout and paint readers. Invalidation
    /// replaces the cache entry without changing any retained snapshot.
    pub(crate) fn computed_style_shared(&mut self, node: &NodeHandle) -> Arc<ComputedStyle> {
        let key = node.identity();
        if let Some(style) = self.cache.get(&key) {
            super::style_profile::record_cache_hit();
            return style.clone();
        }

        let inheritance_parent = flattened_assigned_slot(node).or_else(|| node.parent_node());
        let inherited = inheritance_parent.map(|parent| {
            if parent.node_type() == NodeType::DocumentFragment {
                parent
                    .shadow_host()
                    .map(|host| self.computed_style_shared(&host))
                    .unwrap_or_default()
            } else {
                self.computed_style_shared(&parent)
            }
        });
        let style = {
            let _sample = super::style_profile::StyleComputationSample::begin();
            self.compute_style(node, inherited.as_deref())
        };

        // Auto-update root_font_size from the root element's computed font-size so that
        // `rem` units in descendant elements resolve correctly even without an explicit
        // set_root_font_size() call. Skip if the caller already provided an explicit value.
        let is_root = node.node_type() == NodeType::Element
            && node.with_tag_name(|tag| tag.is_some_and(|tag| tag.eq_ignore_ascii_case("html")))
            && node
                .parent_node()
                .is_some_and(|parent| parent.node_type() == NodeType::Document);
        if is_root {
            if !self.root_font_size_explicit
                && let Some(ComputedValue::Px(px)) = style.get("font-size")
            {
                self.root_font_size = *px;
            }
            self.root_line_height = used_line_height(Some(&style), self.root_font_size());
        }

        let style = Arc::new(style);
        self.cache.insert(key, Arc::clone(&style));
        style
    }

    /// Resolves one computed property without cloning the complete style map
    /// when the node is already cached.  Hot hit-test paths only need a single
    /// value, such as `pointer-events`.
    pub fn computed_property(&mut self, node: &NodeHandle, name: &str) -> Option<ComputedValue> {
        let key = node.identity();
        if let Some(style) = self.cache.get(&key) {
            return style.get(name).cloned();
        }
        self.computed_style_shared(node).get(name).cloned()
    }

    /// Supplies a fresh, private visit snapshot for the current paint call.
    /// Call after layout so geometry and public computed styles stay unvisited.
    pub(crate) fn begin_visited_paint(
        &mut self,
        visited_link_ids: impl IntoIterator<Item = usize>,
    ) {
        let ids: Vec<_> = visited_link_ids.into_iter().collect();
        if ids.is_empty() {
            self.visited_paint_state = None;
            return;
        }
        let state = VisitedPaintStylePass::new(self, ids).into_state();
        self.visited_paint_state = Some(state);
    }

    pub(crate) fn end_visited_paint(&mut self) {
        self.visited_paint_state = None;
    }

    pub(crate) fn has_visited_paint(&self) -> bool {
        self.visited_paint_state.is_some()
    }

    fn with_visited_paint_pass<T>(
        &mut self,
        action: impl FnOnce(&mut VisitedPaintStylePass<'_>) -> T,
    ) -> Option<T> {
        let state = self.visited_paint_state.take()?;
        let mut pass = VisitedPaintStylePass::from_state(self, state);
        let result = action(&mut pass);
        self.visited_paint_state = Some(pass.into_state());
        Some(result)
    }

    pub(crate) fn paint_style(&mut self, node: &NodeHandle) -> Arc<ComputedStyle> {
        let style = self
            .with_visited_paint_pass(|pass| pass.style(node))
            .unwrap_or_else(|| self.computed_style_shared(node));
        forced_colors::paint_style(node, style, &self.media_environment)
    }

    /// Propagates background values without propagating the body's opt-out.
    /// The cached computed styles remain unchanged, including currentColor.
    pub(crate) fn viewport_paint_style(
        &mut self,
        root: &NodeHandle,
        background_source: &NodeHandle,
    ) -> ViewportPaintStyle {
        let root_style = self.computed_style_shared(root);
        let mut viewport = self
            .computed_style_shared(background_source)
            .as_ref()
            .clone();
        if viewport
            .get("background-color")
            .is_some_and(|value| value.css_text().eq_ignore_ascii_case("currentcolor"))
        {
            viewport
                .properties
                .copy_property("color", "background-color");
        }
        viewport
            .properties
            .insert_from("color", &root_style.properties);
        viewport
            .properties
            .insert_from("forced-color-adjust", &root_style.properties);
        let canvas_color = root_style
            .get("forced-color-adjust")
            .is_some_and(|value| value.css_text().eq_ignore_ascii_case("auto"))
            .then(|| {
                self.media_environment
                    .used_forced_color_palette()
                    .map(|palette| palette.canvas)
            })
            .flatten();
        ViewportPaintStyle {
            style: forced_colors::paint_style(root, Arc::new(viewport), &self.media_environment),
            canvas_color,
        }
    }

    pub(crate) fn paint_pseudo_style(
        &mut self,
        node: &NodeHandle,
        pseudo: PseudoElement,
    ) -> Option<Arc<ComputedStyle>> {
        let style = self
            .with_visited_paint_pass(|pass| pass.pseudo_style(node, pseudo))
            .unwrap_or_else(|| self.computed_pseudo_style_shared(node, pseudo));
        style.map(|style| forced_colors::paint_style(node, style, &self.media_environment))
    }

    /// Resolves computed style for a pseudo-element attached to `node`.
    pub fn computed_pseudo_style(
        &mut self,
        node: &NodeHandle,
        pseudo: PseudoElement,
    ) -> Option<ComputedStyle> {
        self.computed_pseudo_style_shared(node, pseudo)
            .map(|style| style.as_ref().clone())
    }

    /// Shares an immutable cached pseudo style without cloning its property maps.
    pub(crate) fn computed_pseudo_style_shared(
        &mut self,
        node: &NodeHandle,
        pseudo: PseudoElement,
    ) -> Option<Arc<ComputedStyle>> {
        let key = (node.identity(), pseudo);
        if let Some(style) = self.pseudo_cache.get(&key) {
            return Some(style.clone());
        }

        let parent_style = match pseudo {
            PseudoElement::BeforeMarker => {
                self.computed_pseudo_style_shared(node, PseudoElement::Before)?
            }
            PseudoElement::AfterMarker => {
                self.computed_pseudo_style_shared(node, PseudoElement::After)?
            }
            _ => self.computed_style_shared(node),
        };
        // Inherit from the immediate originating pseudo, while numeric tree
        // counting still receives the ultimate originating DOM element.
        let style = self.compute_style_with_pseudo(node, Some(&parent_style), Some(pseudo), None);
        if style.properties.is_empty() {
            return None;
        }

        let style = Arc::new(style);
        self.pseudo_cache.insert(key, Arc::clone(&style));
        Some(style)
    }

    /// Attaches owned declaration scope IDs before variable alias substitution.
    fn annotate_tree_counting_candidates(&self, node: &NodeHandle, candidates: &mut [Candidate]) {
        for candidate in candidates {
            if !sibling_functions::contains(&candidate.value) {
                continue;
            }
            let root = candidate.layer_context.scope_root.and_then(|id| {
                self.stylesheet_scopes
                    .iter()
                    .filter_map(|scope| scope.root.as_ref())
                    .find(|root| root.identity() == id)
                    .cloned()
                    .or_else(|| {
                        node.containing_shadow_root()
                            .filter(|root| root.identity() == id)
                    })
            });
            let ancestry = sibling_functions::scope_ancestry(root.as_ref());
            candidate.value = sibling_functions::annotate(&candidate.value, &ancestry);
        }
    }

    fn compute_style(
        &mut self,
        node: &NodeHandle,
        parent_style: Option<&ComputedStyle>,
    ) -> ComputedStyle {
        self.compute_style_with_pseudo(node, parent_style, None, None)
    }

    fn compute_style_with_pseudo(
        &mut self,
        node: &NodeHandle,
        parent_style: Option<&ComputedStyle>,
        pseudo: Option<PseudoElement>,
        paint_custom_properties: Option<&BTreeMap<String, Value>>,
    ) -> ComputedStyle {
        let mut candidates = Vec::new();
        let mut source_order = 0usize;
        collect_builtin_ua_candidates(node, pseudo, &mut source_order, &mut candidates);
        let viewport_width = self.viewport_width;
        let viewport_height = self.viewport_height;
        let color_scheme_dark = self.color_scheme_dark;
        let element_keys = ElementMatchKeys::from_node(node);

        for (stylesheet_position, ((input, index), stylesheet_scope)) in self
            .stylesheets
            .iter()
            .zip(&self.rule_indexes)
            .zip(&self.stylesheet_scopes)
            .enumerate()
        {
            if input.origin == Origin::Author
                && stylesheet_scope.root.is_none()
                && node.containing_shadow_root().is_some()
                && !index.has_part_selector
            {
                continue;
            }
            let layer_context = LayerContextKey {
                origin: input.origin,
                scope_root: stylesheet_scope.root.as_ref().map(NodeHandle::identity),
            };
            let layer_order = self
                .layer_orders
                .get(&layer_context)
                .expect("stylesheet layer order should be registered");
            collect_indexed_rule_candidates(
                node,
                &input.stylesheet.rules,
                index,
                input.origin,
                self.stylesheet_ids[stylesheet_position],
                layer_context,
                layer_order,
                pseudo,
                &mut source_order,
                &mut candidates,
                viewport_width,
                viewport_height,
                color_scheme_dark,
                self.media_type,
                &self.media_environment,
                &mut self.media_query_cache,
                &mut self.scope_prelude_cache,
                &mut self.container_query_cache,
                &self.container_contexts,
                element_keys.as_ref(),
                &mut self.selector_match_cache,
                stylesheet_scope.root.as_ref(),
                stylesheet_scope.implicit_scope_root.as_ref(),
                stylesheet_scope.encapsulation_order,
            );
        }

        if pseudo.is_none()
            && node.node_type() == NodeType::Element
            && !self.blocked_inline_style_nodes.contains(&node.identity())
            && let Some(inline_style) = node.get_attribute("style")
        {
            let layer_context = LayerContextKey {
                origin: Origin::Author,
                scope_root: node
                    .containing_shadow_root()
                    .as_ref()
                    .map(NodeHandle::identity),
            };
            let rule_order = source_order;
            for declaration in super::parse_style_attribute(&inline_style) {
                candidates.push(Candidate {
                    name: canonical_property_name(&declaration.name).to_string(),
                    prefixed_alias: is_prefixed_property_alias(&declaration.name),
                    value: declaration.value,
                    important: declaration.important,
                    origin: Origin::Author,
                    inline: true,
                    specificity: Specificity {
                        ids: 0,
                        classes: 0,
                        elements: 0,
                    },
                    scope_proximity: None,
                    source_order,
                    rule_order,
                    encapsulation_order: tree_scope_order(&self.stylesheet_scopes, node),
                    layer_context,
                    layer_path: None,
                    layer_order: vec![usize::MAX],
                });
                source_order += 1;
            }
        }

        self.annotate_tree_counting_candidates(node, &mut candidates);
        candidates.sort_by(compare_candidate_priority);
        let unexpanded_candidates = candidates.clone();

        let mut custom_candidates: Vec<Candidate> = if paint_custom_properties.is_some() {
            Vec::new()
        } else {
            candidates
                .iter()
                .filter(|candidate| candidate.name.starts_with("--"))
                .cloned()
                .collect()
        };
        remove_reverted_candidates(&mut custom_candidates, None, None);
        let inherited_custom_properties = inherited_custom_properties(parent_style);
        let mut custom_properties = inherited_custom_properties.clone();
        let mut specified_custom_properties = BTreeMap::new();
        for candidate in custom_candidates {
            specified_custom_properties.insert(candidate.name.clone(), candidate.value.clone());
            match &candidate.value {
                Value::Keyword(keyword)
                    if keyword.eq_ignore_ascii_case("inherit")
                        || keyword.eq_ignore_ascii_case("unset") =>
                {
                    if let Some(inherited) = inherited_custom_properties.get(&candidate.name) {
                        custom_properties.insert(candidate.name, inherited.clone());
                    } else {
                        custom_properties.remove(&candidate.name);
                    }
                }
                Value::Keyword(keyword) if keyword.eq_ignore_ascii_case("initial") => {
                    custom_properties.remove(&candidate.name);
                }
                _ => {
                    custom_properties.insert(candidate.name, candidate.value);
                }
            }
        }
        let mut custom_properties = paint_custom_properties
            .cloned()
            .unwrap_or_else(|| resolve_custom_property_values(&custom_properties));
        candidates = expand_pending_shorthand_candidates(candidates, &custom_properties);
        candidates.sort_by(compare_candidate_priority);
        let flow = logical_flow_from_candidates(&candidates, &custom_properties, parent_style);
        remove_reverted_candidates(&mut candidates, Some(&custom_properties), Some(flow));

        let mut properties = PropertyMap::new();
        let mut component_values: BTreeMap<String, Value> = BTreeMap::new();

        // Effective root font-size for rem resolution: use the resolver's configured value,
        // falling back to the CSS default of 16px.
        let root_font_size = if self.root_font_size > 0.0 {
            self.root_font_size
        } else {
            16.0
        };
        let parent_font_size = parent_style
            .and_then(|style| style.get("font-size"))
            .and_then(|value| match value {
                ComputedValue::Px(px) => Some(*px),
                _ => None,
            })
            .unwrap_or(16.0);
        let parent_line_height = used_line_height(parent_style, parent_font_size);
        let root_line_height = self.root_line_height();
        let uses_font_metrics = candidates.iter().any(|candidate| {
            value_has_font_metric_unit(&candidate.value)
                || candidate.name.eq_ignore_ascii_case("font-size-adjust")
        }) || custom_properties.values().any(value_has_font_metric_unit);
        let parent_font_metrics = if uses_font_metrics {
            parent_style.map_or_else(
                || self.css_font_metrics(&PropertyMap::new(), None, parent_font_size),
                |style| {
                    self.css_font_metrics(
                        &style.properties,
                        style.font_family_scope_root(),
                        parent_font_size,
                    )
                },
            )
        } else {
            CssRelativeFontMetrics::fallback(parent_font_size, false)
        };

        let mut important_properties = HashSet::new();
        let mut animation_name_scope_root = None;
        let mut font_family_scope_root =
            parent_style.and_then(ComputedStyle::font_family_scope_root);

        let font_ctx = ResolutionContext {
            parent_font_size,
            root_font_size,
            line_height: parent_line_height,
            root_line_height,
            font_metrics: parent_font_metrics,
            viewport_width: self.viewport_width,
            viewport_height: self.viewport_height,
            container_size: [None, None],
            vertical_writing: false,
        };
        custom_properties = resolve_font_numeric_custom_properties(
            custom_properties,
            paint_custom_properties.unwrap_or(&specified_custom_properties),
            &inherited_custom_properties,
            &self.registered_custom_properties,
            font_ctx,
            node,
        );

        // Process font-size first so that em units in other properties
        // resolve against the element's own computed font-size.
        if let Some((fs_candidate, resolved_value)) = candidates
            .iter()
            .rev()
            .filter(|candidate| candidate.name == "font-size")
            .find_map(|candidate| {
                resolve_cascaded_value(candidate, &custom_properties, node)
                    .map(|(value, _)| (candidate, value))
            })
        {
            let ctx = font_ctx;
            let computed = compute_value(&resolved_value, "font-size", ctx);
            // Resolve font-size keywords "smaller" / "larger" relative to parent.
            let resolved = match &computed {
                ComputedValue::Keyword(kw) if kw.eq_ignore_ascii_case("smaller") => {
                    ComputedValue::Px(parent_font_size * 0.833)
                }
                ComputedValue::Keyword(kw) if kw.eq_ignore_ascii_case("larger") => {
                    ComputedValue::Px(parent_font_size * 1.2)
                }
                other => other.clone(),
            };
            properties.insert(PropertyId::FontSize, resolved);
            if fs_candidate.important {
                important_properties.insert("font-size".to_string());
            }
        }

        // For the root element, update root_font_size from its computed font-size
        // so that rem-based properties on the root itself resolve correctly.
        let mut root_font_size = root_font_size;
        if !self.root_font_size_explicit {
            let is_root = node
                .tag_name()
                .as_deref()
                .is_some_and(|t| t.eq_ignore_ascii_case("html"));
            if is_root && let Some(ComputedValue::Px(px)) = properties.get(&PropertyId::FontSize) {
                root_font_size = *px;
            }
        }

        let element_font_size = inherited_font_size(parent_style, &properties);
        let (font_properties, font_scope_root) =
            candidate_font_properties(&candidates, &custom_properties, parent_style, node);
        let element_font_metrics = if uses_font_metrics {
            self.css_font_metrics(&font_properties, font_scope_root, element_font_size)
        } else {
            CssRelativeFontMetrics::fallback(element_font_size, false)
        };
        // `lh` on line-height uses the parent's computed line-height, while
        // other properties use this element's resulting line-height.
        if let Some((candidate, value)) = candidates
            .iter()
            .rev()
            .filter(|candidate| candidate.name == "line-height")
            .find_map(|candidate| {
                resolve_cascaded_value(candidate, &custom_properties, node)
                    .map(|(value, _)| (candidate, value))
            })
        {
            let ctx = ResolutionContext {
                parent_font_size: element_font_size,
                root_font_size,
                line_height: parent_line_height,
                root_line_height,
                font_metrics: element_font_metrics,
                viewport_width: self.viewport_width,
                viewport_height: self.viewport_height,
                container_size: [None, None],
                vertical_writing: false,
            };
            properties.insert(
                PropertyId::LineHeight,
                compute_value(&value, "line-height", ctx),
            );
            if candidate.important {
                important_properties.insert("line-height".to_string());
            }
        }
        let element_line_height = used_line_height_value(
            properties
                .get(&PropertyId::LineHeight)
                .or_else(|| parent_style.and_then(|style| style.get("line-height"))),
            element_font_size,
        );
        let root_line_height = if node.tag_name().as_deref() == Some("html") {
            element_line_height
        } else {
            root_line_height
        };
        let element_flow =
            logical_flow_from_candidates(&candidates, &custom_properties, parent_style);
        let custom_ctx = ResolutionContext {
            parent_font_size: element_font_size,
            root_font_size,
            line_height: element_line_height,
            root_line_height,
            font_metrics: element_font_metrics,
            viewport_width: self.viewport_width,
            viewport_height: self.viewport_height,
            container_size: self.container_size(node),
            vertical_writing: element_flow.is_vertical(),
        };
        for (name, value) in &specified_custom_properties {
            record_component_value(&mut component_values, name, value);
        }
        let (computed_custom_properties, typed_custom_properties) =
            compute_registered_custom_properties(
                paint_custom_properties.unwrap_or(&specified_custom_properties),
                &inherited_custom_properties,
                &self.registered_custom_properties,
                custom_ctx,
                node,
            );
        custom_properties = computed_custom_properties;
        for (name, value) in typed_custom_properties {
            properties.insert(name, value);
        }
        // Pending shorthands may contain var() references whose registered
        // value changed after syntax validation or inheritance fallback.
        candidates = expand_pending_shorthand_candidates(unexpanded_candidates, &custom_properties);
        candidates.sort_by(compare_candidate_priority);
        let flow = logical_flow_from_candidates(&candidates, &custom_properties, parent_style);
        remove_reverted_candidates(&mut candidates, Some(&custom_properties), Some(flow));
        let logical_flow =
            logical_flow_from_candidates(&candidates, &custom_properties, parent_style);
        important_properties.extend(
            candidates
                .iter()
                .filter(|candidate| candidate.name.starts_with("--") && candidate.important)
                .map(|candidate| candidate.name.clone()),
        );

        for candidate in candidates {
            if matches!(candidate.name.as_str(), "font-size" | "line-height")
                || candidate.name.starts_with("--")
            {
                continue; // already processed above
            }
            log_unsupported_css_if_enabled(&candidate.name, &candidate.value);
            let Some((resolved_value, validation)) =
                resolve_cascaded_value(&candidate, &custom_properties, node)
            else {
                continue;
            };
            // Failed variable substitution computes as unset after the cascade.
            match validation {
                DeclarationValidation::Valid(computed) => {
                    record_component_value(&mut component_values, &candidate.name, &resolved_value);
                    if candidate.name.eq_ignore_ascii_case("animation-name") {
                        animation_name_scope_root = animation_reference_scope_root(
                            &resolved_value,
                            candidate.layer_context.scope_root,
                            parent_style,
                        );
                    }
                    if candidate.name.eq_ignore_ascii_case("font-family") {
                        font_family_scope_root = font_reference_scope_root(
                            &resolved_value,
                            candidate.layer_context.scope_root,
                            parent_style,
                        );
                    }
                    insert_computed_property(
                        &mut properties,
                        &candidate.name.to_ascii_lowercase(),
                        computed,
                        logical_flow,
                    );
                    if candidate.important {
                        important_properties.insert(candidate.name.to_ascii_lowercase());
                    }
                    continue;
                }
                DeclarationValidation::Invalid => continue,
                DeclarationValidation::Unvalidated => {}
            }
            let font_size = inherited_font_size(parent_style, &properties);
            let ctx = ResolutionContext {
                parent_font_size: font_size,
                root_font_size,
                line_height: element_line_height,
                root_line_height,
                font_metrics: element_font_metrics,
                viewport_width: self.viewport_width,
                viewport_height: self.viewport_height,
                container_size: self.container_size(node),
                vertical_writing: logical_flow.is_vertical(),
            };
            if candidate.name == "gap" || candidate.name == "grid-gap" {
                if let Some((row_gap, column_gap)) = compute_gap_shorthand(&resolved_value, ctx) {
                    insert_computed_property(&mut properties, "row-gap", row_gap, logical_flow);
                    insert_computed_property(
                        &mut properties,
                        "column-gap",
                        column_gap,
                        logical_flow,
                    );
                    if candidate.important {
                        important_properties.insert("row-gap".to_string());
                        important_properties.insert("column-gap".to_string());
                    }
                }
                continue;
            }
            if candidate.name == "grid-row-gap" || candidate.name == "grid-column-gap" {
                let target = if candidate.name == "grid-row-gap" {
                    "row-gap"
                } else {
                    "column-gap"
                };
                let computed = compute_value(&resolved_value, target, ctx);
                insert_computed_property(&mut properties, target, computed, logical_flow);
                if candidate.important {
                    important_properties.insert(target.to_string());
                }
                continue;
            }
            let computed = compute_value(&resolved_value, &candidate.name, ctx);
            record_component_value(&mut component_values, &candidate.name, &resolved_value);
            if candidate.name.eq_ignore_ascii_case("animation-name") {
                animation_name_scope_root = animation_reference_scope_root(
                    &resolved_value,
                    candidate.layer_context.scope_root,
                    parent_style,
                );
            }
            if candidate.name.eq_ignore_ascii_case("font-family") {
                font_family_scope_root = font_reference_scope_root(
                    &resolved_value,
                    candidate.layer_context.scope_root,
                    parent_style,
                );
            }
            insert_computed_property(&mut properties, &candidate.name, computed, logical_flow);
            if candidate.important {
                important_properties.insert(candidate.name.to_ascii_lowercase());
            }
        }

        apply_ua_defaults(node, &mut properties, pseudo, parent_style);
        apply_presentational_hints(node, &mut properties, pseudo);
        let color_inherits =
            forced_colors::capture_color_inheritance(&mut component_values, &properties);
        forced_colors::resolve_system_colors(&mut properties, &self.media_environment);
        resolve_current_color_on_color_property(&mut properties, parent_style);
        resolve_inherit_and_unset(&mut properties, parent_style);
        resolve_component_css_wide_keywords(&mut component_values, parent_style);
        apply_inheritance(&mut properties, parent_style);
        resolve_initial_css_wide_keywords(&mut properties);
        forced_colors::preserve_parent_color(
            node,
            pseudo.is_some(),
            parent_style,
            &mut properties,
            color_inherits,
            &self.media_environment,
        );
        apply_initial_values(&mut properties);
        resolve_accent_current_color(&mut properties);
        resolve_text_emphasis_style(&mut properties);
        properties.insert(
            "text-emphasis".to_string(),
            ComputedValue::Keyword(String::new()),
        );
        if pseudo.is_none() && node.node_type() == NodeType::Element {
            // The initial display value applies to elements without a more
            // specific UA or author declaration, including custom elements.
            properties
                .entry(PropertyId::Display)
                .or_insert_with(|| ComputedValue::Keyword("inline".to_string()));
        }
        resolve_column_rule_current_color(&mut properties);
        normalize_background_layer_lists(&mut properties);
        properties.insert(
            "transition".to_string(),
            ComputedValue::Keyword(super::computed_transition_shorthand(&properties)),
        );
        zero_border_width_for_none_style(&mut properties);
        // CSS Animations contribute below CSS Transitions in the cascade. The
        // transition compares and samples the animation-adjusted before/after
        // values, then its active value wins for the transitioned property.
        let animated_untyped = self.apply_animation_effect(
            node,
            pseudo,
            &mut properties,
            &important_properties,
            animation_name_scope_root,
            &custom_properties,
        );
        // Animated custom values participate in var() substitution and
        // inheritance. Re-cascade their dependants before transitions sample
        // the final animation-adjusted style; the override prevents recursion.
        if paint_custom_properties.is_none()
            && let Some(animated_custom) = animated_custom_property_values(
                &specified_custom_properties,
                &custom_properties,
                &properties,
                &self.registered_custom_properties,
                &animated_untyped,
            )
        {
            return self.compute_style_with_pseudo(
                node,
                parent_style,
                pseudo,
                Some(&animated_custom),
            );
        }
        if pseudo.is_none() {
            self.transition_timeline
                .sample(node.identity(), &mut properties);
        }

        forced_colors::compute_presentation_adjustments(
            &mut properties,
            &mut component_values,
            &self.media_environment,
        );
        resolve_outline_auto_color(&mut properties);
        let text_decorations = propagated_text_decorations(
            &properties,
            parent_style,
            font_family_scope_root,
            (node.identity(), pseudo),
        );
        let transform_animation_context = individual_transform::context::has_transform_animation(
            self,
            node,
            pseudo,
            animation_name_scope_root,
            &properties,
        );
        let font_feature_values = self.font_feature_values(font_family_scope_root);
        let shaping_options = Arc::new(computed_shaping_options(&properties, &font_feature_values));
        let font_synthesis = computed_font_synthesis(&properties);
        let font_palette =
            Arc::new(self.computed_font_palette(&properties, font_family_scope_root));
        ComputedStyle {
            properties,
            component_values,
            transform_animation_context,
            custom_properties,
            animation_name_scope_root,
            font_family_scope_root,
            shaping_options,
            font_synthesis,
            font_palette,
            text_decorations,
        }
    }

    fn font_feature_values(&self, scope_root: Option<usize>) -> FontFeatureValues {
        let mut values = FontFeatureValues::default();
        for origin in [Origin::UserAgent, Origin::User, Origin::Author] {
            for (input, scope) in self.stylesheets.iter().zip(&self.stylesheet_scopes) {
                if input.origin != origin
                    || scope
                        .root
                        .as_ref()
                        .is_some_and(|root| Some(root.identity()) != scope_root)
                {
                    continue;
                }
                collect_font_feature_values(
                    &input.stylesheet.rules,
                    &mut values,
                    self.viewport_width,
                    self.viewport_height,
                    &self.media_environment,
                );
            }
        }
        values
    }

    fn computed_font_palette(
        &self,
        properties: &PropertyMap,
        scope_root: Option<usize>,
    ) -> crate::font::FontPalette {
        let Some(ComputedValue::Keyword(name)) = properties.get(&PropertyId::FontPalette) else {
            return crate::font::FontPalette::default();
        };
        let families = match properties.get(&PropertyId::FontFamily) {
            Some(ComputedValue::Keyword(value) | ComputedValue::String(value)) => {
                FontFamilyKey::new(value).member_keys()
            }
            _ => Vec::new(),
        };
        let mut definitions = FontPaletteValues::default();
        for origin in [Origin::UserAgent, Origin::User, Origin::Author] {
            for (input, scope) in self.stylesheets.iter().zip(&self.stylesheet_scopes) {
                if input.origin != origin
                    || scope
                        .root
                        .as_ref()
                        .is_some_and(|root| Some(root.identity()) != scope_root)
                {
                    continue;
                }
                collect_font_palette_values(
                    &input.stylesheet.rules,
                    &mut definitions,
                    self.viewport_width,
                    self.viewport_height,
                    &self.media_environment,
                );
            }
        }
        let declaration = crate::css::parse_style_attribute(&format!("font-palette: {name}"));
        declaration
            .first()
            .and_then(|declaration| {
                resolve_font_palette_value(&declaration.value, &definitions, &families, 0)
            })
            .unwrap_or_default()
    }

    /// Applies a deterministic animation snapshot. Paused animations are
    /// sampled at timeline time zero, completed forwards/both animations keep
    /// their final state, and active animations are sampled at a
    /// fixed post-load instant so screenshots remain stable.
    fn apply_animation_snapshot(
        &self,
        node: &NodeHandle,
        properties: &mut PropertyMap,
        important_properties: &HashSet<String>,
        animation_name_scope_root: Option<usize>,
        custom_properties: &BTreeMap<String, Value>,
    ) -> BTreeMap<String, Value> {
        let anim_name = match properties.get(&PropertyId::AnimationName) {
            Some(ComputedValue::Keyword(name) | ComputedValue::String(name)) => name.clone(),
            _ => return BTreeMap::new(),
        };
        if anim_name.eq_ignore_ascii_case("none") || anim_name.is_empty() {
            return BTreeMap::new();
        }
        let Some(steps) = self.keyframes_for(node, animation_name_scope_root, &anim_name) else {
            return BTreeMap::new();
        };

        let resolved_steps = resolve_animation_keyframes(
            steps,
            node,
            custom_properties,
            &self.registered_custom_properties,
        );
        let steps = resolved_steps.as_slice();
        let Some(progress) = animation_snapshot_progress(properties) else {
            return BTreeMap::new();
        };
        let declarations = steps
            .iter()
            .rev()
            .find(|step| step.offset <= progress)
            .map(|step| &step.declarations);
        let Some(declarations) = declarations else {
            return BTreeMap::new();
        };
        let animation_progress = Some(progress);
        let shadow_base = animation::snapshot_shadow_base(steps, properties);
        let transform_base =
            ["translate", "rotate", "scale"].map(|name| properties.get(name).cloned());

        let element_font_size = properties
            .get(&PropertyId::FontSize)
            .and_then(|value| match value {
                ComputedValue::Px(px) => Some(*px),
                _ => None,
            })
            .unwrap_or(16.0);
        let ctx = ResolutionContext {
            parent_font_size: element_font_size,
            root_font_size: self.root_font_size,
            line_height: used_line_height_value(
                properties.get(&PropertyId::LineHeight),
                element_font_size,
            ),
            root_line_height: self.root_line_height(),
            font_metrics: CssRelativeFontMetrics::fallback(element_font_size, false),
            viewport_width: self.viewport_width,
            viewport_height: self.viewport_height,
            container_size: [None, None],
            vertical_writing: false,
        };
        let standard_properties: HashSet<&str> = declarations
            .iter()
            .filter(|declaration| !is_prefixed_property_alias(&declaration.name))
            .map(|declaration| canonical_property_name(&declaration.name))
            .collect();
        for declaration in declarations {
            let property_name = canonical_property_name(&declaration.name);
            if is_prefixed_property_alias(&declaration.name)
                && standard_properties.contains(property_name)
            {
                continue;
            }
            if important_properties.contains(property_name) {
                continue;
            }
            let resolved =
                resolve_value_with_custom_properties(&declaration.value, custom_properties)
                    .unwrap_or_else(|| declaration.value.clone());
            let resolved = if sibling_functions::contains(&resolved) {
                sibling_functions::resolve(&resolved, node)
            } else {
                resolved
            };
            let computed = compute_value(&resolved, property_name, ctx);
            let flow = logical_flow_from_properties(properties);
            insert_computed_property(properties, property_name, computed, flow);
        }
        if let Some(progress) = animation_progress {
            self.apply_registered_animation_interpolation(
                steps,
                progress,
                properties,
                ctx,
                custom_properties,
                important_properties,
            );
            individual_transform::snapshot::interpolate(
                steps,
                progress,
                properties,
                ctx,
                custom_properties,
                important_properties,
                transform_base,
            );
            self.interpolate_snapshot_text_shadow(
                steps,
                progress,
                properties,
                important_properties,
                shadow_base,
            );
        }
        let timing = properties
            .get(&PropertyId::AnimationTimingFunction)
            .map_or_else(|| "ease".into(), ComputedValue::css_text);
        animation::sample_untyped_custom_values(
            steps,
            progress,
            &timing,
            custom_properties,
            &self.registered_custom_properties,
            important_properties,
        )
    }

    fn apply_registered_animation_interpolation(
        &self,
        steps: &[KeyframeStep],
        progress: f32,
        properties: &mut PropertyMap,
        ctx: ResolutionContext,
        custom_properties: &BTreeMap<String, Value>,
        important_properties: &HashSet<String>,
    ) {
        for (name, registration) in &self.registered_custom_properties {
            if important_properties.contains(name) {
                continue;
            }
            let mut lower = None;
            let mut upper = None;
            for step in steps {
                if step.offset <= progress {
                    lower = Some(step);
                }
                if step.offset >= progress {
                    upper = Some(step);
                    break;
                }
            }
            let lower = lower.or_else(|| steps.first());
            let upper = upper.or_else(|| steps.last());
            let lower_value = lower.and_then(|step| {
                step.declarations
                    .iter()
                    .rev()
                    .find(|declaration| declaration.name == *name)
                    .map(|declaration| declaration.value.clone())
            });
            let upper_value = upper.and_then(|step| {
                step.declarations
                    .iter()
                    .rev()
                    .find(|declaration| declaration.name == *name)
                    .map(|declaration| declaration.value.clone())
            });
            let current = properties.get(name).cloned();
            let lower = lower_value
                .or_else(|| current.as_ref().map(computed_value_to_value))
                .and_then(|value| resolve_value_with_custom_properties(&value, custom_properties))
                .map(|value| compute_registered_value(&value, &registration.syntax, ctx));
            let upper = upper_value
                .or_else(|| current.as_ref().map(computed_value_to_value))
                .and_then(|value| resolve_value_with_custom_properties(&value, custom_properties))
                .map(|value| compute_registered_value(&value, &registration.syntax, ctx));
            let (Some(lower), Some(upper)) = (lower, upper) else {
                continue;
            };
            let lower_offset = steps
                .iter()
                .filter(|step| step.offset <= progress)
                .map(|step| step.offset)
                .next_back()
                .unwrap_or(0.0);
            let upper_offset = steps
                .iter()
                .find(|step| step.offset >= progress)
                .map(|step| step.offset)
                .unwrap_or(1.0);
            let span = if upper_offset > lower_offset {
                (progress - lower_offset) / (upper_offset - lower_offset)
            } else {
                0.0
            };
            let timing = properties
                .get(&PropertyId::AnimationTimingFunction)
                .map(ComputedValue::css_text)
                .unwrap_or_else(|| "ease".into());
            let span = super::transition::animation_timing_progress(&timing, span);
            // An exact endpoint keeps its computed representation, including
            // the color space serialization required by registered colors.
            if span <= 0.0 {
                properties.insert(name.clone(), lower);
                continue;
            }
            if span >= 1.0 {
                properties.insert(name.clone(), upper);
                continue;
            }
            if let Some(value) =
                super::transition::interpolate_custom_property(name, &lower, &upper, span)
            {
                properties.insert(
                    name.clone(),
                    normalize_registered_numeric(value, &registration.syntax),
                );
            }
        }
    }

    fn keyframes_for(
        &self,
        node: &NodeHandle,
        reference_scope_root: Option<usize>,
        animation_name: &str,
    ) -> Option<&[KeyframeStep]> {
        let mut scope_root = reference_scope_root;
        loop {
            if let Some(definition) = self
                .keyframes
                .get(&scope_root)
                .and_then(|definitions| definitions.get(animation_name))
            {
                return Some(&definition.steps);
            }
            let Some(scope_id) = scope_root else {
                return None;
            };
            let root = self
                .stylesheet_scopes
                .iter()
                .filter_map(|scope| scope.root.as_ref())
                .find(|root| root.identity() == scope_id)
                .cloned()
                .or_else(|| {
                    let mut root = node.containing_shadow_root();
                    while let Some(current) = root {
                        if current.identity() == scope_id {
                            return Some(current);
                        }
                        root = current
                            .shadow_host()
                            .and_then(|host| host.containing_shadow_root());
                    }
                    None
                });
            scope_root = root
                .and_then(|root| root.shadow_host())
                .and_then(|host| host.containing_shadow_root())
                .as_ref()
                .map(NodeHandle::identity);
        }
    }
}

fn record_component_value(
    values: &mut BTreeMap<String, Value>,
    property_name: &str,
    value: &Value,
) {
    let name = property_name.to_ascii_lowercase();
    if matches!(
        name.as_str(),
        "content" | "counter-reset" | "counter-increment" | "counter-set" | "quotes" | "color"
    ) || value_uses_container_units(value)
    {
        values.insert(name, value.clone());
    } else {
        values.remove(&name);
    }
}

/// Recognizes color expressions whose computed value depends on a query
/// container. Forced-colors handling uses this narrower classification.
fn color_uses_container_units(value: &Value) -> bool {
    matches!(value, Value::Function { name, .. }
        if matches!(name.to_ascii_lowercase().as_str(), "hwb" | "lab" | "lch" | "oklab" | "oklch" | "color")
            && value_uses_container_units(value))
}

fn value_uses_container_units(value: &Value) -> bool {
    match value {
        Value::Length(_, unit) => matches!(
            unit.to_ascii_lowercase().as_str(),
            "cqw" | "cqh" | "cqi" | "cqb" | "cqmin" | "cqmax"
        ),
        Value::Function { arguments, .. }
        | Value::List(arguments)
        | Value::CommaList(arguments) => arguments.iter().any(value_uses_container_units),
        _ => false,
    }
}

fn resolve_component_css_wide_keywords(
    values: &mut BTreeMap<String, Value>,
    parent_style: Option<&ComputedStyle>,
) {
    for name in [
        "content",
        "counter-reset",
        "counter-increment",
        "counter-set",
        "quotes",
    ] {
        let Some(Value::Keyword(keyword)) = values.get(name) else {
            continue;
        };
        if keyword.eq_ignore_ascii_case("inherit") {
            if let Some(inherited) = parent_style.and_then(|style| style.component_value(name)) {
                values.insert(name.to_string(), inherited.clone());
            } else {
                values.remove(name);
            }
        } else if is_css_wide_keyword(&keyword.to_ascii_lowercase()) {
            if name == "quotes" && keyword.eq_ignore_ascii_case("unset") {
                if let Some(inherited) = parent_style.and_then(|style| style.component_value(name))
                {
                    values.insert(name.to_string(), inherited.clone());
                } else {
                    values.remove(name);
                }
            } else {
                values.remove(name);
            }
        }
    }
    if !values.contains_key("quotes")
        && let Some(inherited) = parent_style.and_then(|style| style.component_value("quotes"))
    {
        values.insert("quotes".to_string(), inherited.clone());
    }
}

fn contains_at_rule_named(rules: &[Rule], expected: &str) -> bool {
    rules.iter().any(|rule| match rule {
        Rule::Style(style_rule) => contains_at_rule_named(&style_rule.rules, expected),
        Rule::At(at_rule) => {
            at_rule.name.eq_ignore_ascii_case(expected)
                || at_rule
                    .block
                    .as_deref()
                    .is_some_and(|block| contains_at_rule_named(block, expected))
        }
        Rule::FontFace(_) => false,
    })
}

fn animation_seconds(value: Option<&ComputedValue>) -> Option<f32> {
    match value {
        Some(ComputedValue::Number(value)) => Some(*value),
        _ => None,
    }
}

/// Resolves variables with their owned source scopes before evaluating this element's counts.
fn resolve_animation_keyframes(
    steps: &[KeyframeStep],
    node: &NodeHandle,
    custom_properties: &BTreeMap<String, Value>,
    registrations: &BTreeMap<String, RegisteredCustomProperty>,
) -> Vec<KeyframeStep> {
    steps
        .iter()
        .map(|step| KeyframeStep {
            offset: step.offset,
            declarations: step
                .declarations
                .iter()
                .map(|declaration| {
                    let value =
                        resolve_value_with_custom_properties(&declaration.value, custom_properties)
                            .unwrap_or_else(|| declaration.value.clone());
                    let untyped = declaration.name.starts_with("--")
                        && registrations
                            .get(&declaration.name)
                            .is_none_or(|registration| {
                                matches!(registration.syntax, RegisteredPropertySyntax::Universal)
                            });
                    let value = if !untyped && sibling_functions::contains(&value) {
                        sibling_functions::resolve(&value, node)
                    } else {
                        value
                    };
                    Declaration {
                        value,
                        ..declaration.clone()
                    }
                })
                .collect(),
        })
        .collect()
}

fn animation_snapshot_progress(properties: &PropertyMap) -> Option<f32> {
    let paused = matches!(properties.get(&PropertyId::AnimationPlayState),
        Some(ComputedValue::Keyword(value)) if value.eq_ignore_ascii_case("paused"));
    animation::snapshot_progress(
        properties,
        if paused {
            0.0
        } else {
            STATIC_ANIMATION_TIME_SECONDS
        },
    )
}

fn animation_reference_scope_root(
    value: &Value,
    declaration_scope_root: Option<usize>,
    parent_style: Option<&ComputedStyle>,
) -> Option<usize> {
    if matches!(value, Value::Keyword(keyword) if keyword.eq_ignore_ascii_case("inherit")) {
        return parent_style.and_then(|style| style.animation_name_scope_root);
    }
    declaration_scope_root
}

fn font_reference_scope_root(
    value: &Value,
    declaration_scope_root: Option<usize>,
    parent_style: Option<&ComputedStyle>,
) -> Option<usize> {
    if matches!(value, Value::Keyword(keyword) if keyword.eq_ignore_ascii_case("inherit") || keyword.eq_ignore_ascii_case("unset"))
    {
        return parent_style.and_then(ComputedStyle::font_family_scope_root);
    }
    declaration_scope_root
}

fn propagated_text_decorations(
    properties: &PropertyMap,
    parent_style: Option<&ComputedStyle>,
    font_family_scope_root: Option<usize>,
    origin: (usize, Option<PseudoElement>),
) -> Arc<[PropagatedTextDecoration]> {
    let interrupts_parent = matches!(
        properties.get(&PropertyId::Position),
        Some(ComputedValue::Keyword(value))
            if value.eq_ignore_ascii_case("absolute") || value.eq_ignore_ascii_case("fixed")
    ) || matches!(
        properties.get(&PropertyId::Float),
        Some(ComputedValue::Keyword(value)) if !value.eq_ignore_ascii_case("none")
    ) || matches!(
        properties.get(&PropertyId::Display),
        Some(ComputedValue::Keyword(value))
            if matches!(
                value.to_ascii_lowercase().as_str(),
                "inline-block" | "inline-table" | "inline-flex" | "inline-grid"
            )
    );
    let decorations = if interrupts_parent {
        Arc::default()
    } else {
        parent_style
            .map(|style| Arc::clone(&style.text_decorations))
            .unwrap_or_default()
    };
    if matches!(
        properties.get(&PropertyId::Display),
        Some(ComputedValue::Keyword(value)) if value.eq_ignore_ascii_case("contents")
    ) {
        return decorations;
    }
    let Some(ComputedValue::Keyword(line)) = properties.get(&PropertyId::TextDecorationLine) else {
        return decorations;
    };
    if !line.split_whitespace().any(|part| {
        matches!(
            part.to_ascii_lowercase().as_str(),
            "underline" | "overline" | "line-through"
        )
    }) {
        return decorations;
    }
    let color_value = properties
        .get(&PropertyId::TextDecorationColor)
        .map(computed_value_css_text)
        .unwrap_or_else(|| "currentcolor".to_string());
    let color = if color_value.eq_ignore_ascii_case("currentcolor") {
        properties
            .get(&PropertyId::Color)
            .map(computed_value_css_text)
            .unwrap_or_else(|| "black".to_string())
    } else {
        color_value
    };
    let mut extended = Vec::with_capacity(decorations.len() + 1);
    extended.extend(decorations.iter().cloned());
    extended.push(PropagatedTextDecoration {
        origin,
        line: line.clone(),
        color,
        style: properties
            .get(&PropertyId::TextDecorationStyle)
            .map(computed_value_css_text)
            .unwrap_or_else(|| "solid".to_string()),
        skip_ink: properties
            .get(&PropertyId::TextDecorationSkipInk)
            .map(computed_value_css_text)
            .unwrap_or_else(|| "auto".to_string()),
        thickness: properties
            .get(&PropertyId::TextDecorationThickness)
            .cloned()
            .unwrap_or_else(|| ComputedValue::Keyword("auto".to_string())),
        underline_position: properties
            .get(&PropertyId::TextUnderlinePosition)
            .map(computed_value_css_text)
            .unwrap_or_else(|| "auto".to_string()),
        underline_offset: properties
            .get(&PropertyId::TextUnderlineOffset)
            .cloned()
            .unwrap_or_else(|| ComputedValue::Keyword("auto".to_string())),
        font_size: properties
            .get(&PropertyId::FontSize)
            .and_then(|value| match value {
                ComputedValue::Px(value) => Some(*value),
                _ => None,
            })
            .unwrap_or(16.0),
        font_family: properties
            .get(&PropertyId::FontFamily)
            .and_then(|value| match value {
                ComputedValue::Keyword(value) | ComputedValue::String(value) => {
                    Some(crate::font::FontFamilyKey::new(value))
                }
                _ => None,
            }),
        font_weight: properties
            .get(&PropertyId::FontWeight)
            .and_then(|value| match value {
                ComputedValue::Keyword(value) | ComputedValue::String(value) => {
                    Some(crate::font::FontWeight::parse(value))
                }
                ComputedValue::Number(value) if value.is_finite() => Some(crate::font::FontWeight(
                    value.round().clamp(1.0, 1000.0) as u16,
                )),
                _ => None,
            })
            .unwrap_or_default(),
        font_style: properties
            .get(&PropertyId::FontStyle)
            .and_then(|value| match value {
                ComputedValue::Keyword(value) | ComputedValue::String(value) => {
                    Some(crate::font::FontStyleRange::parse(value).style)
                }
                _ => None,
            })
            .unwrap_or_default(),
        font_style_angle: properties
            .get(&PropertyId::FontStyle)
            .and_then(|value| match value {
                ComputedValue::Keyword(value) | ComputedValue::String(value) => {
                    Some(crate::font::FontStyleRange::parse(value).requested_angle())
                }
                _ => None,
            })
            .unwrap_or(0),
        font_stretch: properties
            .get(&PropertyId::FontStretch)
            .and_then(|value| match value {
                ComputedValue::Keyword(value) | ComputedValue::String(value) => {
                    Some(crate::font::FontStretch::parse(value))
                }
                ComputedValue::Percentage(value) => {
                    Some(crate::font::FontStretch::parse(&format!("{value}%")))
                }
                _ => None,
            })
            .unwrap_or_default(),
        font_scope_root: font_family_scope_root,
    });
    extended.into()
}

fn compute_gap_shorthand(
    value: &Value,
    ctx: ResolutionContext,
) -> Option<(ComputedValue, ComputedValue)> {
    match value {
        Value::List(values) => match values.as_slice() {
            [single] => {
                let computed = compute_value(single, "row-gap", ctx);
                if should_skip_computed_property("row-gap", &computed) {
                    None
                } else {
                    Some((computed.clone(), computed))
                }
            }
            [row, column] => {
                let row_gap = compute_value(row, "row-gap", ctx);
                let column_gap = compute_value(column, "column-gap", ctx);
                if should_skip_computed_property("row-gap", &row_gap)
                    || should_skip_computed_property("column-gap", &column_gap)
                {
                    None
                } else {
                    Some((row_gap, column_gap))
                }
            }
            _ => None,
        },
        _ => {
            let computed = compute_value(value, "row-gap", ctx);
            if should_skip_computed_property("row-gap", &computed) {
                None
            } else {
                Some((computed.clone(), computed))
            }
        }
    }
}

fn inherited_flow_keyword<'a>(
    parent: Option<&'a ComputedStyle>,
    name: &str,
    initial: &'a str,
) -> &'a str {
    match parent.and_then(|style| style.get(name)) {
        Some(ComputedValue::Keyword(value)) => value,
        _ => initial,
    }
}

/// Resolves a declaration while preserving invalid-at-computed-value-time winners.
fn resolve_cascaded_value(
    candidate: &Candidate,
    custom_properties: &BTreeMap<String, Value>,
    node: &NodeHandle,
) -> Option<(Value, DeclarationValidation)> {
    if !resolved_property_is_supported(&candidate.name) {
        return None;
    }
    let has_variables = value_contains_var_function(&candidate.value);
    let unset = || {
        (
            Value::Keyword("unset".into()),
            DeclarationValidation::Unvalidated,
        )
    };
    let Some(value) = resolve_value_with_custom_properties(&candidate.value, custom_properties)
    else {
        return has_variables.then(unset);
    };
    if matches!(
        validate_tree_counting_numeric_declaration(&candidate.name, &value),
        Some(DeclarationValidation::Invalid)
    ) {
        return has_variables.then(unset);
    }
    let value = if sibling_functions::contains(&value) {
        sibling_functions::resolve(&value, node)
    } else {
        value
    };
    let validation = validate_declaration(&candidate.name, &value);
    if matches!(validation, DeclarationValidation::Invalid) {
        return has_variables.then(unset);
    }
    Some((value, validation))
}

fn logical_flow_from_candidates(
    candidates: &[Candidate],
    custom_properties: &BTreeMap<String, Value>,
    parent: Option<&ComputedStyle>,
) -> super::logical::LogicalFlow {
    let inherited_direction = inherited_flow_keyword(parent, "direction", "ltr");
    let inherited_mode = inherited_flow_keyword(parent, "writing-mode", "horizontal-tb");
    let mut direction = inherited_direction;
    let mut mode = inherited_mode;
    for candidate in candidates {
        if !matches!(candidate.name.as_str(), "direction" | "writing-mode") {
            continue;
        }
        let Some(Value::Keyword(keyword)) =
            resolve_value_with_custom_properties(&candidate.value, custom_properties)
        else {
            continue;
        };
        if matches!(
            validate_declaration(&candidate.name, &Value::Keyword(keyword.clone())),
            DeclarationValidation::Invalid
        ) {
            continue;
        }
        let normalized = keyword.to_ascii_lowercase();
        let value = match normalized.as_str() {
            "inherit" | "unset" => {
                if candidate.name == "direction" {
                    inherited_direction
                } else {
                    inherited_mode
                }
            }
            "initial" => {
                if candidate.name == "direction" {
                    "ltr"
                } else {
                    "horizontal-tb"
                }
            }
            _ => {
                // Candidate values are borrowed only for this loop; use a
                // fixed keyword because the grammar has a closed set.
                match normalized.as_str() {
                    "rtl" => "rtl",
                    "ltr" => "ltr",
                    "vertical-rl" => "vertical-rl",
                    "vertical-lr" => "vertical-lr",
                    "sideways-rl" => "sideways-rl",
                    "sideways-lr" => "sideways-lr",
                    "horizontal-tb" => "horizontal-tb",
                    _ => continue,
                }
            }
        };
        if candidate.name == "direction" {
            direction = value;
        } else {
            mode = value;
        }
    }
    super::logical::LogicalFlow::new(mode, direction)
}

fn logical_flow_from_properties(properties: &PropertyMap) -> super::logical::LogicalFlow {
    let keyword = |id, default| match properties.get(&id) {
        Some(ComputedValue::Keyword(value)) => value.as_str(),
        _ => default,
    };
    super::logical::LogicalFlow::new(
        keyword(PropertyId::WritingMode, "horizontal-tb"),
        keyword(PropertyId::Direction, "ltr"),
    )
}

fn insert_computed_property(
    properties: &mut PropertyMap,
    name: &str,
    mut computed: ComputedValue,
    flow: super::logical::LogicalFlow,
) {
    if should_skip_computed_property(name, &computed) {
        return;
    }
    if name.starts_with("border-")
        && name.ends_with("-width")
        && let ComputedValue::Px(value) = &mut computed
    {
        *value = value.max(0.0);
    }
    // Logical and physical box properties participate in the same cascade.
    // Keep the logical value for CSSOM exposure, while also updating the
    // physical side consumed by layout and paint for the resolved flow.
    // Because candidates are inserted in cascade order, a later declaration
    // in either spelling correctly wins for layout.
    if let Some(physical_name) = flow.physical_name(name) {
        properties.insert(physical_name, computed.clone());
    }
    properties.insert(name, computed);
}

fn should_skip_computed_property(name: &str, computed: &ComputedValue) -> bool {
    // CSS 2.1: non-zero unitless numbers are invalid for length properties;
    // skip them so they don't override valid length values in the cascade.
    if matches!(computed, ComputedValue::Number(n) if *n != 0.0) && is_length_property(name) {
        return true;
    }

    // Enumerated properties: a keyword outside the property's valid set is an
    // invalid declaration and must be discarded by the cascade so it cannot
    // override an earlier valid declaration of the same property. Acid3 test 0
    // relies on `white-space: pre-wrap; white-space: x-bogus;` keeping the
    // `pre-wrap` value (the invalid `x-bogus` declaration is dropped).
    if let ComputedValue::Keyword(keyword) = computed
        && let Some(valid) = enumerated_keyword_set(name)
    {
        let lower = keyword.to_ascii_lowercase();
        // CSS-wide keywords are resolved in a later pass; never drop them.
        // `revert-layer` (CSS Cascade 5) is a CSS-wide keyword too and must
        // not be discarded by the enumerated-value validation.
        let is_css_wide = matches!(
            lower.as_str(),
            "inherit" | "initial" | "unset" | "revert" | "revert-layer" | "revert-rule"
        );
        if !is_css_wide && !valid.iter().any(|candidate| *candidate == lower) {
            return true;
        }
    }

    false
}

/// Returns the set of valid keyword values for an enumerated CSS property, or
/// `None` for properties that are not validated here.
///
/// Only properties whose invalid values Omoikane must actively discard during
/// the cascade are listed. Keeping the set small avoids accidentally dropping a
/// valid value that a property accepts but that is not enumerated here.
fn enumerated_keyword_set(name: &str) -> Option<&'static [&'static str]> {
    match name {
        "white-space" => Some(&[
            "normal",
            "pre",
            "nowrap",
            "pre-wrap",
            "pre-line",
            "break-spaces",
        ]),
        "text-overflow" => Some(&["clip", "ellipsis"]),
        _ => None,
    }
}

/// Outcome of validating a single declaration's value against a property's
/// grammar. Properties without a dedicated grammar report [`Self::Unvalidated`]
/// and fall through to the generic compute path unchanged, so introducing a new
/// validated property cannot alter the handling of existing ones.
enum DeclarationValidation {
    /// The declaration is valid; use this normalized computed value.
    Valid(ComputedValue),
    /// The declaration is invalid and must be dropped by the cascade (CSS error
    /// handling: an invalid declaration is ignored, so it neither applies nor
    /// blocks an earlier/later valid declaration of the same property).
    Invalid,
    /// The property has no dedicated grammar validation here.
    Unvalidated,
}

impl DeclarationValidation {
    fn map_keyword(self, map: impl FnOnce(String) -> String) -> Self {
        match self {
            Self::Valid(ComputedValue::Keyword(value)) => {
                Self::Valid(ComputedValue::Keyword(map(value)))
            }
            validation => validation,
        }
    }
}

fn validate_keyword_value(value: &Value, keywords: &[&str]) -> DeclarationValidation {
    let Value::Keyword(keyword) = value else {
        return DeclarationValidation::Invalid;
    };
    let keyword = keyword.to_ascii_lowercase();
    if is_css_wide_keyword(&keyword) || keywords.contains(&keyword.as_str()) {
        DeclarationValidation::Valid(ComputedValue::Keyword(keyword))
    } else {
        DeclarationValidation::Invalid
    }
}

fn is_supported_pointer_events_keyword(value: &str) -> bool {
    let value = value.trim();
    value.eq_ignore_ascii_case("auto")
        || value.eq_ignore_ascii_case("none")
        || value.eq_ignore_ascii_case("visiblepainted")
        || value.eq_ignore_ascii_case("visiblefill")
        || value.eq_ignore_ascii_case("visiblestroke")
        || value.eq_ignore_ascii_case("visible")
        || value.eq_ignore_ascii_case("painted")
        || value.eq_ignore_ascii_case("fill")
        || value.eq_ignore_ascii_case("stroke")
        || value.eq_ignore_ascii_case("bounding-box")
        || value.eq_ignore_ascii_case("all")
}

fn is_color_property(name: &str) -> bool {
    name.eq_ignore_ascii_case("color")
        || name.eq_ignore_ascii_case("background-color")
        || name.eq_ignore_ascii_case("border-color")
        || name.eq_ignore_ascii_case("border-top-color")
        || name.eq_ignore_ascii_case("border-right-color")
        || name.eq_ignore_ascii_case("border-bottom-color")
        || name.eq_ignore_ascii_case("border-left-color")
        || matches!(
            name,
            "border-inline-start-color"
                | "border-inline-end-color"
                | "border-block-start-color"
                | "border-block-end-color"
        )
        || name.eq_ignore_ascii_case("outline-color")
        || name.eq_ignore_ascii_case("column-rule-color")
        || name.eq_ignore_ascii_case("text-decoration-color")
        || name.eq_ignore_ascii_case("text-emphasis-color")
}

pub(crate) fn is_valid_text_emphasis_style_component(value: &Value) -> bool {
    matches!(value, Value::String(_))
        || matches!(value, Value::Keyword(keyword) if matches!(
            keyword.to_ascii_lowercase().as_str(),
            "none" | "filled" | "open" | "dot" | "circle" | "double-circle" | "triangle" | "sesame"
        ))
}

pub(crate) fn text_emphasis_style_is_valid(value: &Value) -> bool {
    let values = match value {
        Value::List(values) => values.as_slice(),
        value => std::slice::from_ref(value),
    };
    if values.len() == 1 {
        return is_valid_text_emphasis_style_component(&values[0]);
    }
    if values.len() != 2 {
        return false;
    }
    let mut fill = false;
    let mut shape = false;
    values.iter().all(|value| match value {
        Value::Keyword(keyword)
            if matches!(keyword.to_ascii_lowercase().as_str(), "filled" | "open") && !fill =>
        {
            fill = true;
            true
        }
        Value::Keyword(keyword)
            if matches!(
                keyword.to_ascii_lowercase().as_str(),
                "dot" | "circle" | "double-circle" | "triangle" | "sesame"
            ) && !shape =>
        {
            shape = true;
            true
        }
        _ => false,
    })
}

fn validate_text_emphasis_style(value: &Value) -> DeclarationValidation {
    if let Value::Keyword(keyword) = value
        && is_css_wide_keyword(&keyword.to_ascii_lowercase())
    {
        return DeclarationValidation::Unvalidated;
    }
    if !text_emphasis_style_is_valid(value) {
        return DeclarationValidation::Invalid;
    }
    let values = match value {
        Value::List(values) => values.as_slice(),
        value => std::slice::from_ref(value),
    };
    if let [Value::String(value)] = values {
        return DeclarationValidation::Valid(ComputedValue::String(value.clone()));
    }
    if let [Value::Keyword(keyword)] = values
        && matches!(keyword.to_ascii_lowercase().as_str(), "none")
    {
        return DeclarationValidation::Valid(ComputedValue::Keyword("none".into()));
    }
    let fill = values.iter().find_map(|value| match value {
        Value::Keyword(keyword)
            if matches!(keyword.to_ascii_lowercase().as_str(), "filled" | "open") =>
        {
            Some(keyword.to_ascii_lowercase())
        }
        _ => None,
    });
    let shape = values.iter().find_map(|value| match value {
        Value::Keyword(keyword)
            if matches!(
                keyword.to_ascii_lowercase().as_str(),
                "dot" | "circle" | "double-circle" | "triangle" | "sesame"
            ) =>
        {
            Some(keyword.to_ascii_lowercase())
        }
        _ => None,
    });
    let canonical = match (fill.as_deref(), shape.as_deref()) {
        (Some("open"), Some(shape)) => format!("open {shape}"),
        (_, Some(shape)) => shape.to_string(),
        (Some(fill), None) => fill.to_string(),
        (None, None) => return DeclarationValidation::Invalid,
    };
    DeclarationValidation::Valid(ComputedValue::Keyword(canonical))
}

fn resolve_text_emphasis_style(properties: &mut PropertyMap) {
    let vertical = matches!(
        properties.get(&PropertyId::WritingMode),
        Some(ComputedValue::Keyword(mode)) if mode != "horizontal-tb"
    );
    let resolved = match properties.get(&PropertyId::TextEmphasisStyle) {
        Some(ComputedValue::Keyword(style)) if style == "filled" && vertical => "sesame",
        Some(ComputedValue::Keyword(style)) if style == "filled" => "circle",
        Some(ComputedValue::Keyword(style)) if style == "open" && vertical => "open sesame",
        Some(ComputedValue::Keyword(style)) if style == "open" => "open circle",
        _ => return,
    };
    properties.insert(
        PropertyId::TextEmphasisStyle,
        ComputedValue::Keyword(resolved.into()),
    );
}

fn keyword_components(value: &Value) -> Option<Vec<String>> {
    let values = match value {
        Value::List(values) => values.as_slice(),
        value => std::slice::from_ref(value),
    };
    values
        .iter()
        .map(|value| match value {
            Value::Keyword(keyword) => Some(keyword.to_ascii_lowercase()),
            _ => None,
        })
        .collect()
}

fn validate_keyword_groups(
    value: &Value,
    groups: &[&[&str]],
    singular: &[&str],
) -> DeclarationValidation {
    let Some(keywords) = keyword_components(value) else {
        return DeclarationValidation::Invalid;
    };
    if keywords.len() == 1
        && (is_css_wide_keyword(&keywords[0]) || singular.contains(&keywords[0].as_str()))
    {
        return DeclarationValidation::Valid(ComputedValue::Keyword(keywords[0].clone()));
    }
    if keywords.is_empty()
        || keywords
            .iter()
            .any(|keyword| singular.contains(&keyword.as_str()))
    {
        return DeclarationValidation::Invalid;
    }
    let mut selected = vec![None; groups.len()];
    for keyword in keywords {
        let Some(group) = groups
            .iter()
            .position(|group| group.contains(&keyword.as_str()))
        else {
            return DeclarationValidation::Invalid;
        };
        if selected[group].replace(keyword).is_some() {
            return DeclarationValidation::Invalid;
        }
    }
    DeclarationValidation::Valid(ComputedValue::Keyword(
        selected.into_iter().flatten().collect::<Vec<_>>().join(" "),
    ))
}

fn validate_text_combine_upright(value: &Value) -> DeclarationValidation {
    match value {
        Value::Keyword(_) => {
            validate_keyword_value(value, &["none", "all", "digits"]).map_keyword(|value| {
                if value == "digits" {
                    "digits 2".into()
                } else {
                    value
                }
            })
        }
        Value::List(values)
            if matches!(values.as_slice(), [Value::Keyword(keyword), Value::Number(number)]
                if keyword.eq_ignore_ascii_case("digits")
                    && number.fract() == 0.0
                    && (2.0..=4.0).contains(number)) =>
        {
            let Value::Number(number) = values[1] else {
                unreachable!()
            };
            DeclarationValidation::Valid(ComputedValue::Keyword(format!("digits {}", number as u8)))
        }
        _ => DeclarationValidation::Invalid,
    }
}

/// Checks runtime color channels structurally without inventing DOM values.
fn runtime_color_channels_are_valid(value: &Value) -> bool {
    use typed_math::NumericType as T;
    let Value::Function { name, arguments } = value else {
        return false;
    };
    let name = name.to_ascii_lowercase();
    let values = match arguments.as_slice() {
        [Value::List(values)] => values.as_slice(),
        values => values,
    };
    let (values, hue) = if name == "color" {
        let Some((Value::Keyword(profile), values)) = values.split_first() else {
            return false;
        };
        if !matches!(
            profile.to_ascii_lowercase().as_str(),
            "srgb"
                | "srgb-linear"
                | "display-p3"
                | "a98-rgb"
                | "prophoto-rgb"
                | "rec2020"
                | "xyz"
                | "xyz-d50"
                | "xyz-d65"
        ) {
            return false;
        }
        (values, None)
    } else {
        let hue = match name.as_str() {
            "rgb" | "rgba" | "lab" | "oklab" => None,
            "lch" | "oklch" => Some(2),
            "hsl" | "hsla" | "hwb" => Some(0),
            _ => return false,
        };
        (values, hue)
    };
    let valid = |value: &Value, angular: bool| {
        if matches!(value, Value::Keyword(name) if name.eq_ignore_ascii_case("none")) {
            return true;
        }
        if angular {
            matches!(typed_math::infer(value, None), Some(T::NUMBER | T::ANGLE))
        } else {
            typed_math::infer(value, Some(T::NUMBER)) == Some(T::NUMBER)
        }
    };
    if !matches!(values.len(), 3 | 5) {
        return false;
    }
    if !values[..3]
        .iter()
        .enumerate()
        .all(|(index, value)| valid(value, hue == Some(index)))
    {
        return false;
    }
    values.len() == 3
        || (matches!(&values[3], Value::Keyword(name) if name == "/") && valid(&values[4], false))
}

fn validate_color_value(value: &Value) -> DeclarationValidation {
    if sibling_functions::contains(value) {
        return if runtime_color_channels_are_valid(value) {
            DeclarationValidation::Unvalidated
        } else {
            DeclarationValidation::Invalid
        };
    }
    if let Value::Keyword(keyword) = value {
        let lower = keyword.to_ascii_lowercase();
        if is_css_wide_keyword(&lower) || lower == "currentcolor" {
            return DeclarationValidation::Unvalidated;
        }
    }

    let valid = match value {
        Value::Keyword(color) | Value::Color(color) => is_valid_css_color_text(color),
        Value::Function { .. } => is_valid_css_color_text(&render_value(value)),
        _ => false,
    };
    if valid {
        DeclarationValidation::Unvalidated
    } else {
        DeclarationValidation::Invalid
    }
}

/// Validates Color 4 syntax without performing conversion or gamut mapping.
fn is_valid_css_color_text(text: &str) -> bool {
    crate::css::ForcedColorPalette::for_color_scheme(false)
        .system_color(text)
        .is_some()
        || crate::paint::color4::CssColor::parse(text).is_some()
        || crate::paint::color::parse_color(text).is_some()
}

pub(super) fn is_valid_color_value(value: &Value) -> bool {
    !matches!(validate_color_value(value), DeclarationValidation::Invalid)
}

fn validate_multicol_declaration(name: &str, value: &Value) -> Option<DeclarationValidation> {
    let property_name = name;
    let keyword = |allowed: &[&str]| match value {
        Value::Keyword(keyword) => {
            let lower = keyword.to_ascii_lowercase();
            if is_css_wide_keyword(&lower) || allowed.contains(&lower.as_str()) {
                DeclarationValidation::Valid(ComputedValue::Keyword(lower))
            } else {
                DeclarationValidation::Invalid
            }
        }
        _ => DeclarationValidation::Invalid,
    };
    let non_negative_length = || match value {
        Value::Keyword(keyword) if is_css_wide_keyword(&keyword.to_ascii_lowercase()) => {
            DeclarationValidation::Valid(ComputedValue::Keyword(keyword.to_ascii_lowercase()))
        }
        Value::Length(number, unit)
            if *number >= 0.0
                && resolve_length_to_px(*number, unit, ResolutionContext::default()).is_some() =>
        {
            DeclarationValidation::Unvalidated
        }
        Value::Number(number) if *number == 0.0 => {
            DeclarationValidation::Valid(ComputedValue::Px(0.0))
        }
        Value::Function { name: function, .. } if is_length_percentage_math_function(function) => {
            match compute_value(value, property_name, ResolutionContext::default()) {
                ComputedValue::Px(number) if number >= 0.0 => DeclarationValidation::Unvalidated,
                _ => DeclarationValidation::Invalid,
            }
        }
        _ => DeclarationValidation::Invalid,
    };

    Some(match name.to_ascii_lowercase().as_str() {
        "column-count" => match value {
            Value::Keyword(keyword) => {
                let lower = keyword.to_ascii_lowercase();
                if is_css_wide_keyword(&lower) || lower == "auto" {
                    DeclarationValidation::Valid(ComputedValue::Keyword(lower))
                } else {
                    DeclarationValidation::Invalid
                }
            }
            Value::Number(number)
                if number.is_finite() && *number >= 1.0 && number.fract() == 0.0 =>
            {
                DeclarationValidation::Valid(ComputedValue::Number(*number))
            }
            Value::Function { name, arguments } if name.eq_ignore_ascii_case("calc") => {
                match evaluate_calc(arguments, ResolutionContext::default()) {
                    Some(quantity)
                        if quantity.unit == CalcUnit::Unitless
                            && quantity.value >= 1.0
                            && quantity.value.fract() == 0.0 =>
                    {
                        DeclarationValidation::Valid(ComputedValue::Number(quantity.value))
                    }
                    _ => DeclarationValidation::Invalid,
                }
            }
            _ => DeclarationValidation::Invalid,
        },
        "column-width" => match value {
            Value::Keyword(keyword) if keyword.eq_ignore_ascii_case("auto") => {
                DeclarationValidation::Valid(ComputedValue::Keyword("auto".to_string()))
            }
            _ => non_negative_length(),
        },
        "column-fill" => keyword(&["auto", "balance", "balance-all"]),
        "column-span" => keyword(&["none", "all"]),
        "column-rule-style" => keyword(&[
            "none", "hidden", "dotted", "dashed", "solid", "double", "groove", "ridge", "inset",
            "outset",
        ]),
        "column-rule-width" => match value {
            Value::Keyword(keyword)
                if matches!(
                    keyword.to_ascii_lowercase().as_str(),
                    "thin" | "medium" | "thick"
                ) =>
            {
                DeclarationValidation::Unvalidated
            }
            _ => non_negative_length(),
        },
        "column-gap" => match value {
            Value::Keyword(keyword) if keyword.eq_ignore_ascii_case("normal") => {
                DeclarationValidation::Valid(ComputedValue::Keyword("normal".to_string()))
            }
            _ => non_negative_length(),
        },
        "break-before" | "break-after" => keyword(&[
            "auto",
            "avoid",
            "avoid-page",
            "page",
            "left",
            "right",
            "recto",
            "verso",
            "avoid-column",
            "column",
            "avoid-region",
            "region",
        ]),
        "break-inside" => keyword(&[
            "auto",
            "avoid",
            "avoid-page",
            "avoid-column",
            "avoid-region",
        ]),
        "page" => match value {
            Value::Keyword(name) if is_css_wide_keyword(&name.to_ascii_lowercase()) => {
                DeclarationValidation::Unvalidated
            }
            Value::Keyword(name)
                if name.eq_ignore_ascii_case("auto")
                    || (!name.is_empty()
                        && !name.chars().next().is_some_and(|ch| ch.is_ascii_digit())
                        && name
                            .chars()
                            .all(|ch| ch.is_alphanumeric() || matches!(ch, '-' | '_'))) =>
            {
                DeclarationValidation::Valid(ComputedValue::Keyword(name.clone()))
            }
            _ => DeclarationValidation::Invalid,
        },
        "box-decoration-break" => keyword(&["slice", "clone"]),
        "orphans" | "widows" => match value {
            Value::Keyword(keyword) if is_css_wide_keyword(&keyword.to_ascii_lowercase()) => {
                DeclarationValidation::Valid(ComputedValue::Keyword(keyword.to_ascii_lowercase()))
            }
            Value::Number(number)
                if number.is_finite() && *number >= 1.0 && number.fract() == 0.0 =>
            {
                DeclarationValidation::Valid(ComputedValue::Number(*number))
            }
            _ => DeclarationValidation::Invalid,
        },
        _ => return None,
    })
}

// Feature detection and the cascade share the implemented-property registry.
// Recognition does not bypass value validation for retained shorthand metadata.
fn resolved_property_is_supported(name: &str) -> bool {
    is_supported_property(canonical_property_name(name)) || name.starts_with("--")
}

fn validate_retained_shorthand(name: &str, value: &Value) -> DeclarationValidation {
    let expanded = super::shorthand::expand_shorthand(name, value.clone(), false);
    let longhands: Vec<_> = expanded
        .iter()
        .filter(|declaration| declaration.name != name)
        .collect();
    if longhands.is_empty()
        || longhands.iter().any(|declaration| {
            matches!(
                validate_declaration(&declaration.name, &declaration.value),
                DeclarationValidation::Invalid
            )
        })
    {
        return DeclarationValidation::Invalid;
    }
    // Some expansion paths retain a shorthand alongside its longhands for
    // internal consumers. Keep simple typed values, and serialize every token
    // of a valid aggregate rather than computing only its first component.
    match value {
        Value::List(_) | Value::CommaList(_) => {
            DeclarationValidation::Valid(ComputedValue::Keyword(render_value(value)))
        }
        _ => DeclarationValidation::Unvalidated,
    }
}

/// Validates and serializes numeric declarations containing tree-counting functions.
pub(crate) fn normalize_tree_counting_numeric(property: &str, text: &str) -> Option<String> {
    if !text
        .as_bytes()
        .windows(7)
        .any(|word| word.eq_ignore_ascii_case(b"sibling"))
    {
        return None;
    }
    let value = super::parse_style_attribute(&format!("{property}: {text}"))
        .into_iter()
        .find(|declaration| declaration.name == property)?
        .value;
    if !sibling_functions::contains(&value) {
        return None;
    }
    match validate_tree_counting_numeric_declaration(property, &value)? {
        DeclarationValidation::Invalid => None,
        _ => Some(render_value(&canonical_tree_counting_numeric(&value))),
    }
}

/// Folds context-free number math for specified-value CSSOM serialization.
pub(crate) fn normalize_numeric_math_specified(property: &str, text: &str) -> Option<String> {
    if !supports_declaration(property, text) {
        return None;
    }
    if is_color_property(property)
        && let Some(color) = crate::paint::color4::CssColor::parse(text)
    {
        return Some(color.serialize());
    }
    let value = super::parse_style_attribute(&format!("{property}: {text}"))
        .into_iter()
        .find(|declaration| declaration.name == property)?
        .value;

    fn number(value: f32) -> String {
        if value.is_nan() {
            "NaN".into()
        } else if value == f32::INFINITY {
            "infinity".into()
        } else if value == f32::NEG_INFINITY {
            "-infinity".into()
        } else {
            super::tokenizer::trimmed_number(value)
        }
    }

    fn canonical(value: &Value) -> (String, bool) {
        match value {
            Value::Function { name, arguments } => {
                if let Some(result) = individual_transform::fixed_unclamped_number_expression(value)
                {
                    return (format!("calc({})", number(result)), true);
                }
                if let Some(result) = individual_transform::fixed_percentage_expression(value) {
                    return (format!("calc({}%)", number(result)), true);
                }
                if let Some(Value::Length(result, unit)) =
                    individual_transform::fixed_angle_expression(value)
                {
                    return (format!("calc({}{unit})", number(result)), true);
                }
                let canonical = arguments.iter().map(canonical).collect::<Vec<_>>();
                let changed = canonical.iter().any(|(_, changed)| *changed);
                let arguments = canonical
                    .into_iter()
                    .map(|(value, _)| value)
                    .collect::<Vec<_>>()
                    .join(if name.eq_ignore_ascii_case("url") {
                        ","
                    } else {
                        ", "
                    });
                (format!("{name}({arguments})"), changed)
            }
            Value::List(values) => {
                let canonical = values.iter().map(canonical).collect::<Vec<_>>();
                let changed = canonical.iter().any(|(_, changed)| *changed);
                (
                    canonical
                        .into_iter()
                        .map(|(value, _)| value)
                        .collect::<Vec<_>>()
                        .join(" "),
                    changed,
                )
            }
            Value::CommaList(values) => {
                let canonical = values.iter().map(canonical).collect::<Vec<_>>();
                let changed = canonical.iter().any(|(_, changed)| *changed);
                (
                    canonical
                        .into_iter()
                        .map(|(value, _)| value)
                        .collect::<Vec<_>>()
                        .join(", "),
                    changed,
                )
            }
            _ => (render_value(value), false),
        }
    }

    let (value, changed) = canonical(&value);
    changed.then_some(value)
}

fn canonical_tree_counting_numeric(value: &Value) -> Value {
    match value {
        Value::Length(number, unit) if unit.eq_ignore_ascii_case("ms") => {
            Value::Length(number / 1000.0, "s".into())
        }
        Value::Length(number, unit) if unit.eq_ignore_ascii_case("turn") => {
            Value::Length(number * 360.0, "deg".into())
        }
        Value::Function { name, arguments } => {
            let mut arguments: Vec<_> = arguments
                .iter()
                .map(canonical_tree_counting_numeric)
                .collect();
            if name.eq_ignore_ascii_case("calc") && arguments.len() == 1 {
                if let Some(folded) = specified_math::fold_fixed_angle_product(&arguments[0]) {
                    arguments[0] = folded;
                }
            }
            if name.eq_ignore_ascii_case("calc")
                && arguments.len() == 1
                && matches!(&arguments[0], Value::Function { name, arguments } if arguments.is_empty() && matches!(name.as_str(), "sibling-index" | "sibling-count"))
            {
                return arguments.into_iter().next().unwrap();
            }
            Value::Function {
                name: name.clone(),
                arguments,
            }
        }
        Value::List(values) => {
            Value::List(values.iter().map(canonical_tree_counting_numeric).collect())
        }
        Value::CommaList(values) => {
            Value::CommaList(values.iter().map(canonical_tree_counting_numeric).collect())
        }
        _ => value.clone(),
    }
}

/// Checks element-dependent numeric types without evaluating DOM placeholders.
fn validate_tree_counting_numeric_declaration(
    name: &str,
    value: &Value,
) -> Option<DeclarationValidation> {
    use typed_math::NumericType as T;
    if !sibling_functions::contains(value) || name.starts_with("--") {
        return None;
    }
    if !sibling_functions::valid_arguments(value) {
        return Some(DeclarationValidation::Invalid);
    }
    let name = name.to_ascii_lowercase();
    if let Some(validation) = validate_complex_font_numeric_declaration(&name, value) {
        return Some(validation);
    }
    if name == "transform" {
        return Some(if generic_transform::valid(value) {
            DeclarationValidation::Unvalidated
        } else {
            DeclarationValidation::Invalid
        });
    }
    if name == "line-height" {
        let numeric_type = typed_math::infer(value, Some(T::LENGTH));
        let valid = matches!(numeric_type, Some(kind) if kind == T::NUMBER || kind == T::LENGTH);
        return Some(if valid {
            DeclarationValidation::Unvalidated
        } else {
            DeclarationValidation::Invalid
        });
    }
    let expected = match name.as_str() {
        "flex-grow" | "flex-shrink" | "opacity" | "z-index" | "order" | "column-count"
        | "font-weight" | "orphans" | "widows" | "tab-size" | "scale" => T::NUMBER,
        "rotate" => T::ANGLE,
        "animation-duration" | "animation-delay" | "transition-duration" | "transition-delay" => {
            T::TIME
        }
        "width"
        | "height"
        | "min-width"
        | "max-width"
        | "min-height"
        | "max-height"
        | "inline-size"
        | "block-size"
        | "min-inline-size"
        | "max-inline-size"
        | "min-block-size"
        | "max-block-size"
        | "left"
        | "right"
        | "top"
        | "bottom"
        | "font-size"
        | "column-width"
        | "row-gap"
        | "column-gap"
        | "text-indent"
        | "flex-basis"
        | "perspective"
        | "text-underline-offset"
        | "text-decoration-thickness" => T::LENGTH,
        _ if name.starts_with("padding-")
            || name.starts_with("margin-")
            || name.starts_with("inset-")
            || (name.starts_with("border-") && name.ends_with("-width")) =>
        {
            T::LENGTH
        }
        _ => return None,
    };
    let hint = (expected == T::LENGTH).then_some(T::LENGTH);
    let valid = match value {
        Value::CommaList(values) if expected == T::TIME => {
            !values.is_empty()
                && values
                    .iter()
                    .all(|value| typed_math::infer(value, hint) == Some(expected))
        }
        _ => typed_math::infer(value, hint) == Some(expected),
    };
    Some(if valid {
        DeclarationValidation::Unvalidated
    } else {
        DeclarationValidation::Invalid
    })
}

/// Checks compound font grammars using types, without numerical placeholders.
fn validate_complex_font_numeric_declaration(
    name: &str,
    value: &Value,
) -> Option<DeclarationValidation> {
    use typed_math::NumericType as T;
    let valid = match name {
        "font-style" => match value {
            Value::Keyword(word) => {
                is_css_wide_keyword(word)
                    || matches!(
                        word.to_ascii_lowercase().as_str(),
                        "normal" | "italic" | "oblique"
                    )
            }
            Value::List(values) => matches!(values.as_slice(), [Value::Keyword(word), angle]
                if word.eq_ignore_ascii_case("oblique") && typed_math::infer(angle, None) == Some(T::ANGLE)),
            _ => false,
        },
        "font-feature-settings" | "font-variation-settings" => {
            fn valid_setting(value: &Value, feature: bool) -> bool {
                let Value::List(values) = value else {
                    return feature
                        && matches!(value, Value::String(tag)
                            if tag.len() == 4
                                && tag.bytes().all(|byte| (0x20..=0x7e).contains(&byte)));
                };
                match values.as_slice() {
                    [Value::String(tag)] if feature => {
                        tag.len() == 4 && tag.bytes().all(|byte| (0x20..=0x7e).contains(&byte))
                    }
                    [Value::String(tag), amount] => {
                        let valid_amount = if feature {
                            match amount {
                                Value::Keyword(keyword) => {
                                    keyword.eq_ignore_ascii_case("on")
                                        || keyword.eq_ignore_ascii_case("off")
                                }
                                Value::Number(number) => {
                                    number.is_finite() && *number >= 0.0 && number.fract() == 0.0
                                }
                                amount => typed_math::infer(amount, None) == Some(T::NUMBER),
                            }
                        } else {
                            typed_math::infer(amount, None) == Some(T::NUMBER)
                        };
                        tag.len() == 4
                            && tag.bytes().all(|byte| (0x20..=0x7e).contains(&byte))
                            && valid_amount
                    }
                    _ => false,
                }
            }
            let feature = name == "font-feature-settings";
            match value {
                Value::Keyword(word) => {
                    word.eq_ignore_ascii_case("normal") || is_css_wide_keyword(word)
                }
                Value::CommaList(values) => {
                    !values.is_empty() && values.iter().all(|value| valid_setting(value, feature))
                }
                value => valid_setting(value, feature),
            }
        }
        _ => return None,
    };
    Some(if valid {
        DeclarationValidation::Unvalidated
    } else {
        DeclarationValidation::Invalid
    })
}

/// Checks font numeric grammars even after registered values have computed.
fn validate_font_numeric_declaration(name: &str, value: &Value) -> Option<DeclarationValidation> {
    use typed_math::NumericType as T;
    let is_size = name.eq_ignore_ascii_case("font-size");
    if !is_size && !name.eq_ignore_ascii_case("line-height") {
        return None;
    }
    let valid = match value {
        Value::Keyword(keyword) => {
            let keyword = keyword.to_ascii_lowercase();
            is_css_wide_keyword(&keyword)
                || if is_size {
                    matches!(
                        keyword.as_str(),
                        "xx-small"
                            | "x-small"
                            | "small"
                            | "medium"
                            | "large"
                            | "x-large"
                            | "xx-large"
                            | "xxx-large"
                            | "smaller"
                            | "larger"
                            | "math"
                    )
                } else {
                    keyword == "normal"
                }
        }
        Value::Number(number) => {
            number.is_finite()
                && if is_size {
                    *number == 0.0
                } else {
                    *number >= 0.0
                }
        }
        Value::Length(number, _) | Value::Percentage(number)
            if !number.is_finite() || *number < 0.0 =>
        {
            false
        }
        _ => matches!(typed_math::infer(value, Some(T::LENGTH)), Some(kind)
            if kind == T::LENGTH || (!is_size && kind == T::NUMBER)),
    };
    Some(if valid {
        DeclarationValidation::Unvalidated
    } else {
        DeclarationValidation::Invalid
    })
}

fn validate_font_variant_declaration(name: &str, value: &Value) -> Option<DeclarationValidation> {
    if name == "font-variant-alternates" {
        return Some(validate_font_variant_alternates(value));
    }
    let allowed: &[&str] = match name {
        "font-kerning" => &["auto", "normal", "none"],
        "font-optical-sizing" => &["auto", "none"],
        "font-synthesis" => &[
            "none",
            "weight",
            "style",
            "oblique-only",
            "small-caps",
            "position",
        ],
        "font-variant-caps" => &[
            "normal",
            "small-caps",
            "all-small-caps",
            "petite-caps",
            "all-petite-caps",
            "unicase",
            "titling-caps",
        ],
        "font-variant-east-asian" => &[
            "normal",
            "jis78",
            "jis83",
            "jis90",
            "jis04",
            "simplified",
            "traditional",
            "full-width",
            "proportional-width",
            "ruby",
        ],
        "font-variant-ligatures" => &[
            "normal",
            "none",
            "common-ligatures",
            "no-common-ligatures",
            "discretionary-ligatures",
            "no-discretionary-ligatures",
            "historical-ligatures",
            "no-historical-ligatures",
            "contextual",
            "no-contextual",
        ],
        "font-variant-numeric" => &[
            "normal",
            "lining-nums",
            "oldstyle-nums",
            "proportional-nums",
            "tabular-nums",
            "diagonal-fractions",
            "stacked-fractions",
            "ordinal",
            "slashed-zero",
        ],
        "font-variant-position" => &["normal", "sub", "super"],
        _ => return None,
    };
    let words = match value {
        Value::Keyword(word) => vec![word.to_ascii_lowercase()],
        Value::List(values) => values
            .iter()
            .map(|value| match value {
                Value::Keyword(word) => Some(word.to_ascii_lowercase()),
                _ => None,
            })
            .collect::<Option<Vec<_>>>()
            .unwrap_or_default(),
        _ => Vec::new(),
    };
    let css_wide = words.len() == 1 && is_css_wide_keyword(&words[0]);
    let unique = words.iter().collect::<HashSet<_>>().len() == words.len();
    let allows_multiple = matches!(
        name,
        "font-synthesis"
            | "font-variant-east-asian"
            | "font-variant-ligatures"
            | "font-variant-numeric"
    );
    let group_count = |group: &[&str]| {
        words
            .iter()
            .filter(|word| group.contains(&word.as_str()))
            .count()
    };
    let conflicts = match name {
        "font-variant-east-asian" => {
            group_count(&[
                "jis78",
                "jis83",
                "jis90",
                "jis04",
                "simplified",
                "traditional",
            ]) > 1
                || group_count(&["full-width", "proportional-width"]) > 1
        }
        "font-variant-ligatures" => [
            ["common-ligatures", "no-common-ligatures"],
            ["discretionary-ligatures", "no-discretionary-ligatures"],
            ["historical-ligatures", "no-historical-ligatures"],
            ["contextual", "no-contextual"],
        ]
        .iter()
        .any(|group| group_count(group) > 1),
        "font-variant-numeric" => {
            group_count(&["lining-nums", "oldstyle-nums"]) > 1
                || group_count(&["proportional-nums", "tabular-nums"]) > 1
                || group_count(&["diagonal-fractions", "stacked-fractions"]) > 1
        }
        "font-synthesis" => group_count(&["style", "oblique-only"]) > 1,
        _ => false,
    };
    let valid = css_wide
        || !words.is_empty()
            && unique
            && (allows_multiple || words.len() == 1)
            && !conflicts
            && words.iter().all(|word| allowed.contains(&word.as_str()))
            && (words.len() == 1 || !words.iter().any(|word| word == "normal" || word == "none"));
    Some(if valid {
        let computed = if allows_multiple && words.len() > 1 {
            allowed
                .iter()
                .filter(|candidate| words.iter().any(|word| word == **candidate))
                .copied()
                .collect::<Vec<_>>()
                .join(" ")
        } else {
            words.join(" ")
        };
        DeclarationValidation::Valid(ComputedValue::Keyword(computed))
    } else {
        DeclarationValidation::Invalid
    })
}

fn validate_font_variant_alternates(value: &Value) -> DeclarationValidation {
    let values = match value {
        Value::List(values) => values.as_slice(),
        value => std::slice::from_ref(value),
    };
    if values.len() == 1
        && matches!(&values[0], Value::Keyword(keyword)
            if keyword.eq_ignore_ascii_case("normal") || is_css_wide_keyword(&keyword.to_ascii_lowercase()))
    {
        return DeclarationValidation::Valid(ComputedValue::Keyword(render_value(value)));
    }
    let mut categories = HashSet::<String>::new();
    let valid = !values.is_empty()
        && values.iter().all(|value| match value {
            Value::Keyword(keyword) if keyword.eq_ignore_ascii_case("historical-forms") => {
                categories.insert("historical-forms".to_string())
            }
            Value::Function { name, arguments } => {
                let category = name.to_ascii_lowercase();
                let valid_name = matches!(
                    category.as_str(),
                    "stylistic"
                        | "styleset"
                        | "character-variant"
                        | "swash"
                        | "ornaments"
                        | "annotation"
                );
                valid_name
                    && categories.insert(category)
                    && !arguments.is_empty()
                    && arguments.iter().all(|argument| {
                        matches!(argument, Value::Keyword(identifier)
                            if !identifier.is_empty()
                                && !is_css_wide_keyword(&identifier.to_ascii_lowercase()))
                    })
                    && (name.eq_ignore_ascii_case("styleset")
                        || name.eq_ignore_ascii_case("character-variant")
                        || arguments.len() == 1)
            }
            _ => false,
        });
    if valid {
        let canonical = values
            .iter()
            .map(|value| match value {
                Value::Keyword(keyword) => keyword.to_ascii_lowercase(),
                Value::Function { name, arguments } => format!(
                    "{}({})",
                    name.to_ascii_lowercase(),
                    arguments
                        .iter()
                        .map(render_value)
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                value => render_value(value),
            })
            .collect::<Vec<_>>()
            .join(" ");
        DeclarationValidation::Valid(ComputedValue::Keyword(canonical))
    } else {
        DeclarationValidation::Invalid
    }
}

#[derive(Clone, Copy)]
enum FontSizeAdjustMetric {
    ExHeight,
    CapHeight,
    ChWidth,
    IcWidth,
    IcHeight,
}

enum FontSizeAdjustRatio<'a> {
    FromFont,
    Value(&'a Value),
}

fn parse_font_size_adjust(
    value: &Value,
) -> Option<(FontSizeAdjustMetric, FontSizeAdjustRatio<'_>)> {
    fn ratio(value: &Value) -> Option<FontSizeAdjustRatio<'_>> {
        match value {
            Value::Keyword(keyword) if keyword.eq_ignore_ascii_case("from-font") => {
                Some(FontSizeAdjustRatio::FromFont)
            }
            Value::Number(number) if number.is_finite() && *number >= 0.0 => {
                Some(FontSizeAdjustRatio::Value(value))
            }
            Value::Function { .. }
                if typed_math::infer(value, None) == Some(typed_math::NumericType::NUMBER) =>
            {
                Some(FontSizeAdjustRatio::Value(value))
            }
            _ => None,
        }
    }
    match value {
        Value::Keyword(keyword) if keyword.eq_ignore_ascii_case("from-font") => Some((
            FontSizeAdjustMetric::ExHeight,
            FontSizeAdjustRatio::FromFont,
        )),
        Value::Number(_) | Value::Function { .. } => {
            Some((FontSizeAdjustMetric::ExHeight, ratio(value)?))
        }
        Value::List(values) if values.len() == 2 => {
            let Value::Keyword(metric) = &values[0] else {
                return None;
            };
            let metric = match metric.to_ascii_lowercase().as_str() {
                "ex-height" => FontSizeAdjustMetric::ExHeight,
                "cap-height" => FontSizeAdjustMetric::CapHeight,
                "ch-width" => FontSizeAdjustMetric::ChWidth,
                "ic-width" => FontSizeAdjustMetric::IcWidth,
                "ic-height" => FontSizeAdjustMetric::IcHeight,
                _ => return None,
            };
            Some((metric, ratio(&values[1])?))
        }
        _ => None,
    }
}

fn validate_font_size_adjust(value: &Value) -> DeclarationValidation {
    let valid = matches!(value, Value::Keyword(keyword)
        if is_css_wide_keyword(&keyword.to_ascii_lowercase()) || keyword.eq_ignore_ascii_case("none"))
        || parse_font_size_adjust(value).is_some();
    if valid {
        DeclarationValidation::Unvalidated
    } else {
        DeclarationValidation::Invalid
    }
}

/// Canonicalizes a validated `font-size-adjust` specified value for CSSOM.
pub(crate) fn normalize_font_size_adjust_specified(value: &Value) -> Option<String> {
    if let Value::Keyword(keyword) = value
        && (keyword.eq_ignore_ascii_case("none")
            || is_css_wide_keyword(&keyword.to_ascii_lowercase()))
    {
        return Some(keyword.to_ascii_lowercase());
    }
    let (metric, ratio) = parse_font_size_adjust(value)?;
    let ratio = match ratio {
        FontSizeAdjustRatio::FromFont => "from-font".to_string(),
        FontSizeAdjustRatio::Value(value) => serialize_font_size_adjust_ratio(value),
    };
    Some(match metric {
        FontSizeAdjustMetric::ExHeight => ratio,
        FontSizeAdjustMetric::CapHeight => format!("cap-height {ratio}"),
        FontSizeAdjustMetric::ChWidth => format!("ch-width {ratio}"),
        FontSizeAdjustMetric::IcWidth => format!("ic-width {ratio}"),
        FontSizeAdjustMetric::IcHeight => format!("ic-height {ratio}"),
    })
}

fn serialize_font_size_adjust_ratio(value: &Value) -> String {
    fn expression(values: &[Value]) -> String {
        if let [left, Value::Keyword(operator), right] = values
            && operator == "*"
            && matches!(right, Value::Number(_))
            && !matches!(left, Value::Number(_))
        {
            return format!("{} * {}", component(right, true), component(left, true));
        }
        values
            .iter()
            .map(|value| component(value, true))
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn component(value: &Value, nested: bool) -> String {
        if let Value::List(values) = value {
            let body = expression(values);
            return if nested { format!("({body})") } else { body };
        }
        let Value::Function { name, arguments } = value else {
            return render_value(value);
        };
        if name.eq_ignore_ascii_case("calc") {
            if let Some(number) = individual_transform::fixed_number_expression(value) {
                return if nested {
                    serialize_opentype_number(number)
                } else {
                    format!("calc({})", serialize_opentype_number(number))
                };
            }
            let body = match arguments.as_slice() {
                [Value::List(values)] => expression(values),
                _ => expression(arguments),
            };
            return if nested {
                format!("({body})")
            } else {
                format!("calc({body})")
            };
        }
        format!(
            "{}({})",
            name.to_ascii_lowercase(),
            arguments
                .iter()
                .map(|argument| component(argument, false))
                .collect::<Vec<_>>()
                .join(", ")
        )
    }

    component(value, false)
}

/// Validates a resolved declaration value against the property's grammar.
///
/// This is the extension point for per-property validation and normalization.
fn validate_declaration(name: &str, value: &Value) -> DeclarationValidation {
    if !resolved_property_is_supported(name) {
        return DeclarationValidation::Invalid;
    }
    if let Some(validation) = validate_tree_counting_numeric_declaration(name, value) {
        return validation;
    }
    if name.eq_ignore_ascii_case("opacity") {
        let valid = match value {
            Value::Keyword(keyword) => is_css_wide_keyword(&keyword.to_ascii_lowercase()),
            Value::Number(number) | Value::Percentage(number) => number.is_finite(),
            Value::Function { name, .. }
                if name.eq_ignore_ascii_case("var") || name.eq_ignore_ascii_case("env") =>
            {
                true
            }
            Value::Function { .. } | Value::List(_) => matches!(
                typed_math::infer(value, None),
                Some(typed_math::NumericType::NUMBER | typed_math::NumericType::PERCENTAGE)
            ),
            _ => false,
        };
        return if valid {
            DeclarationValidation::Unvalidated
        } else {
            DeclarationValidation::Invalid
        };
    }
    if name.eq_ignore_ascii_case("outline-offset") {
        let valid = match value {
            Value::Keyword(keyword) => is_css_wide_keyword(&keyword.to_ascii_lowercase()),
            Value::Number(number) => *number == 0.0,
            Value::Length(number, unit) => number.is_finite() && is_css_length_unit(unit),
            Value::Function { name, .. }
                if name.eq_ignore_ascii_case("var") || name.eq_ignore_ascii_case("env") =>
            {
                true
            }
            Value::Function { .. } | Value::List(_) => {
                typed_math::infer(value, Some(typed_math::NumericType::LENGTH))
                    == Some(typed_math::NumericType::LENGTH)
            }
            _ => false,
        };
        return if valid {
            DeclarationValidation::Unvalidated
        } else {
            DeclarationValidation::Invalid
        };
    }
    if let Some(validation) = validate_font_numeric_declaration(name, value) {
        return validation;
    }
    if let Some(validation) = validate_complex_font_numeric_declaration(name, value) {
        return validation;
    }
    if name.eq_ignore_ascii_case("font-size-adjust") {
        return validate_font_size_adjust(value);
    }
    if let Some(validation) = validate_font_variant_declaration(name, value) {
        return validation;
    }
    if super::shorthand::is_deferred_var_shorthand(name) {
        return validate_retained_shorthand(name, value);
    }
    if name.eq_ignore_ascii_case("font-palette") {
        return if font_palette::valid(value) {
            DeclarationValidation::Unvalidated
        } else {
            DeclarationValidation::Invalid
        };
    }
    if let Some(validation) = forced_colors::validate(name, value) {
        return validation;
    }
    if let Some(validation) = will_change::validate(name, value) {
        return validation;
    }
    if let Some(validation) = individual_transform::validate(name, value) {
        return validation;
    }
    if let Some(validation) = grid_properties::validate(name, value) {
        return validation;
    }
    // Color shorthands are expanded before entering the cascade. Validate all
    // resulting longhands here, after var() substitution and before selecting
    // a winner, so one malformed color cannot hide an earlier valid candidate.
    // This must precede the generic comma-list handling because a single color
    // never accepts a top-level comma-separated list.
    if name.eq_ignore_ascii_case("outline-color")
        && matches!(value, Value::Keyword(keyword) if keyword.eq_ignore_ascii_case("auto"))
    {
        return DeclarationValidation::Valid(ComputedValue::Keyword("auto".into()));
    }
    if is_color_property(name) {
        return validate_color_value(value);
    }
    if let Some(validation) = validate_box_spacing_declaration(name, value) {
        return validation;
    }
    if let Some(validation) = validate_logical_box_declaration(name, value) {
        return validation;
    }
    if name.eq_ignore_ascii_case("counter-reset")
        || name.eq_ignore_ascii_case("counter-increment")
        || name.eq_ignore_ascii_case("counter-set")
    {
        if matches!(value, Value::Keyword(keyword) if is_css_wide_keyword(&keyword.to_ascii_lowercase()))
        {
            return DeclarationValidation::Unvalidated;
        }
        let default = if name.eq_ignore_ascii_case("counter-increment") {
            1
        } else {
            0
        };
        if counter_value_contains_calc(value) {
            return if counter_pair_syntax_is_valid(value) {
                DeclarationValidation::Unvalidated
            } else {
                DeclarationValidation::Invalid
            };
        }
        return match counter_pairs(value, default) {
            Some(pairs) if pairs.is_empty() => {
                DeclarationValidation::Valid(ComputedValue::Keyword("none".to_string()))
            }
            Some(pairs) => DeclarationValidation::Valid(ComputedValue::Keyword(
                pairs
                    .iter()
                    .map(|(name, value)| format!("{name} {value}"))
                    .collect::<Vec<_>>()
                    .join(" "),
            )),
            None => DeclarationValidation::Invalid,
        };
    }
    if name.eq_ignore_ascii_case("quotes") {
        if matches!(value, Value::Keyword(keyword) if is_css_wide_keyword(&keyword.to_ascii_lowercase()))
        {
            return DeclarationValidation::Unvalidated;
        }
        let valid = match value {
            Value::Keyword(keyword) => {
                matches!(keyword.to_ascii_lowercase().as_str(), "auto" | "none")
            }
            Value::List(values) => {
                values.len() >= 2
                    && values.len() % 2 == 0
                    && values.iter().all(|value| matches!(value, Value::String(_)))
            }
            _ => false,
        };
        return if valid {
            DeclarationValidation::Valid(ComputedValue::Keyword(render_content_value(value)))
        } else {
            DeclarationValidation::Invalid
        };
    }
    if let Some(validation) = validate_multicol_declaration(name, value) {
        return validation;
    }

    // Normalize the property name once; every branch below compares against
    // this lowercase key instead of re-deriving it with `to_ascii_lowercase()`
    // or `eq_ignore_ascii_case()` per branch.
    let key = name.to_ascii_lowercase();

    // The following checks recognize properties whose value can itself be a
    // top-level `Value::CommaList` (handled generically further down), so
    // they must keep running before that generic handling: a comma-separated
    // value for one of these properties is rejected/normalized by its own
    // dedicated grammar rather than by the generic per-layer recursion.
    if key == "content-visibility" {
        return validate_keyword_value(value, &["visible", "auto", "hidden"]);
    }
    if key == "scroll-behavior" {
        return validate_keyword_value(value, &["auto", "smooth"]);
    }
    if key == "table-layout" {
        return validate_keyword_value(value, &["auto", "fixed"]);
    }
    if let Some(validation) = validate_scroll_snap_declaration(name, value) {
        return validation;
    }
    if matches!(
        key.as_str(),
        "overscroll-behavior-x"
            | "overscroll-behavior-y"
            | "overscroll-behavior-inline"
            | "overscroll-behavior-block"
    ) {
        return validate_keyword_value(value, &["auto", "contain", "none", "chain"]);
    }
    if matches!(
        key.as_str(),
        "contain-intrinsic-size"
            | "contain-intrinsic-width"
            | "contain-intrinsic-height"
            | "contain-intrinsic-inline-size"
            | "contain-intrinsic-block-size"
    ) {
        return validate_contain_intrinsic_size(value);
    }
    if key == "text-decoration-thickness" {
        return validate_text_decoration_thickness(value);
    }
    if key == "text-decoration-skip-ink" {
        return validate_keyword_value(value, &["auto", "all", "none"]);
    }
    if key == "text-emphasis-style" {
        return validate_text_emphasis_style(value);
    }
    if key == "text-emphasis-position" {
        return validate_keyword_groups(
            value,
            &[&["over", "under"], &["right", "left"]],
            &["auto"],
        )
        .map_keyword(|value| value.strip_suffix(" right").unwrap_or(&value).to_string());
    }
    if key == "ruby-position" {
        return validate_keyword_groups(
            value,
            &[&["alternate"], &["over", "under"]],
            &["inter-character"],
        );
    }
    if key == "ruby-align" {
        return validate_keyword_value(
            value,
            &["start", "center", "space-between", "space-around"],
        );
    }
    if key == "text-orientation" {
        return validate_keyword_value(value, &["mixed", "upright", "sideways"]);
    }
    if key == "text-combine-upright" {
        return validate_text_combine_upright(value);
    }
    if key == "text-autospace" {
        return validate_keyword_groups(
            value,
            &[
                &["ideograph-alpha"],
                &["ideograph-numeric"],
                &["punctuation"],
                &["insert", "replace"],
            ],
            &["normal", "auto", "no-autospace"],
        );
    }
    if key == "text-spacing-trim" {
        let keywords = keyword_components(value).unwrap_or_default();
        if (keywords.len() == 1 && keywords[0] == "allow-end")
            || (keywords.len() > 1 && keywords.iter().any(|value| value == "space-first"))
        {
            return DeclarationValidation::Invalid;
        }
        return validate_keyword_groups(
            value,
            &[
                &["trim-start", "space-start", "space-first"],
                &["trim-adjacent", "space-adjacent"],
                &["trim-end", "space-end", "allow-end"],
            ],
            &["normal", "auto", "trim-both", "space-all", "trim-all"],
        );
    }
    if key == "white-space-collapse" {
        return validate_keyword_value(
            value,
            &[
                "collapse",
                "discard",
                "preserve",
                "preserve-breaks",
                "preserve-spaces",
                "break-spaces",
            ],
        );
    }
    if key == "text-wrap-mode" {
        return validate_keyword_value(value, &["wrap", "nowrap"]);
    }
    if key == "text-wrap-style" {
        return validate_keyword_value(value, &["auto", "balance", "stable", "pretty"]);
    }
    if key == "text-align-last" {
        return validate_keyword_value(
            value,
            &[
                "auto",
                "start",
                "end",
                "left",
                "right",
                "center",
                "justify",
                "match-parent",
            ],
        );
    }
    if key == "text-justify" {
        if matches!(value, Value::Keyword(keyword) if keyword.eq_ignore_ascii_case("distribute")) {
            return DeclarationValidation::Valid(ComputedValue::Keyword(
                "inter-character".to_string(),
            ));
        }
        return validate_keyword_value(value, &["auto", "none", "inter-word", "inter-character"]);
    }
    if key == "block-ellipsis" {
        return match value {
            Value::Keyword(keyword) if is_css_wide_keyword(&keyword.to_ascii_lowercase()) => {
                DeclarationValidation::Valid(ComputedValue::Keyword(keyword.to_ascii_lowercase()))
            }
            Value::Keyword(keyword)
                if matches!(
                    keyword.to_ascii_lowercase().as_str(),
                    "no-ellipsis" | "ellipsis"
                ) =>
            {
                DeclarationValidation::Valid(ComputedValue::Keyword(keyword.to_ascii_lowercase()))
            }
            Value::String(_) => {
                DeclarationValidation::Valid(ComputedValue::Keyword(render_content_value(value)))
            }
            _ => DeclarationValidation::Invalid,
        };
    }
    if key == "continue" {
        return validate_keyword_value(value, &["normal", "discard", "collapse", "-webkit-legacy"]);
    }
    if key == "max-lines" {
        if matches!(value, Value::Keyword(keyword) if is_css_wide_keyword(&keyword.to_ascii_lowercase()))
        {
            return DeclarationValidation::Valid(ComputedValue::Keyword(render_value(value)));
        }
        let values = match value {
            Value::List(values) => values.as_slice(),
            value => std::slice::from_ref(value),
        };
        let mut auto = false;
        let mut integer = None;
        for value in values {
            match value {
                Value::Keyword(keyword) if keyword.eq_ignore_ascii_case("auto") && !auto => {
                    auto = true;
                }
                Value::Number(number)
                    if number.is_finite()
                        && *number >= 1.0
                        && number.fract() == 0.0
                        && integer.is_none() =>
                {
                    integer = Some(*number);
                }
                _ => return DeclarationValidation::Invalid,
            }
        }
        return if auto || integer.is_some() {
            let computed = match (auto, integer) {
                (true, Some(integer)) => {
                    format!("auto {}", super::tokenizer::trimmed_number(integer))
                }
                (true, None) => "auto".to_string(),
                (false, Some(integer)) => super::tokenizer::trimmed_number(integer),
                (false, None) => unreachable!("max-lines requires a component"),
            };
            DeclarationValidation::Valid(ComputedValue::Keyword(computed))
        } else {
            DeclarationValidation::Invalid
        };
    }
    if key == "initial-letter" {
        if matches!(value, Value::Keyword(keyword) if keyword.eq_ignore_ascii_case("normal") || is_css_wide_keyword(&keyword.to_ascii_lowercase()))
        {
            return DeclarationValidation::Valid(ComputedValue::Keyword(render_value(value)));
        }
        let values = match value {
            Value::List(values) => values.as_slice(),
            value => std::slice::from_ref(value),
        };
        let computed = match values {
            [Value::Number(size)] if size.is_finite() && *size >= 1.0 => {
                Some(format!("{}", super::tokenizer::trimmed_number(*size)))
            }
            [Value::Number(size), Value::Number(sink)] => (size.is_finite()
                && *size >= 1.0
                && sink.is_finite()
                && *sink >= 1.0
                && sink.fract() == 0.0)
                .then(|| {
                    format!(
                        "{} {}",
                        super::tokenizer::trimmed_number(*size),
                        super::tokenizer::trimmed_number(*sink)
                    )
                }),
            [Value::Number(size), Value::Function { name, arguments }]
                if name.eq_ignore_ascii_case("calc") && size.is_finite() && *size >= 1.0 =>
            {
                calc_unitless_number(arguments).and_then(|sink| {
                    (sink.is_finite() && sink >= 1.0).then(|| {
                        format!(
                            "{} {}",
                            super::tokenizer::trimmed_number(*size),
                            super::tokenizer::trimmed_number(sink.round())
                        )
                    })
                })
            }
            [Value::Number(size), Value::Keyword(position)]
            | [Value::Keyword(position), Value::Number(size)] => (size.is_finite()
                && *size >= 1.0
                && matches!(position.to_ascii_lowercase().as_str(), "drop" | "raise"))
            .then(|| {
                format!(
                    "{} {}",
                    super::tokenizer::trimmed_number(*size),
                    position.to_ascii_lowercase()
                )
            }),
            _ => None,
        };
        return if let Some(computed) = computed {
            DeclarationValidation::Valid(ComputedValue::Keyword(computed))
        } else {
            DeclarationValidation::Invalid
        };
    }
    if key == "text-box-trim" {
        return validate_keyword_value(value, &["none", "trim-start", "trim-end", "trim-both"]);
    }
    if key == "text-box-edge" {
        if matches!(value, Value::Keyword(keyword) if is_css_wide_keyword(&keyword.to_ascii_lowercase()))
        {
            return DeclarationValidation::Valid(ComputedValue::Keyword(render_value(value)));
        }
        let values = match value {
            Value::List(values) => values.as_slice(),
            value => std::slice::from_ref(value),
        };
        return if let Some(computed) = super::shorthand::canonical_text_box_edge(values) {
            DeclarationValidation::Valid(ComputedValue::Keyword(computed))
        } else {
            DeclarationValidation::Invalid
        };
    }
    if key == "hanging-punctuation" {
        if matches!(value, Value::Keyword(keyword) if keyword.eq_ignore_ascii_case("none") || is_css_wide_keyword(&keyword.to_ascii_lowercase()))
        {
            return DeclarationValidation::Unvalidated;
        }
        let values = match value {
            Value::List(values) => values.as_slice(),
            value => std::slice::from_ref(value),
        };
        let mut first = false;
        let mut end = None;
        let mut last = false;
        for value in values {
            let Value::Keyword(keyword) = value else {
                return DeclarationValidation::Invalid;
            };
            match keyword.to_ascii_lowercase().as_str() {
                "first" if !first => first = true,
                "force-end" | "allow-end" if end.is_none() => {
                    end = Some(keyword.to_ascii_lowercase())
                }
                "last" if !last => last = true,
                _ => return DeclarationValidation::Invalid,
            }
        }
        if !first && end.is_none() && !last {
            return DeclarationValidation::Invalid;
        }
        let mut canonical = Vec::new();
        if first {
            canonical.push("first".to_string());
        }
        if let Some(end) = end {
            canonical.push(end);
        }
        if last {
            canonical.push("last".to_string());
        }
        return DeclarationValidation::Valid(ComputedValue::Keyword(canonical.join(" ")));
    }
    if key == "line-break" {
        return validate_keyword_value(value, &["auto", "loose", "normal", "strict", "anywhere"]);
    }
    if key == "hyphens" {
        return validate_keyword_value(value, &["none", "manual", "auto"]);
    }
    if key == "hyphenate-character" {
        return match value {
            Value::Keyword(keyword)
                if keyword.eq_ignore_ascii_case("auto")
                    || is_css_wide_keyword(&keyword.to_ascii_lowercase()) =>
            {
                DeclarationValidation::Unvalidated
            }
            Value::String(_) => DeclarationValidation::Unvalidated,
            _ => DeclarationValidation::Invalid,
        };
    }
    if key == "tab-size" {
        return match value {
            Value::Keyword(keyword) if is_css_wide_keyword(&keyword.to_ascii_lowercase()) => {
                DeclarationValidation::Unvalidated
            }
            Value::Number(number) if number.is_finite() && *number >= 0.0 => {
                DeclarationValidation::Unvalidated
            }
            value if valid_logical_length(value, false, false) => {
                DeclarationValidation::Unvalidated
            }
            _ => DeclarationValidation::Invalid,
        };
    }
    if key == "word-spacing" {
        return match value {
            Value::Keyword(keyword)
                if keyword.eq_ignore_ascii_case("normal")
                    || is_css_wide_keyword(&keyword.to_ascii_lowercase()) =>
            {
                DeclarationValidation::Unvalidated
            }
            value if valid_logical_length(value, true, true) => DeclarationValidation::Unvalidated,
            _ => DeclarationValidation::Invalid,
        };
    }
    if key == "text-indent" {
        if matches!(value, Value::Keyword(keyword) if is_css_wide_keyword(&keyword.to_ascii_lowercase()))
        {
            return DeclarationValidation::Unvalidated;
        }
        return if text_indent_components(value).is_some() {
            DeclarationValidation::Unvalidated
        } else {
            DeclarationValidation::Invalid
        };
    }
    if key == "text-underline-position" {
        return validate_text_underline_position(value);
    }
    if key == "text-underline-offset" {
        return match value {
            Value::Keyword(value) if value.eq_ignore_ascii_case("from-font") => {
                DeclarationValidation::Invalid
            }
            Value::Number(value) if *value == 0.0 => {
                DeclarationValidation::Valid(ComputedValue::Px(0.0))
            }
            _ => validate_text_decoration_thickness(value),
        };
    }
    if let Value::CommaList(values) = value {
        let is_mask_layer_property = matches!(
            key.as_str(),
            "mask-image"
                | "mask-mode"
                | "mask-composite"
                | "mask-repeat"
                | "mask-size"
                | "mask-position"
                | "mask-position-x"
                | "mask-position-y"
                | "-webkit-mask-image"
                | "-webkit-mask-mode"
                | "-webkit-mask-composite"
                | "-webkit-mask-repeat"
                | "-webkit-mask-size"
                | "-webkit-mask-position"
                | "-webkit-mask-position-x"
                | "-webkit-mask-position-y"
        );
        if values.is_empty()
            || (values.len() > crate::paint::MAX_MASK_LAYERS && is_mask_layer_property)
        {
            return DeclarationValidation::Invalid;
        }
        let mut normalized = Vec::with_capacity(values.len());
        let mut has_unvalidated = false;
        for item in values {
            match validate_declaration(name, item) {
                DeclarationValidation::Valid(value) => {
                    normalized.push(computed_value_css_text(&value));
                }
                DeclarationValidation::Invalid => return DeclarationValidation::Invalid,
                DeclarationValidation::Unvalidated => has_unvalidated = true,
            }
        }
        // Validation is per layer, but computation still needs the resolution
        // context so relative units and calc() are normalized in every layer.
        return if has_unvalidated {
            DeclarationValidation::Unvalidated
        } else {
            DeclarationValidation::Valid(ComputedValue::Keyword(normalized.join(", ")))
        };
    }

    // Every remaining single-property (or small-alias-group) grammar dispatches
    // through this match. `is_non_negative_sizing_property` and
    // `is_position_offset_property` below are checked after it (matching their
    // original relative position among these branches): `shape-margin` is also
    // matched by `is_non_negative_sizing_property`, so its dedicated arm here
    // must run and return first, exactly as the original if-chain order
    // required; every other name in this match is disjoint from both helpers'
    // property sets, so placing their checks after the match instead of before
    // it does not change which branch a given property name takes.
    match key.as_str() {
        "text-overflow" => return validate_keyword_value(value, &["clip", "ellipsis"]),
        "text-decoration-style" => {
            return validate_keyword_value(value, &["solid", "double", "dotted", "dashed", "wavy"]);
        }
        "position" => {
            return validate_keyword_value(
                value,
                &["static", "relative", "absolute", "fixed", "sticky"],
            );
        }
        "direction" => return validate_keyword_value(value, &["ltr", "rtl"]),
        "writing-mode" => {
            return validate_keyword_value(
                value,
                &[
                    "horizontal-tb",
                    "vertical-rl",
                    "vertical-lr",
                    "sideways-rl",
                    "sideways-lr",
                ],
            );
        }
        "unicode-bidi" => {
            return validate_keyword_value(
                value,
                &[
                    "normal",
                    "embed",
                    "bidi-override",
                    "isolate",
                    "isolate-override",
                    "plaintext",
                ],
            );
        }
        "transform-style" => return validate_keyword_value(value, &["flat", "preserve-3d"]),
        "backface-visibility" => return validate_keyword_value(value, &["visible", "hidden"]),
        "mix-blend-mode" => {
            return validate_keyword_value(
                value,
                &[
                    "normal",
                    "multiply",
                    "screen",
                    "overlay",
                    "darken",
                    "lighten",
                    "color-dodge",
                    "color-burn",
                    "hard-light",
                    "soft-light",
                    "difference",
                    "exclusion",
                    "hue",
                    "saturation",
                    "color",
                    "luminosity",
                    "plus-darker",
                    "plus-lighter",
                ],
            );
        }
        "isolation" => return validate_keyword_value(value, &["auto", "isolate"]),
        "background-origin" => {
            return validate_keyword_value(value, &["border-box", "padding-box", "content-box"]);
        }
        "background-clip" => {
            return validate_keyword_value(
                value,
                &["border-box", "padding-box", "content-box", "text"],
            );
        }
        "background-blend-mode" => {
            return validate_blend_mode_list(value);
        }
        "background-image" => return validate_background_image_declaration(value),
        "image-rendering" => {
            return validate_keyword_value(
                value,
                &["auto", "smooth", "high-quality", "crisp-edges", "pixelated"],
            );
        }
        "image-orientation" => {
            return validate_image_orientation(value);
        }
        "border-image-source" | "mask-border-source" => {
            return validate_background_image_declaration(value);
        }
        "border-image-slice" | "mask-border-slice" => {
            return validate_nine_slice_components(value, NineSliceComponent::Slice);
        }
        "border-image-width" | "mask-border-width" => {
            return validate_nine_slice_components(value, NineSliceComponent::Width);
        }
        "border-image-outset" | "mask-border-outset" => {
            return validate_nine_slice_components(value, NineSliceComponent::Outset);
        }
        "border-image-repeat" | "mask-border-repeat" => {
            return validate_nine_slice_repeat(value);
        }
        "mask-border-mode" => {
            return validate_keyword_value(value, &["alpha", "luminance"]);
        }
        "mask-image" => return validate_mask_image_declaration(value),
        "mask-mode" => {
            return match value {
                Value::Keyword(keyword)
                    if is_css_wide_keyword(&keyword.to_ascii_lowercase())
                        || matches!(
                            keyword.to_ascii_lowercase().as_str(),
                            "alpha" | "luminance" | "match-source"
                        ) =>
                {
                    DeclarationValidation::Unvalidated
                }
                _ => DeclarationValidation::Invalid,
            };
        }
        "mask-composite" => {
            return match value {
                Value::Keyword(keyword)
                    if is_css_wide_keyword(&keyword.to_ascii_lowercase())
                        || matches!(
                            keyword.to_ascii_lowercase().as_str(),
                            "add" | "subtract" | "intersect" | "exclude"
                        ) =>
                {
                    DeclarationValidation::Unvalidated
                }
                _ => DeclarationValidation::Invalid,
            };
        }
        "shape-outside" => {
            if matches!(value, Value::Keyword(keyword) if is_css_wide_keyword(&keyword.to_ascii_lowercase()))
            {
                return DeclarationValidation::Unvalidated;
            }
            let rendered = render_value(value);
            return if crate::paint::is_valid_shape_outside_value(&rendered) {
                DeclarationValidation::Unvalidated
            } else {
                DeclarationValidation::Invalid
            };
        }
        "shape-margin" => return validate_shape_margin_declaration(name, value),
        "clip-path" => {
            return match value {
                Value::Keyword(keyword)
                    if is_css_wide_keyword(&keyword.to_ascii_lowercase())
                        || keyword.eq_ignore_ascii_case("none") =>
                {
                    DeclarationValidation::Unvalidated
                }
                Value::Function { name: function, .. }
                    if matches!(
                        function.to_ascii_lowercase().as_str(),
                        "inset" | "circle" | "ellipse" | "polygon"
                    ) =>
                {
                    let rendered = render_value(value);
                    if crate::paint::is_valid_clip_path_value(&rendered) {
                        DeclarationValidation::Unvalidated
                    } else {
                        DeclarationValidation::Invalid
                    }
                }
                _ => DeclarationValidation::Invalid,
            };
        }
        "mask-repeat" | "background-repeat" => return validate_repeat_style_value(value),
        "background-attachment" => {
            return match value {
                Value::Keyword(keyword)
                    if is_css_wide_keyword(&keyword.to_ascii_lowercase())
                        || matches!(
                            keyword.to_ascii_lowercase().as_str(),
                            "scroll" | "fixed" | "local"
                        ) =>
                {
                    DeclarationValidation::Unvalidated
                }
                _ => DeclarationValidation::Invalid,
            };
        }
        "accent-color" | "caret-color" => {
            if matches!(value, Value::Keyword(keyword) if keyword.eq_ignore_ascii_case("auto") || is_css_wide_keyword(&keyword.to_ascii_lowercase()))
            {
                return DeclarationValidation::Valid(ComputedValue::Keyword(
                    render_value(value).to_ascii_lowercase(),
                ));
            }
            return validate_color_value(value);
        }
        "appearance" => {
            return validate_keyword_value(
                value,
                &[
                    "none",
                    "auto",
                    "base-select",
                    "textfield",
                    "menulist-button",
                ],
            );
        }
        "color-scheme" => return validate_color_scheme_declaration(value),
        "field-sizing" => return validate_keyword_value(value, &["fixed", "content"]),
        "resize" => {
            return validate_keyword_value(
                value,
                &["none", "both", "horizontal", "vertical", "block", "inline"],
            );
        }
        "touch-action" => return validate_touch_action_declaration(value),
        "user-select" => {
            return validate_keyword_value(value, &["auto", "text", "none", "all"]);
        }
        "scrollbar-width" => {
            return validate_keyword_value(value, &["auto", "thin", "none"]);
        }
        "scrollbar-gutter" => {
            let rendered = render_value(value).to_ascii_lowercase();
            if is_css_wide_keyword(&rendered) || matches!(rendered.as_str(), "auto" | "stable") {
                return DeclarationValidation::Valid(ComputedValue::Keyword(rendered));
            }
            let words = rendered.split_ascii_whitespace().collect::<Vec<_>>();
            return if words.len() == 2 && words.contains(&"stable") && words.contains(&"both-edges")
            {
                DeclarationValidation::Valid(ComputedValue::Keyword(
                    "stable both-edges".to_string(),
                ))
            } else {
                DeclarationValidation::Invalid
            };
        }
        "scrollbar-color" => return validate_scrollbar_color_declaration(value),
        "overflow-anchor" => return validate_keyword_value(value, &["auto", "none"]),
        "overflow-clip-margin" => return validate_overflow_clip_margin_declaration(value),
        "cursor" => {
            return match compute_cursor_value(value) {
                Some(computed) => DeclarationValidation::Valid(computed),
                None => DeclarationValidation::Invalid,
            };
        }
        "aspect-ratio" => {
            // Normalizing needs the resolution context for `calc()`, so a valid
            // value goes on to `compute_value` (see `render_aspect_ratio_value`).
            let is_css_wide = matches!(
                value,
                Value::Keyword(keyword) if is_css_wide_keyword(&keyword.to_ascii_lowercase())
            );
            return if is_css_wide || aspect_ratio_parts(value).is_some() {
                DeclarationValidation::Unvalidated
            } else {
                DeclarationValidation::Invalid
            };
        }
        "object-fit" => {
            return validate_keyword_value(
                value,
                &["fill", "contain", "cover", "none", "scale-down"],
            );
        }
        "pointer-events" => {
            return match value {
                Value::Keyword(keyword) => {
                    let lower = keyword.to_ascii_lowercase();
                    if is_css_wide_keyword(&lower) || is_supported_pointer_events_keyword(&lower) {
                        DeclarationValidation::Valid(ComputedValue::Keyword(lower))
                    } else {
                        DeclarationValidation::Invalid
                    }
                }
                _ => DeclarationValidation::Invalid,
            };
        }
        "object-position" => {
            // The grammar is checked here, but normalizing keywords to percentages
            // and lengths to pixels needs the resolution context, so the value goes
            // through `compute_value` (see `render_object_position_value`). A
            // CSS-wide keyword is handled by the cascade, not by this grammar.
            let is_css_wide = matches!(
                value,
                Value::Keyword(keyword) if is_css_wide_keyword(&keyword.to_ascii_lowercase())
            );
            return if is_css_wide || object_position_components(value).is_some() {
                DeclarationValidation::Unvalidated
            } else {
                DeclarationValidation::Invalid
            };
        }
        "container-type" => {
            return validate_keyword_value(value, &["normal", "inline-size", "size"]);
        }
        "container-name" => return validate_container_name_declaration(value),
        "contain" => return validate_contain_declaration(value),
        "transform" => return validate_transform_declaration(value),
        "perspective" => return validate_perspective_declaration(value),
        "filter" | "backdrop-filter" => {
            let rendered = render_value(value);
            if is_css_wide_keyword(&rendered.to_ascii_lowercase()) {
                return DeclarationValidation::Valid(ComputedValue::Keyword(
                    rendered.to_ascii_lowercase(),
                ));
            }
            return match super::normalize_filter_list(&rendered) {
                Some(normalized) => {
                    DeclarationValidation::Valid(ComputedValue::Keyword(normalized))
                }
                None => DeclarationValidation::Invalid,
            };
        }
        "text-shadow" => return text_shadow::validate(value),
        "transform-origin" => return validate_transform_origin_declaration(value),
        "perspective-origin" => return validate_perspective_origin_declaration(value),
        "transition-property"
        | "transition-duration"
        | "transition-timing-function"
        | "transition-delay" => {
            if matches!(name, "transition-duration" | "transition-delay") {
                let values = match value {
                    Value::CommaList(values) => values.as_slice(),
                    value => std::slice::from_ref(value),
                };
                let allow_negative = name == "transition-delay";
                if let Some(seconds) = values
                    .iter()
                    .map(|value| resolve_time_seconds(value))
                    .collect::<Option<Vec<_>>>()
                    && (allow_negative || seconds.iter().all(|value| *value >= 0.0))
                {
                    return DeclarationValidation::Valid(ComputedValue::Keyword(
                        seconds
                            .iter()
                            .map(|value| format!("{}s", super::tokenizer::trimmed_number(*value)))
                            .collect::<Vec<_>>()
                            .join(", "),
                    ));
                }
            }
            let rendered = render_value(value);
            return match super::computed_transition_longhand(name, &rendered) {
                Some(normalized) => {
                    DeclarationValidation::Valid(ComputedValue::Keyword(normalized))
                }
                None => DeclarationValidation::Invalid,
            };
        }
        _ => {}
    }

    if is_non_negative_sizing_property(name) {
        return validate_sizing_value(name, value);
    }
    if is_position_offset_property(name)
        && matches!(value, Value::Function { name: function, .. } if is_length_percentage_math_function(function))
    {
        return match compute_value(value, name, ResolutionContext::default()) {
            ComputedValue::Px(_)
            | ComputedValue::Percentage(_)
            | ComputedValue::LengthPercentage(_) => DeclarationValidation::Unvalidated,
            _ => DeclarationValidation::Invalid,
        };
    }
    DeclarationValidation::Unvalidated
}

pub(crate) fn normalize_cjk_specified(property: &str, value: &str) -> Option<String> {
    let declarations = crate::css::parse_style_attribute(&format!("{property}:{value}"));
    let declaration = declarations
        .iter()
        .find(|declaration| declaration.name.eq_ignore_ascii_case(property))?;
    match validate_declaration(property, &declaration.value) {
        DeclarationValidation::Valid(value) => Some(value.css_text()),
        DeclarationValidation::Unvalidated if supports_declaration(property, value) => {
            Some(value.trim().to_ascii_lowercase())
        }
        DeclarationValidation::Invalid | DeclarationValidation::Unvalidated => None,
    }
}

pub(crate) fn normalize_initial_letter_specified(value: &Value) -> Option<String> {
    let values = match value {
        Value::List(values) => values.as_slice(),
        value => std::slice::from_ref(value),
    };
    match values {
        [Value::Keyword(keyword)] if keyword.eq_ignore_ascii_case("normal") => {
            Some("normal".to_string())
        }
        [Value::Number(size)] if size.is_finite() && *size >= 1.0 => {
            Some(super::tokenizer::trimmed_number(*size))
        }
        [Value::Number(size), second] if size.is_finite() && *size >= 1.0 => Some(format!(
            "{} {}",
            super::tokenizer::trimmed_number(*size),
            render_value(second)
        )),
        [Value::Keyword(position), Value::Number(size)]
            if matches!(position.to_ascii_lowercase().as_str(), "drop" | "raise")
                && size.is_finite()
                && *size >= 1.0 =>
        {
            Some(format!(
                "{} {}",
                super::tokenizer::trimmed_number(*size),
                position.to_ascii_lowercase()
            ))
        }
        _ => None,
    }
}

pub(crate) fn normalize_text_box_edge_specified(value: &Value) -> Option<String> {
    let values = match value {
        Value::List(values) => values.as_slice(),
        value => std::slice::from_ref(value),
    };
    super::shorthand::canonical_text_box_edge(values)
}

fn text_indent_components(value: &Value) -> Option<(&Value, bool, bool)> {
    let components = match value {
        Value::List(values) => values.as_slice(),
        value => std::slice::from_ref(value),
    };
    let mut amount = None;
    let mut hanging = false;
    let mut each_line = false;
    for component in components {
        match component {
            Value::Keyword(keyword) if keyword.eq_ignore_ascii_case("hanging") && !hanging => {
                hanging = true;
            }
            Value::Keyword(keyword) if keyword.eq_ignore_ascii_case("each-line") && !each_line => {
                each_line = true;
            }
            value if amount.is_none() && valid_logical_length(value, true, true) => {
                amount = Some(value);
            }
            _ => return None,
        }
    }
    Some((amount?, hanging, each_line))
}

/// Validates `background-image`: gradient functions are checked with the
/// paint module's gradient parser, and everything else must be `none`,
/// `url(...)`, or a CSS-wide keyword (left `Unvalidated` for the generic
/// compute path).
/// Canonicalizes computed colors and resolution units within owned image values.
/// Serializes context-free opaque legacy colors inside computed image values.
fn opaque_image_color(value: &Value) -> Option<Value> {
    let color = crate::paint::color::parse_color(&render_value(value))?;
    (color.a == 255).then(|| Value::Keyword(format!("rgb({}, {}, {})", color.r, color.g, color.b)))
}

fn computed_image_components(value: &Value) -> Value {
    if matches!(value, Value::Function { .. }) && !sibling_functions::contains(value) {
        if typed_math::infer(value, None) == Some(typed_math::NumericType::NUMBER) {
            if let Some(LengthMathValue::Number(number)) =
                evaluate_length_math_value(value, ResolutionContext::default())
            {
                return Value::Number(number);
            }
        }
    }
    if let Value::Function { name, .. } = value {
        if name.eq_ignore_ascii_case("url") {
            return value.clone();
        }
    }
    if !matches!(value, Value::Keyword(name) if name.eq_ignore_ascii_case("currentcolor")) {
        if let Some(color) = crate::paint::color4::CssColor::parse(&render_value(value)) {
            return Value::Keyword(color.serialize_computed());
        }
        if let Some(color) = opaque_image_color(value) {
            return color;
        }
    }
    let resolved = match value {
        Value::Length(number, unit) if unit.eq_ignore_ascii_case("x") => {
            Value::Length(*number, "dppx".into())
        }
        Value::Function { name, arguments } => Value::Function {
            name: name.clone(),
            arguments: arguments.iter().map(computed_image_components).collect(),
        },
        Value::List(values) => Value::List(values.iter().map(computed_image_components).collect()),
        Value::CommaList(values) => {
            Value::CommaList(values.iter().map(computed_image_components).collect())
        }
        _ => value.clone(),
    };
    if let Value::Function { .. } = &resolved {
        if let Some(color) = crate::paint::color4::CssColor::parse(&render_value(&resolved)) {
            return Value::Keyword(color.serialize_computed());
        }
        if let Some(color) = opaque_image_color(&resolved) {
            return color;
        }
    }
    resolved
}

fn valid_composite_image(value: &Value) -> bool {
    let Value::Function { name, arguments } = value else {
        return false;
    };
    match name.to_ascii_lowercase().as_str() {
        "cross-fade" if arguments.len() >= 2 => arguments.iter().all(|value| {
            let values = match value {
                Value::List(values) => values.as_slice(),
                value => std::slice::from_ref(value),
            };
            let image_valid =
                |value: &Value| valid_image_component(value) || is_valid_color_value(value);
            match values {
                [image] => image_valid(image),
                [image, Value::Percentage(amount)] => {
                    amount.is_finite() && *amount >= 0.0 && image_valid(image)
                }
                [Value::Percentage(amount), image] => amount.is_finite() && image_valid(image),
                _ => false,
            }
        }),
        "image-set" | "-webkit-image-set" if !arguments.is_empty() => {
            arguments.iter().all(valid_image_set_entry)
        }
        _ => false,
    }
}

fn validate_blend_mode_list(value: &Value) -> DeclarationValidation {
    const MODES: &[&str] = &[
        "normal",
        "multiply",
        "screen",
        "overlay",
        "darken",
        "lighten",
        "color-dodge",
        "color-burn",
        "hard-light",
        "soft-light",
        "difference",
        "exclusion",
        "hue",
        "saturation",
        "color",
        "luminosity",
        "plus-darker",
        "plus-lighter",
    ];
    if let Value::Keyword(keyword) = value {
        let keyword = keyword.to_ascii_lowercase();
        return if is_css_wide_keyword(&keyword) || MODES.contains(&keyword.as_str()) {
            DeclarationValidation::Valid(ComputedValue::Keyword(keyword))
        } else {
            DeclarationValidation::Invalid
        };
    }
    let Value::CommaList(values) = value else {
        return DeclarationValidation::Invalid;
    };
    let modes = values
        .iter()
        .map(|value| match value {
            Value::Keyword(keyword) if MODES.contains(&keyword.to_ascii_lowercase().as_str()) => {
                Some(keyword.to_ascii_lowercase())
            }
            _ => None,
        })
        .collect::<Option<Vec<_>>>();
    match modes {
        Some(modes) if !modes.is_empty() => {
            DeclarationValidation::Valid(ComputedValue::Keyword(modes.join(", ")))
        }
        _ => DeclarationValidation::Invalid,
    }
}

fn validate_image_orientation(value: &Value) -> DeclarationValidation {
    if let Value::Keyword(keyword) = value {
        let keyword = keyword.to_ascii_lowercase();
        if is_css_wide_keyword(&keyword) || matches!(keyword.as_str(), "from-image" | "none") {
            return DeclarationValidation::Valid(ComputedValue::Keyword(keyword));
        }
    }
    DeclarationValidation::Invalid
}

fn valid_image_component(value: &Value) -> bool {
    match value {
        Value::Function { name, .. } if name.eq_ignore_ascii_case("url") => true,
        Value::Keyword(value) if value.trim_start().to_ascii_lowercase().starts_with("url(") => {
            true
        }
        Value::Function { name, .. } if name.to_ascii_lowercase().ends_with("gradient") => {
            crate::paint::parse_gradient(&render_value(value)).is_some()
        }
        value => valid_composite_image(value),
    }
}

fn valid_image_set_entry(value: &Value) -> bool {
    let values = match value {
        Value::List(values) => values.as_slice(),
        value => std::slice::from_ref(value),
    };
    let Some((image, descriptors)) = values.split_first() else {
        return false;
    };
    if !matches!(image, Value::String(_)) && !valid_image_component(image) {
        return false;
    }
    let mut resolution = false;
    let mut mime_type = false;
    for descriptor in descriptors {
        match descriptor {
            Value::Length(number, unit)
                if !resolution
                    && number.is_finite()
                    && *number > 0.0
                    && matches!(unit.to_ascii_lowercase().as_str(), "dppx" | "dpi" | "dpcm") =>
            {
                resolution = true
            }
            Value::Function { name, arguments }
                if !mime_type
                    && name.eq_ignore_ascii_case("type")
                    && matches!(arguments.as_slice(), [Value::String(_)]) =>
            {
                mime_type = true
            }
            _ => return false,
        }
    }
    true
}

fn validate_background_image_declaration(value: &Value) -> DeclarationValidation {
    let computed = computed_image_components(value);
    if valid_composite_image(&computed) {
        return DeclarationValidation::Valid(ComputedValue::Keyword(render_value(&computed)));
    }
    let value = &computed;
    if let Value::Function { name: function, .. } = value {
        let lower = function.to_ascii_lowercase();
        if matches!(
            lower.as_str(),
            "linear-gradient"
                | "repeating-linear-gradient"
                | "radial-gradient"
                | "repeating-radial-gradient"
                | "conic-gradient"
                | "repeating-conic-gradient"
        ) {
            let rendered = render_value(value);
            return if gradient_syntax_is_supported(&rendered) {
                DeclarationValidation::Valid(ComputedValue::Keyword(rendered))
            } else {
                DeclarationValidation::Invalid
            };
        }
    }
    match value {
        Value::Keyword(keyword)
            if is_css_wide_keyword(&keyword.to_ascii_lowercase())
                || keyword.eq_ignore_ascii_case("none")
                || keyword.to_ascii_lowercase().starts_with("url(") =>
        {
            DeclarationValidation::Unvalidated
        }
        _ => DeclarationValidation::Invalid,
    }
}

fn gradient_syntax_is_supported(text: &str) -> bool {
    if crate::paint::parse_gradient(text).is_some() {
        return true;
    }
    let replacement = if text
        .trim_start()
        .to_ascii_lowercase()
        .starts_with("conic-gradient(")
        || text
            .trim_start()
            .to_ascii_lowercase()
            .starts_with("repeating-conic-gradient(")
    {
        "0deg"
    } else {
        "0%"
    };
    let mut normalized = text.replace("currentcolor", "black");
    for function in ["calc", "min", "max", "clamp"] {
        while let Some(start) = normalized
            .to_ascii_lowercase()
            .find(&format!("{function}("))
        {
            let open = start + function.len();
            let mut depth = 0usize;
            let mut end = None;
            for (offset, ch) in normalized[open..].char_indices() {
                match ch {
                    '(' => depth += 1,
                    ')' => {
                        depth = depth.saturating_sub(1);
                        if depth == 0 {
                            end = Some(open + offset + ch.len_utf8());
                            break;
                        }
                    }
                    _ => {}
                }
            }
            let Some(end) = end else {
                return false;
            };
            normalized.replace_range(start..end, replacement);
        }
    }
    crate::paint::parse_gradient(&normalized).is_some()
}

/// Validates `mask-image`: same shape as `background-image`, but gradients go
/// through the paint color module's gradient parser.
fn validate_mask_image_declaration(value: &Value) -> DeclarationValidation {
    if let Value::Function { name: function, .. } = value {
        let lower = function.to_ascii_lowercase();
        if lower == "linear-gradient"
            || lower == "repeating-linear-gradient"
            || lower == "radial-gradient"
            || lower == "repeating-radial-gradient"
            || lower == "conic-gradient"
            || lower == "repeating-conic-gradient"
        {
            let rendered = render_value(value);
            return if crate::paint::color::parse_gradient(&rendered).is_some() {
                DeclarationValidation::Valid(ComputedValue::Keyword(rendered))
            } else {
                DeclarationValidation::Invalid
            };
        }
    }
    match value {
        Value::Keyword(keyword)
            if is_css_wide_keyword(&keyword.to_ascii_lowercase())
                || keyword.eq_ignore_ascii_case("none")
                || keyword.to_ascii_lowercase().starts_with("url(") =>
        {
            DeclarationValidation::Unvalidated
        }
        _ => DeclarationValidation::Invalid,
    }
}

/// Validates `shape-margin`: a non-negative length, percentage, `0`, or a
/// `calc()`-family function resolving to one of those. `name` is forwarded to
/// `compute_value` for math-function resolution.
fn validate_shape_margin_declaration(name: &str, value: &Value) -> DeclarationValidation {
    match value {
        Value::Keyword(keyword) if is_css_wide_keyword(&keyword.to_ascii_lowercase()) => {
            DeclarationValidation::Unvalidated
        }
        Value::Length(number, unit)
            if *number >= 0.0
                && resolve_length_to_px(*number, unit, ResolutionContext::default()).is_some() =>
        {
            DeclarationValidation::Unvalidated
        }
        Value::Percentage(number) if *number >= 0.0 => DeclarationValidation::Unvalidated,
        Value::Number(number) if *number == 0.0 => {
            DeclarationValidation::Valid(ComputedValue::Px(0.0))
        }
        Value::Function { name: function, .. } if is_length_percentage_math_function(function) => {
            match compute_value(value, name, ResolutionContext::default()) {
                ComputedValue::Px(number) | ComputedValue::Percentage(number) if number >= 0.0 => {
                    DeclarationValidation::Unvalidated
                }
                ComputedValue::LengthPercentage(_) => DeclarationValidation::Unvalidated,
                _ => DeclarationValidation::Invalid,
            }
        }
        _ => DeclarationValidation::Invalid,
    }
}

/// Validates the shared `repeat`/`no-repeat`/`repeat-x`/`repeat-y` (or a
/// 1-2 item axis list of `repeat`/`no-repeat`) grammar used by both
/// `mask-repeat` and `background-repeat`.
fn validate_repeat_style_value(value: &Value) -> DeclarationValidation {
    let valid_axis = |value: &Value| {
        matches!(
            value,
            Value::Keyword(keyword)
                if matches!(keyword.to_ascii_lowercase().as_str(), "repeat" | "no-repeat")
        )
    };
    match value {
        Value::Keyword(keyword)
            if is_css_wide_keyword(&keyword.to_ascii_lowercase())
                || matches!(
                    keyword.to_ascii_lowercase().as_str(),
                    "repeat" | "no-repeat" | "repeat-x" | "repeat-y"
                ) =>
        {
            DeclarationValidation::Unvalidated
        }
        Value::List(values) if (1..=2).contains(&values.len()) && values.iter().all(valid_axis) => {
            DeclarationValidation::Unvalidated
        }
        _ => DeclarationValidation::Invalid,
    }
}

#[derive(Clone, Copy)]
enum NineSliceComponent {
    Slice,
    Width,
    Outset,
}

fn validate_nine_slice_components(
    value: &Value,
    component: NineSliceComponent,
) -> DeclarationValidation {
    if matches!(value, Value::Keyword(keyword) if is_css_wide_keyword(&keyword.to_ascii_lowercase()))
    {
        return DeclarationValidation::Unvalidated;
    }
    let values = match value {
        Value::List(values) => values.as_slice(),
        value => std::slice::from_ref(value),
    };
    let mut fill = false;
    let mut fill_index = None;
    let mut count = 0usize;
    for (index, value) in values.iter().enumerate() {
        if matches!(component, NineSliceComponent::Slice)
            && matches!(value, Value::Keyword(keyword) if keyword.eq_ignore_ascii_case("fill"))
        {
            if fill {
                return DeclarationValidation::Invalid;
            }
            fill = true;
            fill_index = Some(index);
            continue;
        }
        let valid = match (component, value) {
            (NineSliceComponent::Slice, Value::Number(number))
            | (NineSliceComponent::Slice, Value::Percentage(number))
            | (NineSliceComponent::Width, Value::Number(number))
            | (NineSliceComponent::Width, Value::Percentage(number))
            | (NineSliceComponent::Outset, Value::Number(number)) => {
                number.is_finite() && *number >= 0.0
            }
            (NineSliceComponent::Width, Value::Keyword(keyword)) => {
                keyword.eq_ignore_ascii_case("auto")
            }
            (
                NineSliceComponent::Width | NineSliceComponent::Outset,
                Value::Length(number, unit),
            ) => {
                number.is_finite()
                    && *number >= 0.0
                    && resolve_length_to_px(*number, unit, ResolutionContext::default()).is_some()
            }
            (_, Value::Function { name, .. }) if is_length_percentage_math_function(name) => {
                match evaluate_length_math_value(value, ResolutionContext::default()) {
                    Some(LengthMathValue::Number(_)) => true,
                    Some(LengthMathValue::Length(math)) => {
                        !matches!(component, NineSliceComponent::Outset) || math.is_pure_px()
                    }
                    None => false,
                }
            }
            _ => false,
        };
        if !valid {
            return DeclarationValidation::Invalid;
        }
        count += 1;
    }
    if fill_index.is_some_and(|index| index != 0 && index + 1 != values.len()) {
        DeclarationValidation::Invalid
    } else if (1..=4).contains(&count) {
        DeclarationValidation::Unvalidated
    } else {
        DeclarationValidation::Invalid
    }
}

fn validate_nine_slice_repeat(value: &Value) -> DeclarationValidation {
    if matches!(value, Value::Keyword(keyword) if is_css_wide_keyword(&keyword.to_ascii_lowercase()))
    {
        return DeclarationValidation::Unvalidated;
    }
    let values = match value {
        Value::List(values) => values.as_slice(),
        value => std::slice::from_ref(value),
    };
    if (1..=2).contains(&values.len())
        && values.iter().all(|value| {
            matches!(value, Value::Keyword(keyword) if matches!(keyword.to_ascii_lowercase().as_str(), "stretch" | "repeat" | "round" | "space"))
        })
    {
        DeclarationValidation::Unvalidated
    } else {
        DeclarationValidation::Invalid
    }
}

pub(super) fn nine_slice_longhand_is_valid(name: &str, value: &Value) -> bool {
    !matches!(
        match name {
            "border-image-source" | "mask-border-source" => {
                validate_background_image_declaration(value)
            }
            "border-image-slice" | "mask-border-slice" => {
                validate_nine_slice_components(value, NineSliceComponent::Slice)
            }
            "border-image-width" | "mask-border-width" => {
                validate_nine_slice_components(value, NineSliceComponent::Width)
            }
            "border-image-outset" | "mask-border-outset" => {
                validate_nine_slice_components(value, NineSliceComponent::Outset)
            }
            "border-image-repeat" | "mask-border-repeat" => validate_nine_slice_repeat(value),
            "mask-border-mode" => validate_keyword_value(value, &["alpha", "luminance"]),
            _ => DeclarationValidation::Invalid,
        },
        DeclarationValidation::Invalid
    )
}

pub(crate) fn normalize_nine_slice_specified(property: &str, text: &str) -> Option<String> {
    if !supports_declaration(property, text) {
        return None;
    }
    let declarations = super::parse_style_attribute(&format!("{property}: {text}"));
    let value = &declarations
        .iter()
        .find(|declaration| declaration.name == property)?
        .value;
    let values = match value {
        Value::List(values) => values.as_slice(),
        value => std::slice::from_ref(value),
    };
    if property.ends_with("-slice") {
        let mut parts = values
            .iter()
            .filter(|value| !matches!(value, Value::Keyword(keyword) if keyword.eq_ignore_ascii_case("fill")))
            .map(render_value)
            .collect::<Vec<_>>();
        if values.iter().any(
            |value| matches!(value, Value::Keyword(keyword) if keyword.eq_ignore_ascii_case("fill")),
        ) {
            parts.push("fill".to_string());
        }
        return Some(parts.join(" "));
    }
    if property.ends_with("-repeat") && values.len() == 2 && values[0] == values[1] {
        return Some(render_value(&values[0]));
    }
    Some(render_value(value))
}

/// Validates `container-name`: `none`, a CSS-wide keyword, a single custom
/// identifier, or a non-empty list of custom identifiers (excluding the
/// reserved `and`/`or`/`not`/`default`/`none` keywords).
fn validate_container_name_declaration(value: &Value) -> DeclarationValidation {
    let valid_custom_name = |keyword: &str| {
        let lower = keyword.to_ascii_lowercase();
        !is_css_wide_keyword(&lower)
            && !matches!(lower.as_str(), "none" | "and" | "or" | "not" | "default")
    };
    let valid =
        match value {
            Value::Keyword(keyword) => {
                keyword.eq_ignore_ascii_case("none")
                    || is_css_wide_keyword(&keyword.to_ascii_lowercase())
                    || valid_custom_name(keyword)
            }
            Value::List(values) => !values.is_empty()
                && values.iter().all(
                    |value| matches!(value, Value::Keyword(keyword) if valid_custom_name(keyword)),
                ),
            _ => false,
        };
    if valid {
        DeclarationValidation::Valid(ComputedValue::Keyword(render_value(value)))
    } else {
        DeclarationValidation::Invalid
    }
}

/// Validates `contain`: a single keyword (`none`/`strict`/`content`/`size`/
/// `layout`/`paint`), a CSS-wide keyword, or a list of distinct
/// `size`/`layout`/`paint` keywords.
fn validate_contain_declaration(value: &Value) -> DeclarationValidation {
    let keyword = |value: &Value| match value {
        Value::Keyword(keyword) => Some(keyword.to_ascii_lowercase()),
        _ => None,
    };
    let valid = match value {
        Value::Keyword(value) => {
            let value = value.to_ascii_lowercase();
            is_css_wide_keyword(&value)
                || matches!(
                    value.as_str(),
                    "none" | "strict" | "content" | "size" | "layout" | "paint"
                )
        }
        Value::List(values) => {
            let keywords = values.iter().filter_map(keyword).collect::<Vec<_>>();
            !keywords.is_empty()
                && keywords.len() == values.len()
                && keywords
                    .iter()
                    .all(|value| matches!(value.as_str(), "size" | "layout" | "paint"))
                && keywords.iter().collect::<BTreeSet<_>>().len() == keywords.len()
        }
        _ => false,
    };
    if valid {
        DeclarationValidation::Valid(ComputedValue::Keyword(
            render_value(value).to_ascii_lowercase(),
        ))
    } else {
        DeclarationValidation::Invalid
    }
}

/// A reference box used only to exercise the transform/perspective grammar
/// parsers during validation; the parsed geometry itself is discarded; layout
/// resolves the real reference box when computing the used transform.
fn validation_transform_reference_box() -> super::TransformReferenceBox {
    super::TransformReferenceBox {
        x: 0.0,
        y: 0.0,
        width: 100.0,
        height: 100.0,
        font_size: 16.0,
        root_font_size: 16.0,
    }
}

/// Validates `transform` by parsing it as a transform function list.
fn validate_transform_declaration(value: &Value) -> DeclarationValidation {
    if generic_transform::valid(value) {
        DeclarationValidation::Unvalidated
    } else {
        DeclarationValidation::Invalid
    }
}

/// Validates `perspective`: `none` and CSS-wide keywords pass through
/// directly, otherwise the value must parse as a perspective length.
fn validate_perspective_declaration(value: &Value) -> DeclarationValidation {
    let rendered = render_value(value);
    let lower = rendered.to_ascii_lowercase();
    if is_css_wide_keyword(&lower) || lower == "none" {
        return DeclarationValidation::Valid(ComputedValue::Keyword(lower));
    }
    let reference = validation_transform_reference_box();
    if super::parse_perspective_with_origin(&rendered, "50% 50%", reference).is_some() {
        DeclarationValidation::Valid(ComputedValue::Keyword(rendered))
    } else {
        DeclarationValidation::Invalid
    }
}

/// Validates `transform-origin` by parsing it alongside a representative
/// transform function.
fn validate_transform_origin_declaration(value: &Value) -> DeclarationValidation {
    let rendered = render_value(value);
    let reference = validation_transform_reference_box();
    if super::parse_transform_with_origin("scale(2)", &rendered, reference).is_some() {
        DeclarationValidation::Valid(ComputedValue::Keyword(rendered))
    } else {
        DeclarationValidation::Invalid
    }
}

/// Validates `perspective-origin`.
fn validate_perspective_origin_declaration(value: &Value) -> DeclarationValidation {
    let rendered = render_value(value);
    let reference = validation_transform_reference_box();
    if super::parse_perspective_origin(&rendered, reference).is_some() {
        DeclarationValidation::Valid(ComputedValue::Keyword(rendered))
    } else {
        DeclarationValidation::Invalid
    }
}

fn validate_box_spacing_declaration(name: &str, value: &Value) -> Option<DeclarationValidation> {
    if !is_margin_or_padding_longhand(name) {
        return None;
    }
    Some(if box_spacing_component_is_valid(name, value) {
        DeclarationValidation::Unvalidated
    } else {
        DeclarationValidation::Invalid
    })
}

/// Checks a margin or padding component before shorthand expansion.
pub(super) fn box_spacing_component_is_valid(name: &str, value: &Value) -> bool {
    let margin = name.starts_with("margin-");
    match value {
        Value::Keyword(keyword) => {
            is_css_wide_keyword(&keyword.to_ascii_lowercase())
                || (margin && keyword.eq_ignore_ascii_case("auto"))
        }
        Value::Function { .. } if sibling_functions::contains(value) => {
            // Element-dependent expressions must be checked structurally:
            // their numbers are unavailable while parsing the stylesheet.
            let length = typed_math::NumericType::LENGTH;
            typed_math::infer(value, Some(length)) == Some(length)
        }
        // Math expressions may become negative after resolving relative units.
        // Padding clamps the resulting used value to zero in layout.
        Value::Function { .. } => valid_logical_length(value, true, true),
        value => valid_logical_length(value, margin, true),
    }
}

fn validate_logical_box_declaration(name: &str, value: &Value) -> Option<DeclarationValidation> {
    let is_offset = matches!(
        name,
        "top"
            | "right"
            | "bottom"
            | "left"
            | "inset-inline-start"
            | "inset-inline-end"
            | "inset-block-start"
            | "inset-block-end"
    );
    let is_border_width = matches!(
        name,
        "border-inline-start-width"
            | "border-inline-end-width"
            | "border-block-start-width"
            | "border-block-end-width"
    );
    let is_border_style = matches!(
        name,
        "border-inline-start-style"
            | "border-inline-end-style"
            | "border-block-start-style"
            | "border-block-end-style"
    );
    let is_corner = matches!(
        name,
        "border-start-start-radius"
            | "border-start-end-radius"
            | "border-end-start-radius"
            | "border-end-end-radius"
    );
    if is_offset {
        let valid = matches!(value, Value::Keyword(keyword) if keyword.eq_ignore_ascii_case("auto") || is_css_wide_keyword(&keyword.to_ascii_lowercase()))
            || valid_logical_length(value, true, true);
        return Some(if valid {
            DeclarationValidation::Unvalidated
        } else {
            DeclarationValidation::Invalid
        });
    }
    if is_border_width {
        let valid = matches!(value, Value::Keyword(keyword) if matches!(keyword.to_ascii_lowercase().as_str(), "thin" | "medium" | "thick") || is_css_wide_keyword(&keyword.to_ascii_lowercase()))
            || valid_logical_length(value, false, false);
        return Some(if valid {
            DeclarationValidation::Unvalidated
        } else {
            DeclarationValidation::Invalid
        });
    }
    if is_border_style {
        let valid = matches!(value, Value::Keyword(keyword) if matches!(keyword.to_ascii_lowercase().as_str(), "none" | "hidden" | "dotted" | "dashed" | "solid" | "double" | "groove" | "ridge" | "inset" | "outset") || is_css_wide_keyword(&keyword.to_ascii_lowercase()));
        return Some(if valid {
            DeclarationValidation::Unvalidated
        } else {
            DeclarationValidation::Invalid
        });
    }
    if is_corner {
        let valid = match value {
            Value::List(values) if (1..=2).contains(&values.len()) => values
                .iter()
                .all(|value| valid_logical_length(value, false, true)),
            Value::Keyword(keyword) if is_css_wide_keyword(&keyword.to_ascii_lowercase()) => true,
            value => valid_logical_length(value, false, true),
        };
        return Some(if valid {
            DeclarationValidation::Unvalidated
        } else {
            DeclarationValidation::Invalid
        });
    }
    if matches!(
        name,
        "inset"
            | "inset-inline"
            | "inset-block"
            | "border-inline"
            | "border-block"
            | "border-inline-start"
            | "border-inline-end"
            | "border-block-start"
            | "border-block-end"
            | "border-inline-width"
            | "border-inline-style"
            | "border-inline-color"
            | "border-block-width"
            | "border-block-style"
            | "border-block-color"
    ) {
        return Some(DeclarationValidation::Invalid);
    }
    None
}

fn valid_logical_length(value: &Value, allow_negative: bool, allow_percentage: bool) -> bool {
    match value {
        Value::Length(number, unit) => {
            number.is_finite()
                && (allow_negative || *number >= 0.0)
                && resolve_length_to_px(*number, unit, ResolutionContext::default()).is_some()
        }
        Value::Percentage(number) => {
            allow_percentage && number.is_finite() && (allow_negative || *number >= 0.0)
        }
        Value::Number(number) => *number == 0.0,
        Value::Function { name, .. } if is_length_percentage_math_function(name) => {
            let computed = compute_value(value, "left", ResolutionContext::default());
            match computed {
                ComputedValue::Px(number) | ComputedValue::Number(number) => {
                    number.is_finite() && (allow_negative || number >= 0.0)
                }
                ComputedValue::Percentage(number) => {
                    allow_percentage && number.is_finite() && (allow_negative || number >= 0.0)
                }
                ComputedValue::LengthPercentage(_) => allow_percentage,
                _ => false,
            }
        }
        _ => false,
    }
}

fn validate_scroll_snap_declaration(name: &str, value: &Value) -> Option<DeclarationValidation> {
    let name = name.to_ascii_lowercase();
    if matches!(
        name.as_str(),
        "scroll-padding"
            | "scroll-margin"
            | "scroll-padding-inline"
            | "scroll-padding-block"
            | "scroll-margin-inline"
            | "scroll-margin-block"
    ) {
        // A valid shorthand is expanded before reaching the cascade. Keeping
        // an invalid one intact prevents any of its sides from winning.
        return Some(DeclarationValidation::Invalid);
    }
    if matches!(name.as_str(), "scroll-snap-type" | "scroll-snap-align") {
        let values = match value {
            Value::List(values) => values.as_slice(),
            value => std::slice::from_ref(value),
        };
        let keywords = values
            .iter()
            .map(|value| match value {
                Value::Keyword(keyword) => Some(keyword.to_ascii_lowercase()),
                _ => None,
            })
            .collect::<Option<Vec<_>>>();
        let Some(keywords) = keywords else {
            return Some(DeclarationValidation::Invalid);
        };
        let valid = match (name.as_str(), keywords.as_slice()) {
            (_, [keyword]) if is_css_wide_keyword(keyword) => true,
            ("scroll-snap-type", [axis]) => {
                matches!(
                    axis.as_str(),
                    "none" | "x" | "y" | "block" | "inline" | "both"
                )
            }
            ("scroll-snap-type", [axis, strictness]) => {
                matches!(axis.as_str(), "x" | "y" | "block" | "inline" | "both")
                    && matches!(strictness.as_str(), "mandatory" | "proximity")
            }
            ("scroll-snap-align", [alignment]) => {
                matches!(alignment.as_str(), "none" | "start" | "end" | "center")
            }
            ("scroll-snap-align", [block, inline]) => {
                matches!(block.as_str(), "none" | "start" | "end" | "center")
                    && matches!(inline.as_str(), "none" | "start" | "end" | "center")
            }
            _ => false,
        };
        return Some(if valid {
            DeclarationValidation::Valid(ComputedValue::Keyword(keywords.join(" ")))
        } else {
            DeclarationValidation::Invalid
        });
    }
    let padding = name.starts_with("scroll-padding-");
    let margin = name.starts_with("scroll-margin-");
    if !padding && !margin {
        return None;
    }
    let valid = match value {
        Value::Keyword(keyword) => {
            is_css_wide_keyword(&keyword.to_ascii_lowercase())
                || (padding && keyword.eq_ignore_ascii_case("auto"))
        }
        Value::Length(number, unit) => {
            number.is_finite()
                && (!padding || *number >= 0.0)
                && resolve_length_to_px(*number, unit, ResolutionContext::default()).is_some()
        }
        Value::Percentage(number) => padding && number.is_finite() && *number >= 0.0,
        Value::Number(number) => *number == 0.0,
        Value::Function { name: function, .. } if is_length_percentage_math_function(function) => {
            match compute_value(value, &name, ResolutionContext::default()) {
                ComputedValue::Px(number) => number.is_finite() && (!padding || number >= 0.0),
                ComputedValue::Percentage(number) => padding && number.is_finite() && number >= 0.0,
                ComputedValue::LengthPercentage(_) => padding,
                _ => false,
            }
        }
        _ => false,
    };
    Some(if valid {
        DeclarationValidation::Unvalidated
    } else {
        DeclarationValidation::Invalid
    })
}

pub(super) fn scroll_offset_shorthand_component_is_valid(name: &str, value: &Value) -> bool {
    if matches!(value, Value::Keyword(keyword) if is_css_wide_keyword(&keyword.to_ascii_lowercase()))
    {
        return false;
    }
    matches!(
        validate_scroll_snap_declaration(&format!("{name}-top"), value),
        Some(DeclarationValidation::Valid(_) | DeclarationValidation::Unvalidated)
    )
}

fn validate_contain_intrinsic_size(value: &Value) -> DeclarationValidation {
    if let Value::Keyword(keyword) = value
        && is_css_wide_keyword(&keyword.to_ascii_lowercase())
    {
        return DeclarationValidation::Valid(ComputedValue::Keyword(keyword.to_ascii_lowercase()));
    }
    let values = match value {
        Value::List(values) => values.as_slice(),
        value => std::slice::from_ref(value),
    };
    let valid_fallback = |value: &Value| match value {
        Value::Keyword(keyword) => keyword.eq_ignore_ascii_case("none"),
        Value::Length(number, unit) => {
            number.is_finite()
                && *number >= 0.0
                && resolve_length_to_px(*number, unit, ResolutionContext::default()).is_some()
        }
        Value::Number(number) => *number == 0.0,
        Value::Function { name, .. } => {
            is_length_percentage_math_function(name)
                && matches!(
                    compute_value(value, "width", ResolutionContext::default()),
                    ComputedValue::Px(number) if number.is_finite() && number >= 0.0
                )
        }
        _ => false,
    };
    let valid = match values {
        [fallback] => valid_fallback(fallback),
        [Value::Keyword(auto), fallback] if auto.eq_ignore_ascii_case("auto") => {
            valid_fallback(fallback)
        }
        _ => false,
    };
    if valid {
        DeclarationValidation::Unvalidated
    } else {
        DeclarationValidation::Invalid
    }
}

fn validate_text_underline_position(value: &Value) -> DeclarationValidation {
    if let Value::Keyword(keyword) = value {
        let keyword = keyword.to_ascii_lowercase();
        if keyword == "auto" || is_css_wide_keyword(&keyword) {
            return DeclarationValidation::Valid(ComputedValue::Keyword(keyword));
        }
    }
    let values = match value {
        Value::List(values) => values.as_slice(),
        Value::Keyword(_) => std::slice::from_ref(value),
        _ => return DeclarationValidation::Invalid,
    };
    let mut horizontal = None;
    let mut vertical = None;
    for value in values {
        let Value::Keyword(keyword) = value else {
            return DeclarationValidation::Invalid;
        };
        let keyword = keyword.to_ascii_lowercase();
        match keyword.as_str() {
            "from-font" | "under" if horizontal.is_none() => horizontal = Some(keyword),
            "left" | "right" if vertical.is_none() => vertical = Some(keyword),
            _ => return DeclarationValidation::Invalid,
        }
    }
    if horizontal.is_none() && vertical.is_none() {
        return DeclarationValidation::Invalid;
    }
    DeclarationValidation::Valid(ComputedValue::Keyword(
        horizontal
            .into_iter()
            .chain(vertical)
            .collect::<Vec<_>>()
            .join(" "),
    ))
}

fn validate_text_decoration_thickness(value: &Value) -> DeclarationValidation {
    match value {
        Value::Keyword(keyword)
            if is_css_wide_keyword(&keyword.to_ascii_lowercase())
                || matches!(keyword.to_ascii_lowercase().as_str(), "auto" | "from-font") =>
        {
            DeclarationValidation::Unvalidated
        }
        Value::Length(number, unit)
            if number.is_finite()
                && resolve_length_to_px(*number, unit, ResolutionContext::default()).is_some() =>
        {
            DeclarationValidation::Unvalidated
        }
        Value::Percentage(number) if number.is_finite() => DeclarationValidation::Unvalidated,
        Value::Number(number) if *number == 0.0 => DeclarationValidation::Unvalidated,
        Value::Function { name, .. } if is_length_percentage_math_function(name) => {
            match compute_value(
                value,
                "text-decoration-thickness",
                ResolutionContext::default(),
            ) {
                ComputedValue::Px(_)
                | ComputedValue::Percentage(_)
                | ComputedValue::LengthPercentage(_) => DeclarationValidation::Unvalidated,
                _ => DeclarationValidation::Invalid,
            }
        }
        _ => DeclarationValidation::Invalid,
    }
}

pub(super) fn is_valid_text_decoration_thickness(value: &Value) -> bool {
    !matches!(
        validate_text_decoration_thickness(value),
        DeclarationValidation::Invalid
    )
}

fn validate_sizing_value(name: &str, value: &Value) -> DeclarationValidation {
    let valid_keyword = |keyword: &str| {
        let keyword = keyword.to_ascii_lowercase();
        is_css_wide_keyword(&keyword)
            || matches!(
                keyword.as_str(),
                "min-content" | "max-content" | "fit-content" | "stretch"
            )
            || (keyword == "auto" && !matches!(name, "max-inline-size" | "max-block-size"))
            || (name.starts_with("max-") && keyword == "none")
    };
    match value {
        Value::Keyword(keyword) if valid_keyword(keyword) => DeclarationValidation::Unvalidated,
        Value::Length(number, unit)
            if *number >= 0.0
                && resolve_length_to_px(*number, unit, ResolutionContext::default()).is_some() =>
        {
            DeclarationValidation::Unvalidated
        }
        Value::Percentage(number) if *number >= 0.0 => DeclarationValidation::Unvalidated,
        Value::Number(number) if *number == 0.0 => DeclarationValidation::Unvalidated,
        Value::Function { name: function, .. } if is_length_percentage_math_function(function) => {
            if typed_math::infer(value, Some(typed_math::NumericType::LENGTH))
                == Some(typed_math::NumericType::LENGTH)
            {
                DeclarationValidation::Unvalidated
            } else {
                DeclarationValidation::Invalid
            }
        }
        _ => DeclarationValidation::Invalid,
    }
}

fn is_length_percentage_math_function(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "calc" | "min" | "max" | "clamp" | "round" | "hypot" | "abs" | "mod" | "rem"
    )
}

fn is_non_negative_sizing_property(name: &str) -> bool {
    matches!(
        name,
        "width"
            | "height"
            | "inline-size"
            | "block-size"
            | "min-inline-size"
            | "min-block-size"
            | "max-inline-size"
            | "max-block-size"
            | "min-width"
            | "min-height"
            | "max-width"
            | "max-height"
            | "column-width"
            | "column-gap"
            | "column-rule-width"
            | "shape-margin"
            | "tab-size"
    )
}

fn is_position_offset_property(name: &str) -> bool {
    matches!(
        name,
        "top"
            | "right"
            | "bottom"
            | "left"
            | "inset-inline-start"
            | "inset-inline-end"
            | "inset-block-start"
            | "inset-block-end"
    )
}

fn validate_color_scheme_declaration(value: &Value) -> DeclarationValidation {
    let rendered = render_value(value);
    let words = rendered.split_ascii_whitespace().collect::<Vec<_>>();
    if words.len() == 1
        && (is_css_wide_keyword(&words[0].to_ascii_lowercase())
            || words[0].eq_ignore_ascii_case("normal"))
    {
        return DeclarationValidation::Valid(ComputedValue::Keyword(words[0].to_ascii_lowercase()));
    }
    if words.is_empty() {
        return DeclarationValidation::Invalid;
    }
    let mut schemes = Vec::new();
    let mut only = false;
    for word in words {
        if word.eq_ignore_ascii_case("only") {
            if only {
                return DeclarationValidation::Invalid;
            }
            only = true;
            continue;
        }
        if is_css_wide_keyword(&word.to_ascii_lowercase()) || word.eq_ignore_ascii_case("normal") {
            return DeclarationValidation::Invalid;
        }
        schemes.push(word.to_ascii_lowercase());
    }
    if schemes.is_empty() {
        return DeclarationValidation::Invalid;
    }
    if only {
        schemes.push("only".to_string());
    }
    DeclarationValidation::Valid(ComputedValue::Keyword(schemes.join(" ")))
}

fn validate_touch_action_declaration(value: &Value) -> DeclarationValidation {
    let rendered = render_value(value).to_ascii_lowercase();
    if is_css_wide_keyword(&rendered) {
        return DeclarationValidation::Valid(ComputedValue::Keyword(rendered));
    }
    if matches!(rendered.as_str(), "auto" | "none" | "manipulation") {
        return DeclarationValidation::Valid(ComputedValue::Keyword(rendered));
    }
    let words = rendered.split_ascii_whitespace().collect::<Vec<_>>();
    if words.is_empty()
        || words.iter().any(|word| {
            !matches!(
                *word,
                "pan-x" | "pan-y" | "pan-left" | "pan-right" | "pan-up" | "pan-down" | "pinch-zoom"
            )
        })
    {
        return DeclarationValidation::Invalid;
    }
    let horizontal = words
        .iter()
        .filter(|&&word| matches!(word, "pan-x" | "pan-left" | "pan-right"))
        .count();
    let vertical = words
        .iter()
        .filter(|&&word| matches!(word, "pan-y" | "pan-up" | "pan-down"))
        .count();
    let pinch = words.iter().filter(|&&word| word == "pinch-zoom").count();
    if horizontal > 1 || vertical > 1 || pinch > 1 {
        return DeclarationValidation::Invalid;
    }
    let canonical = [
        "pan-x",
        "pan-left",
        "pan-right",
        "pan-y",
        "pan-up",
        "pan-down",
        "pinch-zoom",
    ]
    .into_iter()
    .filter(|candidate| words.contains(candidate))
    .collect::<Vec<_>>()
    .join(" ");
    DeclarationValidation::Valid(ComputedValue::Keyword(canonical))
}

fn validate_scrollbar_color_declaration(value: &Value) -> DeclarationValidation {
    if matches!(value, Value::Keyword(keyword)
        if keyword.eq_ignore_ascii_case("auto")
            || is_css_wide_keyword(&keyword.to_ascii_lowercase()))
    {
        return DeclarationValidation::Valid(ComputedValue::Keyword(
            render_value(value).to_ascii_lowercase(),
        ));
    }
    let Value::List(colors) = value else {
        return DeclarationValidation::Invalid;
    };
    if colors.len() != 2
        || colors
            .iter()
            .any(|color| matches!(validate_color_value(color), DeclarationValidation::Invalid))
    {
        return DeclarationValidation::Invalid;
    }
    DeclarationValidation::Valid(ComputedValue::Keyword(
        colors
            .iter()
            .map(render_value)
            .collect::<Vec<_>>()
            .join(" "),
    ))
}

fn validate_overflow_clip_margin_declaration(value: &Value) -> DeclarationValidation {
    let values = match value {
        Value::List(values) => values.as_slice(),
        value => std::slice::from_ref(value),
    };
    if values.len() == 1
        && matches!(&values[0], Value::Keyword(keyword)
            if is_css_wide_keyword(&keyword.to_ascii_lowercase()))
    {
        return DeclarationValidation::Unvalidated;
    }
    if values.is_empty() || values.len() > 2 {
        return DeclarationValidation::Invalid;
    }
    let mut boxes = 0;
    let mut lengths = 0;
    for component in values {
        match component {
            Value::Keyword(keyword)
                if matches!(
                    keyword.to_ascii_lowercase().as_str(),
                    "content-box" | "padding-box" | "border-box"
                ) =>
            {
                boxes += 1;
            }
            Value::Length(number, unit)
                if number.is_finite()
                    && resolve_length_to_px(*number, unit, ResolutionContext::default())
                        .is_some() =>
            {
                lengths += 1;
            }
            Value::Number(number) if *number == 0.0 => lengths += 1,
            Value::Function { name, .. }
                if is_length_percentage_math_function(name)
                    && typed_math::infer(component, Some(typed_math::NumericType::LENGTH))
                        == Some(typed_math::NumericType::LENGTH) =>
            {
                lengths += 1;
            }
            _ => return DeclarationValidation::Invalid,
        }
    }
    if boxes <= 1 && lengths <= 1 {
        DeclarationValidation::Unvalidated
    } else {
        DeclarationValidation::Invalid
    }
}

pub(crate) fn normalize_css_ui_specified(property: &str, text: &str) -> Option<String> {
    if !matches!(
        property,
        "accent-color"
            | "appearance"
            | "caret-color"
            | "color-scheme"
            | "field-sizing"
            | "resize"
            | "touch-action"
            | "user-select"
    ) {
        return None;
    }
    let lower = text.trim().to_ascii_lowercase();
    if matches!(property, "accent-color" | "caret-color")
        && !matches!(
            lower.as_str(),
            "auto" | "currentcolor" | "initial" | "inherit" | "unset" | "revert" | "revert-layer"
        )
    {
        if lower.starts_with('#') {
            let color = crate::paint::color::parse_color(&lower)?;
            return Some(if color.a == 255 {
                format!("rgb({}, {}, {})", color.r, color.g, color.b)
            } else {
                format!(
                    "rgba({}, {}, {}, {})",
                    color.r,
                    color.g,
                    color.b,
                    super::tokenizer::trimmed_number(color.a as f32 / 255.0)
                )
            });
        }
        return crate::paint::color4::CssColor::parse(text)
            .map(|color| color.serialize())
            .or_else(|| crate::paint::color::parse_color(text).map(|_| lower));
    }
    if !supports_declaration(property, text) {
        return None;
    }
    if matches!(property, "color-scheme" | "touch-action") {
        let declaration = crate::css::parse_style_attribute(&format!("{property}: {text}"))
            .into_iter()
            .find(|declaration| declaration.name == property)?;
        let validation = if property == "color-scheme" {
            validate_color_scheme_declaration(&declaration.value)
        } else {
            validate_touch_action_declaration(&declaration.value)
        };
        return match validation {
            DeclarationValidation::Valid(value) => Some(value.css_text()),
            _ => None,
        };
    }
    Some(lower)
}

pub(crate) fn normalize_scroll_ui_specified(property: &str, text: &str) -> Option<String> {
    if !matches!(
        property,
        "scrollbar-width"
            | "scrollbar-gutter"
            | "scrollbar-color"
            | "overflow-anchor"
            | "overflow-clip-margin"
    ) {
        return None;
    }
    let declaration = crate::css::parse_style_attribute(&format!("{property}: {text}"))
        .into_iter()
        .find(|declaration| declaration.name == property)?;
    let validation = validate_declaration(property, &declaration.value);
    match validation {
        DeclarationValidation::Invalid => None,
        DeclarationValidation::Valid(value) if property == "scrollbar-gutter" => {
            let words = value.css_text();
            Some(if words.split_ascii_whitespace().count() == 2 {
                "stable both-edges".to_string()
            } else {
                words
            })
        }
        DeclarationValidation::Valid(value) if property == "scrollbar-color" => {
            if value.css_text().eq_ignore_ascii_case("auto")
                || is_css_wide_keyword(&value.css_text().to_ascii_lowercase())
            {
                return Some(value.css_text());
            }
            let css_text = value.css_text();
            let colors = crate::css::split_top_level_whitespace(&css_text);
            Some(
                colors
                    .into_iter()
                    .map(|color| {
                        crate::paint::color4::CssColor::parse(color)
                            .map(|color| color.serialize())
                            .or_else(|| {
                                crate::paint::color::parse_color(color).map(|parsed| {
                                    if color.starts_with('#') {
                                        format!("rgb({}, {}, {})", parsed.r, parsed.g, parsed.b)
                                    } else {
                                        color.to_ascii_lowercase()
                                    }
                                })
                            })
                            .unwrap_or_else(|| color.to_ascii_lowercase())
                    })
                    .collect::<Vec<_>>()
                    .join(" "),
            )
        }
        DeclarationValidation::Valid(value) => Some(value.css_text()),
        DeclarationValidation::Unvalidated => {
            Some(render_value(&declaration.value).to_ascii_lowercase())
        }
    }
}

/// Valid `cursor` keyword set: the CSS 2.1 values plus the CSS UI Level 3 / 4
/// additions. Includes every keyword exercised by Acid3 test 47 and the common
/// CSS3 extras (`grab`/`grabbing`, `zoom-in`/`zoom-out`).
fn is_valid_cursor_keyword(keyword: &str) -> bool {
    matches!(
        keyword,
        "auto"
            | "default"
            | "none"
            | "context-menu"
            | "help"
            | "pointer"
            | "progress"
            | "wait"
            | "cell"
            | "crosshair"
            | "text"
            | "vertical-text"
            | "alias"
            | "copy"
            | "move"
            | "no-drop"
            | "not-allowed"
            | "grab"
            | "grabbing"
            | "e-resize"
            | "n-resize"
            | "ne-resize"
            | "nw-resize"
            | "s-resize"
            | "se-resize"
            | "sw-resize"
            | "w-resize"
            | "ew-resize"
            | "ns-resize"
            | "nesw-resize"
            | "nwse-resize"
            | "col-resize"
            | "row-resize"
            | "all-scroll"
            | "zoom-in"
            | "zoom-out"
    )
}

/// Validates a `cursor` declaration value and returns its normalized computed
/// value, or `None` if the value is invalid.
///
/// Per the CSS UI `cursor` grammar `[ <url> [ <x> <y> ]? , ]* <keyword>`, the
/// value is a comma-separated list of `url()` groups followed by a mandatory
/// trailing keyword. Each group is a `url()` reference optionally followed by a
/// hotspot coordinate **pair** `<x> <y>` (never a lone coordinate). Coordinates
/// may only appear directly after a `url()`. Any grammar violation — a
/// coordinate with no preceding `url()`, an odd number of coordinates, or an
/// unexpected token — makes the whole declaration invalid (returns `None`, so
/// the cascade falls back to the initial value `auto`). The trailing keyword is
/// validated against the supported set and normalized to lowercase; CSS-wide
/// keywords pass through for resolution by later cascade passes.
///
/// The CSS parser discards commas from the token stream, so the group structure
/// is reconstructed positionally: each `url()` starts a new group and consumes
/// the run of coordinate tokens that immediately follows it. Serialization
/// re-inserts the group-separating commas (`url(a), url(b) 1 2, pointer`).
fn compute_cursor_value(value: &Value) -> Option<ComputedValue> {
    match value {
        Value::Keyword(keyword) => {
            let lower = keyword.to_ascii_lowercase();
            if is_css_wide_keyword(&lower) || is_valid_cursor_keyword(&lower) {
                Some(ComputedValue::Keyword(lower))
            } else {
                None
            }
        }
        Value::List(items) => {
            // The final component must be a valid, non-CSS-wide cursor keyword.
            let (last, leading) = items.split_last()?;
            let Value::Keyword(keyword) = last else {
                return None;
            };
            let lower = keyword.to_ascii_lowercase();
            if !is_valid_cursor_keyword(&lower) {
                return None;
            }
            if leading.is_empty() {
                return Some(ComputedValue::Keyword(lower));
            }
            // Parse the leading `url()` groups positionally. Each group must
            // start with a `url()` reference (the parser renders these as
            // `Keyword("url(...)")` or, defensively, a `url` function) and may
            // be followed by exactly zero or two coordinate tokens.
            let mut groups: Vec<String> = Vec::new();
            let mut index = 0;
            while index < leading.len() {
                let is_url = match &leading[index] {
                    Value::Keyword(k) => k.to_ascii_lowercase().starts_with("url("),
                    Value::Function { name, .. } => name.eq_ignore_ascii_case("url"),
                    _ => false,
                };
                if !is_url {
                    // A coordinate (or anything else) with no preceding `url()`.
                    return None;
                }
                let url = render_value(&leading[index]);
                index += 1;

                // Consume the coordinate run that follows this `url()`.
                let mut coords: Vec<String> = Vec::new();
                while index < leading.len() {
                    match &leading[index] {
                        Value::Number(_) | Value::Length(_, _) => {
                            coords.push(render_value(&leading[index]));
                            index += 1;
                        }
                        _ => break,
                    }
                }
                // Coordinates are only valid as an `<x> <y>` pair.
                if coords.len() != 2 && !coords.is_empty() {
                    return None;
                }

                if coords.is_empty() {
                    groups.push(url);
                } else {
                    groups.push(format!("{url} {}", coords.join(" ")));
                }
            }

            let prefix = groups.join(", ");
            Some(ComputedValue::Keyword(format!("{prefix}, {lower}")))
        }
        _ => None,
    }
}

#[derive(Debug, Clone)]
struct Candidate {
    name: String,
    prefixed_alias: bool,
    value: Value,
    important: bool,
    origin: Origin,
    inline: bool,
    specificity: Specificity,
    /// Ancestor hops from the styled element to the applicable scoping root.
    scope_proximity: Option<usize>,
    source_order: usize,
    /// First declaration position of the originating style rule.
    rule_order: usize,
    /// Position of this declaration's tree scope in tree-of-trees order.
    /// Encapsulation order reverses for important declarations.
    encapsulation_order: usize,
    layer_context: LayerContextKey,
    layer_path: Option<LayerPath>,
    layer_order: Vec<usize>,
}

#[derive(Debug, Clone)]
struct ActiveScope {
    roots: Vec<ScopeRoot>,
}

#[derive(Debug, Clone)]
struct ScopeRoot {
    node: NodeHandle,
    proximity: usize,
}

struct ElementMatchKeys {
    id: Option<String>,
    classes: HashSet<String>,
    tag_name: String,
}

#[derive(Debug, Default)]
struct StylesheetRuleIndex {
    by_id: HashMap<String, Vec<usize>>,
    by_class: HashMap<String, Vec<usize>>,
    by_tag: HashMap<String, Vec<usize>>,
    fallback: Vec<usize>,
    declaration_offsets: Vec<usize>,
    total_declarations: usize,
    has_part_selector: bool,
}

impl StylesheetRuleIndex {
    fn build(stylesheet: &Stylesheet) -> Self {
        let mut index = Self::default();
        let mut offset = 0;
        for (rule_index, rule) in stylesheet.rules.iter().enumerate() {
            index.declaration_offsets.push(offset);
            offset += match rule {
                Rule::Style(rule) => {
                    index.has_part_selector |= rule.selectors.iter().any(selector_uses_part_pseudo)
                        || rules_contain_part_selector(&rule.rules);
                    rule.declarations.len() + count_declarations(&rule.rules)
                }
                Rule::At(rule) => {
                    if let Some(block) = rule.block.as_deref() {
                        index.has_part_selector |= rules_contain_part_selector(block);
                        count_declarations(block)
                    } else {
                        rule.declarations.len()
                    }
                }
                Rule::FontFace(_) => 0,
            };
            let Rule::Style(style_rule) = rule else {
                continue;
            };
            if !style_rule.rules.is_empty() {
                // A nested selector can match even when its parent selector
                // does not match this node (for example `.a { .b {} }`).
                index.fallback.push(rule_index);
                continue;
            }
            let keys: Option<Vec<RuleMatchKey>> = style_rule
                .selectors
                .iter()
                .map(selector_match_key)
                .collect();
            let Some(keys) = keys else {
                index.fallback.push(rule_index);
                continue;
            };
            for key in keys {
                let bucket = match key {
                    RuleMatchKey::Id(value) => index.by_id.entry(value).or_default(),
                    RuleMatchKey::Class(value) => index.by_class.entry(value).or_default(),
                    RuleMatchKey::Tag(value) => index.by_tag.entry(value).or_default(),
                };
                if bucket.last() != Some(&rule_index) {
                    bucket.push(rule_index);
                }
            }
        }
        index.total_declarations = offset;
        index
    }

    fn candidates(&self, keys: &ElementMatchKeys) -> BTreeSet<usize> {
        let mut candidates = self.fallback.iter().copied().collect::<BTreeSet<_>>();
        if let Some(id) = &keys.id
            && let Some(rules) = self.by_id.get(id)
        {
            candidates.extend(rules);
        }
        for class in &keys.classes {
            if let Some(rules) = self.by_class.get(class) {
                candidates.extend(rules);
            }
        }
        if let Some(rules) = self.by_tag.get(&keys.tag_name.to_ascii_lowercase()) {
            candidates.extend(rules);
        }
        candidates
    }
}

enum RuleMatchKey {
    Id(String),
    Class(String),
    Tag(String),
}

fn selector_match_key(selector: &super::Selector) -> Option<RuleMatchKey> {
    if selector_uses_shadow_pseudo(selector) {
        return None;
    }
    let rightmost = selector.parts.last()?;
    rightmost
        .simples
        .iter()
        .find_map(|simple| match simple {
            SimpleSelector::Id(value) => Some(RuleMatchKey::Id(value.clone())),
            _ => None,
        })
        .or_else(|| {
            rightmost.simples.iter().find_map(|simple| match simple {
                SimpleSelector::Class(value) => Some(RuleMatchKey::Class(value.clone())),
                _ => None,
            })
        })
        .or_else(|| {
            rightmost.simples.iter().find_map(|simple| match simple {
                SimpleSelector::Type(value) => Some(RuleMatchKey::Tag(value.to_ascii_lowercase())),
                _ => None,
            })
        })
}

impl ElementMatchKeys {
    fn from_node(node: &NodeHandle) -> Option<Self> {
        Some(Self {
            id: node.get_attribute("id"),
            classes: node
                .get_attribute("class")
                .map(|value| value.split_ascii_whitespace().map(str::to_string).collect())
                .unwrap_or_default(),
            tag_name: node.tag_name()?,
        })
    }
}

fn style_rule_might_match(style_rule: &super::StyleRule, keys: &ElementMatchKeys) -> bool {
    style_rule.selectors.iter().any(|selector| {
        if selector_uses_shadow_pseudo(selector) {
            return true;
        }
        let Some(rightmost) = selector.parts.last() else {
            return false;
        };
        rightmost.simples.iter().all(|simple| match simple {
            SimpleSelector::Id(id) => keys.id.as_deref() == Some(id.as_str()),
            SimpleSelector::Class(class) => keys.classes.contains(class),
            SimpleSelector::Type(tag) => tag.eq_ignore_ascii_case(&keys.tag_name),
            _ => true,
        })
    })
}

fn selector_uses_shadow_pseudo(selector: &super::Selector) -> bool {
    selector.parts.iter().any(|part| {
        part.simples.iter().any(|simple| match simple {
            SimpleSelector::PseudoClass(name) => {
                name.eq_ignore_ascii_case("host")
                    || functional_selector(name)
                        .is_some_and(|(function, _)| function.eq_ignore_ascii_case("host"))
            }
            SimpleSelector::PseudoElement(name) => {
                functional_selector(name).is_some_and(|(function, _)| {
                    function.eq_ignore_ascii_case("slotted")
                        || function.eq_ignore_ascii_case("part")
                })
            }
            _ => false,
        })
    })
}

fn rules_contain_part_selector(rules: &[Rule]) -> bool {
    rules.iter().any(|rule| match rule {
        Rule::Style(rule) => {
            rule.selectors.iter().any(selector_uses_part_pseudo)
                || rules_contain_part_selector(&rule.rules)
        }
        Rule::At(rule) => rule
            .block
            .as_deref()
            .is_some_and(rules_contain_part_selector),
        Rule::FontFace(_) => false,
    })
}

fn selector_uses_part_pseudo(selector: &Selector) -> bool {
    selector.parts.iter().any(|part| {
        part.simples.iter().any(|simple| {
            matches!(simple, SimpleSelector::PseudoElement(name) if functional_selector(name).is_some_and(|(function, _)| function.eq_ignore_ascii_case("part")))
        })
    })
}

fn functional_selector(name: &str) -> Option<(&str, &str)> {
    let open = name.find('(')?;
    Some((&name[..open], name[open + 1..].strip_suffix(')')?.trim()))
}

fn matches_shadow_scoped_selector(
    node: &NodeHandle,
    selector: &super::Selector,
    scope: &NodeHandle,
    pseudo: Option<PseudoElement>,
    cache: &mut SelectorMatchCache,
) -> bool {
    if selector_uses_part_pseudo(selector) {
        return matches_part_selector(node, selector, Some(scope), pseudo, cache);
    }
    if selector.parts.iter().any(|part| {
        part.simples.iter().any(|simple| {
            matches!(simple, SimpleSelector::PseudoClass(name) if name.eq_ignore_ascii_case("host") || functional_selector(name).is_some_and(|(function, _)| function.eq_ignore_ascii_case("host")))
        })
    }) {
        return matches_host_selector(node, selector, scope, pseudo, cache);
    }

    if selector.parts.iter().any(|part| {
        part.simples.iter().any(|simple| {
            matches!(simple, SimpleSelector::PseudoElement(name) if functional_selector(name).is_some_and(|(function, _)| function.eq_ignore_ascii_case("slotted")))
        })
    }) {
        return pseudo.is_none() && matches_slotted_selector(node, selector, scope, cache);
    }

    node.containing_shadow_root().as_ref() == Some(scope)
        && matches_selector_with_pseudo_cached(node, selector, pseudo, cache)
}

fn matches_host_selector(
    node: &NodeHandle,
    selector: &super::Selector,
    scope: &NodeHandle,
    pseudo: Option<PseudoElement>,
    cache: &mut SelectorMatchCache,
) -> bool {
    if selector.parts.is_empty() {
        return false;
    }
    matches_host_selector_part(
        node,
        selector,
        selector.parts.len() - 1,
        scope,
        pseudo,
        cache,
    )
}

fn matches_host_selector_part(
    node: &NodeHandle,
    selector: &Selector,
    index: usize,
    scope: &NodeHandle,
    pseudo: Option<PseudoElement>,
    cache: &mut SelectorMatchCache,
) -> bool {
    let part = &selector.parts[index];
    if !matches_shadow_compound(node, part, scope, pseudo, cache) {
        return false;
    }

    let Some(combinator) = part.combinator else {
        return true;
    };
    if index == 0 {
        return false;
    }

    match combinator {
        Combinator::Descendant => {
            let mut ancestor = shadow_selector_parent(node, scope);
            while let Some(parent) = ancestor {
                if matches_host_selector_part(&parent, selector, index - 1, scope, None, cache) {
                    return true;
                }
                ancestor = shadow_selector_parent(&parent, scope);
            }
            false
        }
        Combinator::Child => shadow_selector_parent(node, scope).is_some_and(|parent| {
            matches_host_selector_part(&parent, selector, index - 1, scope, None, cache)
        }),
        Combinator::AdjacentSibling => previous_element_sibling(node).is_some_and(|sibling| {
            matches_host_selector_part(&sibling, selector, index - 1, scope, None, cache)
        }),
        Combinator::GeneralSibling => {
            let Some(parent) = node.parent_node() else {
                return false;
            };
            let siblings = parent.child_nodes();
            let Some(position) = siblings.iter().position(|candidate| candidate == node) else {
                return false;
            };
            siblings[..position].iter().rev().any(|sibling| {
                sibling.node_type() == NodeType::Element
                    && matches_host_selector_part(sibling, selector, index - 1, scope, None, cache)
            })
        }
    }
}

fn matches_shadow_compound(
    node: &NodeHandle,
    part: &SelectorPart,
    scope: &NodeHandle,
    pseudo: Option<PseudoElement>,
    cache: &mut SelectorMatchCache,
) -> bool {
    let mut ordinary_part = part.clone();
    ordinary_part.combinator = None;
    let mut has_host = false;
    ordinary_part.simples.retain(|simple| {
        let SimpleSelector::PseudoClass(name) = simple else {
            return true;
        };
        let is_host = name.eq_ignore_ascii_case("host")
            || functional_selector(name)
                .is_some_and(|(function, _)| function.eq_ignore_ascii_case("host"));
        has_host |= is_host;
        !is_host
    });

    if has_host {
        let Some(host) = scope.shadow_host() else {
            return false;
        };
        if node != &host || !host_arguments_match(&host, part, cache) {
            return false;
        }
    } else if node.containing_shadow_root().as_ref() != Some(scope) {
        return false;
    }

    if ordinary_part.simples.is_empty() {
        return pseudo.is_none();
    }
    matches_selector_with_pseudo_cached(
        node,
        &Selector {
            parts: vec![ordinary_part],
        },
        pseudo,
        cache,
    )
}

fn host_arguments_match(
    host: &NodeHandle,
    part: &SelectorPart,
    cache: &mut SelectorMatchCache,
) -> bool {
    part.simples.iter().all(|simple| {
        let SimpleSelector::PseudoClass(name) = simple else {
            return true;
        };
        if name.eq_ignore_ascii_case("host") {
            return true;
        }
        let Some((function, argument)) = functional_selector(name) else {
            return true;
        };
        if !function.eq_ignore_ascii_case("host") {
            return true;
        }
        super::parse_selector_list(argument)
            .ok()
            .is_some_and(|selectors| {
                selectors.len() == 1
                    && selectors[0].parts.len() == 1
                    && matches_selector_with_pseudo_cached(host, &selectors[0], None, cache)
            })
    })
}

fn shadow_selector_parent(node: &NodeHandle, scope: &NodeHandle) -> Option<NodeHandle> {
    let parent = node.parent_node()?;
    if &parent == scope {
        scope.shadow_host()
    } else {
        Some(parent)
    }
}

fn previous_element_sibling(node: &NodeHandle) -> Option<NodeHandle> {
    let parent = node.parent_node()?;
    let siblings = parent.child_nodes();
    let position = siblings.iter().position(|candidate| candidate == node)?;
    siblings[..position]
        .iter()
        .rev()
        .find(|sibling| sibling.node_type() == NodeType::Element)
        .cloned()
}

fn matches_slotted_selector(
    node: &NodeHandle,
    selector: &super::Selector,
    scope: &NodeHandle,
    cache: &mut SelectorMatchCache,
) -> bool {
    let Some(slot) = assigned_slot_in_scope(node, scope) else {
        return false;
    };

    let mut slot_selector = selector.clone();
    let mut argument = None;
    for part in &mut slot_selector.parts {
        part.simples.retain(|simple| {
            let SimpleSelector::PseudoElement(name) = simple else {
                return true;
            };
            let Some((function, candidate)) = functional_selector(name) else {
                return true;
            };
            if !function.eq_ignore_ascii_case("slotted") || argument.is_some() {
                return true;
            }
            argument = Some(candidate.to_string());
            false
        });
    }
    let Some(argument) = argument else {
        return false;
    };
    let argument_matches = super::parse_selector_list(&argument)
        .ok()
        .is_some_and(|selectors| {
            selectors.len() == 1
                && selectors[0].parts.len() == 1
                && matches_selector_with_pseudo_cached(node, &selectors[0], None, cache)
        });
    if !argument_matches {
        return false;
    }

    slot_selector.parts.retain(|part| !part.simples.is_empty());
    if slot_selector.parts.is_empty() {
        true
    } else {
        slot_selector.parts[0].combinator = None;
        matches_selector_with_pseudo_cached(&slot, &slot_selector, None, cache)
    }
}

fn assigned_slot_in_scope(node: &NodeHandle, scope: &NodeHandle) -> Option<NodeHandle> {
    let mut current = node.clone();
    while let Some(slot) = current.assigned_slot() {
        if slot.containing_shadow_root().as_ref() == Some(scope) {
            return Some(slot);
        }
        current = slot;
    }
    None
}

fn flattened_assigned_slot(node: &NodeHandle) -> Option<NodeHandle> {
    let mut current = node.clone();
    let mut outermost = None;
    while let Some(slot) = current.assigned_slot() {
        current = slot.clone();
        outermost = Some(slot);
    }
    outermost
}

#[allow(clippy::too_many_arguments)]
fn collect_indexed_rule_candidates(
    node: &NodeHandle,
    rules: &[Rule],
    index: &StylesheetRuleIndex,
    origin: Origin,
    stylesheet_id: usize,
    layer_context: LayerContextKey,
    layer_order: &CascadeLayerOrder,
    pseudo: Option<PseudoElement>,
    source_order: &mut usize,
    out: &mut Vec<Candidate>,
    viewport_width: f32,
    viewport_height: f32,
    color_scheme_dark: bool,
    media_type: MediaType,
    media_environment: &MediaEnvironment,
    media_cache: &mut HashMap<String, Vec<MediaQuery>>,
    scope_cache: &mut HashMap<String, Option<super::ScopePrelude>>,
    container_cache: &mut HashMap<String, Option<super::ContainerQuery>>,
    container_contexts: &HashMap<usize, ContainerContext>,
    element_keys: Option<&ElementMatchKeys>,
    selector_cache: &mut SelectorMatchCache,
    shadow_scope: Option<&NodeHandle>,
    implicit_scope_root: Option<&NodeHandle>,
    encapsulation_order: usize,
) {
    let Some(element_keys) = element_keys else {
        collect_rule_candidates(
            node,
            rules,
            origin,
            stylesheet_id,
            layer_context,
            layer_order,
            pseudo,
            source_order,
            out,
            viewport_width,
            viewport_height,
            color_scheme_dark,
            media_type,
            media_environment,
            media_cache,
            scope_cache,
            container_cache,
            container_contexts,
            None,
            selector_cache,
            shadow_scope,
            implicit_scope_root,
            encapsulation_order,
            None,
            None,
        );
        return;
    };

    let base_order = *source_order;
    for rule_index in index.candidates(element_keys) {
        let mut rule_order = base_order + index.declaration_offsets[rule_index];
        collect_rule_candidates(
            node,
            &rules[rule_index..=rule_index],
            origin,
            stylesheet_id,
            layer_context,
            layer_order,
            pseudo,
            &mut rule_order,
            out,
            viewport_width,
            viewport_height,
            color_scheme_dark,
            media_type,
            media_environment,
            media_cache,
            scope_cache,
            container_cache,
            container_contexts,
            Some(element_keys),
            selector_cache,
            shadow_scope,
            implicit_scope_root,
            encapsulation_order,
            None,
            None,
        );
    }
    for (rule_index, rule) in rules.iter().enumerate() {
        if !matches!(rule, Rule::At(_)) {
            continue;
        }
        let mut rule_order = base_order + index.declaration_offsets[rule_index];
        collect_rule_candidates(
            node,
            &rules[rule_index..=rule_index],
            origin,
            stylesheet_id,
            layer_context,
            layer_order,
            pseudo,
            &mut rule_order,
            out,
            viewport_width,
            viewport_height,
            color_scheme_dark,
            media_type,
            media_environment,
            media_cache,
            scope_cache,
            container_cache,
            container_contexts,
            Some(element_keys),
            selector_cache,
            shadow_scope,
            implicit_scope_root,
            encapsulation_order,
            None,
            None,
        );
    }
    *source_order += index.total_declarations;
}

fn collect_rule_candidates(
    node: &NodeHandle,
    rules: &[Rule],
    origin: Origin,
    stylesheet_id: usize,
    layer_context: LayerContextKey,
    layer_order: &CascadeLayerOrder,
    pseudo: Option<PseudoElement>,
    source_order: &mut usize,
    out: &mut Vec<Candidate>,
    viewport_width: f32,
    viewport_height: f32,
    color_scheme_dark: bool,
    media_type: MediaType,
    media_environment: &MediaEnvironment,
    media_cache: &mut HashMap<String, Vec<MediaQuery>>,
    scope_cache: &mut HashMap<String, Option<super::ScopePrelude>>,
    container_cache: &mut HashMap<String, Option<super::ContainerQuery>>,
    container_contexts: &HashMap<usize, ContainerContext>,
    element_keys: Option<&ElementMatchKeys>,
    selector_cache: &mut SelectorMatchCache,
    shadow_scope: Option<&NodeHandle>,
    implicit_scope_root: Option<&NodeHandle>,
    encapsulation_order: usize,
    active_scope: Option<&ActiveScope>,
    active_layer: Option<&LayerPath>,
) {
    if node.node_type() != NodeType::Element {
        return;
    }

    for rule in rules {
        match rule {
            Rule::Style(style_rule) => {
                let rule_order = *source_order;
                let parent_might_match =
                    element_keys.is_none_or(|keys| style_rule_might_match(style_rule, keys));
                let mut matching = None;
                for selector in &style_rule.selectors {
                    if !parent_might_match {
                        break;
                    }
                    let selector_specificity = specificity(selector);
                    if let Some(active) = active_scope {
                        for root in &active.roots {
                            if root.node == *node && !selector_references_scope(selector) {
                                continue;
                            }
                            if matches_selector_with_scope_cached(
                                node,
                                selector,
                                pseudo,
                                selector_cache,
                                Some(&root.node),
                            ) {
                                retain_best_scoped_match(
                                    &mut matching,
                                    selector_specificity,
                                    root.proximity,
                                );
                            }
                        }
                    } else {
                        let matches = if origin == Origin::Author
                            && shadow_scope.is_none()
                            && node.containing_shadow_root().is_some()
                        {
                            selector_uses_part_pseudo(selector)
                                && matches_part_selector(
                                    node,
                                    selector,
                                    None,
                                    pseudo,
                                    selector_cache,
                                )
                        } else if let Some(scope) = shadow_scope {
                            matches_shadow_scoped_selector(
                                node,
                                selector,
                                scope,
                                pseudo,
                                selector_cache,
                            )
                        } else {
                            matches_selector_with_pseudo_cached(
                                node,
                                selector,
                                pseudo,
                                selector_cache,
                            )
                        };
                        if matches {
                            retain_best_scoped_match(
                                &mut matching,
                                selector_specificity,
                                usize::MAX,
                            );
                        }
                    }
                }

                if let Some((specificity, proximity)) = matching {
                    let layer_rank = layer_order.rank(active_layer);
                    for declaration in &style_rule.declarations {
                        out.push(Candidate {
                            name: canonical_property_name(&declaration.name).to_string(),
                            prefixed_alias: is_prefixed_property_alias(&declaration.name),
                            value: declaration.value.clone(),
                            important: declaration.important,
                            origin,
                            inline: false,
                            specificity,
                            scope_proximity: active_scope.map(|_| proximity),
                            source_order: *source_order,
                            rule_order,
                            encapsulation_order,
                            layer_context,
                            layer_path: active_layer.cloned(),
                            layer_order: layer_rank.clone(),
                        });
                        *source_order += 1;
                    }
                } else {
                    *source_order += style_rule.declarations.len();
                }
                if !style_rule.rules.is_empty() {
                    collect_rule_candidates(
                        node,
                        &style_rule.rules,
                        origin,
                        stylesheet_id,
                        layer_context,
                        layer_order,
                        pseudo,
                        source_order,
                        out,
                        viewport_width,
                        viewport_height,
                        color_scheme_dark,
                        media_type,
                        media_environment,
                        media_cache,
                        scope_cache,
                        container_cache,
                        container_contexts,
                        element_keys,
                        selector_cache,
                        shadow_scope,
                        implicit_scope_root,
                        encapsulation_order,
                        active_scope,
                        active_layer,
                    );
                }
            }
            Rule::At(at_rule) => {
                if let Some(block) = &at_rule.block {
                    if at_rule.name.eq_ignore_ascii_case("layer") {
                        let Some(path) = layer_block_path(
                            at_rule,
                            stylesheet_id,
                            active_layer.map(Vec::as_slice).unwrap_or(&[]),
                        ) else {
                            *source_order += count_declarations(block);
                            continue;
                        };
                        collect_rule_candidates(
                            node,
                            block,
                            origin,
                            stylesheet_id,
                            layer_context,
                            layer_order,
                            pseudo,
                            source_order,
                            out,
                            viewport_width,
                            viewport_height,
                            color_scheme_dark,
                            media_type,
                            media_environment,
                            media_cache,
                            scope_cache,
                            container_cache,
                            container_contexts,
                            element_keys,
                            selector_cache,
                            shadow_scope,
                            implicit_scope_root,
                            encapsulation_order,
                            active_scope,
                            Some(&path),
                        );
                        continue;
                    }
                    if at_rule.name.eq_ignore_ascii_case("scope") {
                        let prelude = scope_cache
                            .entry(at_rule.prelude.clone())
                            .or_insert_with(|| super::parse_scope_prelude(&at_rule.prelude));
                        let Some(prelude) = prelude.as_ref() else {
                            *source_order += count_declarations(block);
                            continue;
                        };
                        let Some(scope) = applicable_scope(
                            node,
                            &prelude,
                            active_scope,
                            implicit_scope_root,
                            selector_cache,
                        ) else {
                            *source_order += count_declarations(block);
                            continue;
                        };
                        collect_rule_candidates(
                            node,
                            block,
                            origin,
                            stylesheet_id,
                            layer_context,
                            layer_order,
                            pseudo,
                            source_order,
                            out,
                            viewport_width,
                            viewport_height,
                            color_scheme_dark,
                            media_type,
                            media_environment,
                            media_cache,
                            scope_cache,
                            container_cache,
                            container_contexts,
                            element_keys,
                            selector_cache,
                            shadow_scope,
                            implicit_scope_root,
                            encapsulation_order,
                            Some(&scope),
                            active_layer,
                        );
                        continue;
                    }
                    // Evaluate @media queries before descending into the block.
                    let should_apply = if at_rule.name == "media" {
                        media_query_matches(
                            &at_rule.prelude,
                            viewport_width,
                            viewport_height,
                            media_environment,
                            media_cache,
                        )
                    } else if at_rule.name.eq_ignore_ascii_case("supports") {
                        super::supports_condition_matches(&at_rule.prelude)
                    } else if at_rule.name.eq_ignore_ascii_case("container") {
                        container_query_matches(
                            node,
                            shadow_scope,
                            &at_rule.prelude,
                            container_cache,
                            container_contexts,
                        )
                    } else if at_rule.name.eq_ignore_ascii_case("keyframes")
                        || at_rule.name.eq_ignore_ascii_case("-webkit-keyframes")
                    {
                        // @keyframes rules are handled separately; skip them in cascade.
                        false
                    } else {
                        // Non-conditional grouping rules (e.g. @layer) pass through.
                        true
                    };
                    if should_apply {
                        collect_rule_candidates(
                            node,
                            block,
                            origin,
                            stylesheet_id,
                            layer_context,
                            layer_order,
                            pseudo,
                            source_order,
                            out,
                            viewport_width,
                            viewport_height,
                            color_scheme_dark,
                            media_type,
                            media_environment,
                            media_cache,
                            scope_cache,
                            container_cache,
                            container_contexts,
                            element_keys,
                            selector_cache,
                            shadow_scope,
                            implicit_scope_root,
                            encapsulation_order,
                            active_scope,
                            active_layer,
                        );
                    } else {
                        // Count the rules inside for correct source_order numbering.
                        *source_order += count_declarations(block);
                    }
                } else {
                    *source_order += at_rule.declarations.len();
                }
            }
            // @font-face rules are handled by the font loading layer, not style resolution.
            Rule::FontFace(_) => {}
        }
    }
}

/// Matches an outer-tree `::part()` selector against a shadow-tree element.
///
/// Each entry in the exposure chain pairs the host visible in a tree scope
/// with the names exported into that scope. A nested host only forwards names
/// listed by its own `exportparts` attribute, so ordinary document selectors
/// can never pierce an unexported shadow boundary.
fn matches_part_selector(
    node: &NodeHandle,
    selector: &Selector,
    stylesheet_scope: Option<&NodeHandle>,
    pseudo: Option<PseudoElement>,
    cache: &mut SelectorMatchCache,
) -> bool {
    let Some(part) = part_selector_components(selector) else {
        return false;
    };
    if part.pseudo != pseudo
        || !super::matcher::matches_shadow_part_target(node, &part.target, cache)
    {
        return false;
    }
    let Some(mut root) = node.containing_shadow_root() else {
        return false;
    };
    let mut exposed_names = HashSet::new();
    if let Some(value) = node.get_attribute("part") {
        exposed_names.extend(value.split_ascii_whitespace().map(str::to_string));
    }
    if exposed_names.is_empty() {
        return false;
    }

    loop {
        let Some(host) = root.shadow_host() else {
            return false;
        };
        let visible_in_stylesheet_scope = match stylesheet_scope {
            Some(scope) => &root == scope || host.containing_shadow_root().as_ref() == Some(scope),
            None => host.containing_shadow_root().is_none(),
        };
        if visible_in_stylesheet_scope && exposed_names.contains(&part.name) {
            let host_matches = if let Some(scope) = stylesheet_scope {
                matches_shadow_scoped_selector(&host, &part.host, scope, None, cache)
            } else {
                matches_selector_with_pseudo_cached(&host, &part.host, None, cache)
            };
            if host_matches {
                return true;
            }
        }

        let Some(outer_root) = host.containing_shadow_root() else {
            return false;
        };
        exposed_names = forwarded_part_names(&host, &exposed_names);
        if exposed_names.is_empty() {
            return false;
        }
        root = outer_root;
    }
}

struct PartSelector {
    name: String,
    host: Selector,
    target: Selector,
    pseudo: Option<PseudoElement>,
}

fn part_selector_components(selector: &Selector) -> Option<PartSelector> {
    let subject = selector.parts.last()?;
    let (index, argument) = subject
        .simples
        .iter()
        .enumerate()
        .find_map(|(index, simple)| {
            let SimpleSelector::PseudoElement(name) = simple else {
                return None;
            };
            let (function, argument) = functional_selector(name)?;
            function
                .eq_ignore_ascii_case("part")
                .then_some((index, argument))
        })?;
    let mut host = selector.clone();
    let last = host.parts.last_mut()?;
    let mut tail = last.simples.split_off(index + 1);
    last.simples.pop();
    if last.simples.is_empty() {
        last.simples.push(SimpleSelector::Universal);
    }
    let pseudo_start = tail.iter().position(|simple| match simple {
        SimpleSelector::PseudoElement(_) => true,
        SimpleSelector::PseudoClass(name) => matches!(name.as_str(), "before" | "after"),
        _ => false,
    });
    let pseudo = if let Some(start) = pseudo_start {
        let pseudo_selector = Selector {
            parts: vec![SelectorPart {
                combinator: None,
                simples: tail.split_off(start),
            }],
        };
        Some(super::matcher::selector_pseudo_element(&pseudo_selector)?)
    } else {
        None
    };
    if tail.is_empty() {
        tail.push(SimpleSelector::Universal);
    }
    Some(PartSelector {
        name: argument.to_string(),
        host,
        target: Selector {
            parts: vec![SelectorPart {
                combinator: None,
                simples: tail,
            }],
        },
        pseudo,
    })
}

fn forwarded_part_names(host: &NodeHandle, inner_names: &HashSet<String>) -> HashSet<String> {
    let Some(mapping) = host.get_attribute("exportparts") else {
        return HashSet::new();
    };
    mapping
        .split(',')
        .filter_map(parse_exportparts_entry)
        .filter_map(|(inner, outer)| inner_names.contains(&inner).then_some(outer))
        .collect()
}

fn parse_exportparts_entry(entry: &str) -> Option<(String, String)> {
    let tokens: Vec<CssToken> = super::tokenize(entry)
        .ok()?
        .into_iter()
        .filter(|token| *token != CssToken::Whitespace)
        .collect();
    match tokens.as_slice() {
        [CssToken::Ident(name)] => Some((name.clone(), name.clone())),
        [
            CssToken::Ident(inner),
            CssToken::Colon,
            CssToken::Ident(outer),
        ] => Some((inner.clone(), outer.clone())),
        _ => None,
    }
}

fn applicable_scope(
    node: &NodeHandle,
    prelude: &super::ScopePrelude,
    outer: Option<&ActiveScope>,
    implicit_scope_root: Option<&NodeHandle>,
    selector_cache: &mut SelectorMatchCache,
) -> Option<ActiveScope> {
    let mut ancestors = Vec::new();
    let mut current = Some(node.clone());
    while let Some(candidate) = current {
        if candidate.node_type() == NodeType::Element {
            ancestors.push(candidate.clone());
        }
        current = candidate.parent_node();
    }
    if ancestors.is_empty() {
        return None;
    }

    let ambient_roots: Vec<NodeHandle> = if let Some(outer) = outer {
        outer.roots.iter().map(|root| root.node.clone()).collect()
    } else {
        vec![ancestors.last()?.clone()]
    };

    let mut roots = Vec::new();
    for ambient_root in ambient_roots {
        let Some(ambient_proximity) = ancestors
            .iter()
            .position(|candidate| candidate == &ambient_root)
        else {
            continue;
        };
        let candidates: Vec<(usize, NodeHandle)> = if let Some(start) = &prelude.start {
            ancestors
                .iter()
                .take(ambient_proximity + 1)
                .enumerate()
                .filter(|(_, candidate)| {
                    start.iter().any(|selector| {
                        matches_selector_boundary_cached(
                            candidate,
                            selector,
                            None,
                            selector_cache,
                            Some(&ambient_root),
                        )
                    })
                })
                .map(|(proximity, root)| (proximity, root.clone()))
                .collect()
        } else if let Some(implicit_root) = implicit_scope_root {
            ancestors
                .iter()
                .take(ambient_proximity + 1)
                .position(|candidate| candidate == implicit_root)
                .map(|proximity| vec![(proximity, implicit_root.clone())])
                .or_else(|| {
                    // A style directly in a shadow tree is implicitly scoped
                    // to its host, which is not represented as a parent of
                    // nodes in the separate shadow-tree fragment.
                    let in_shadow_tree = node
                        .containing_shadow_root()
                        .and_then(|root| root.shadow_host())
                        .is_some_and(|host| host == *implicit_root);
                    in_shadow_tree.then_some(vec![(ancestors.len(), implicit_root.clone())])
                })
                .unwrap_or_default()
        } else {
            vec![(ambient_proximity, ambient_root)]
        };

        for (proximity, root) in candidates {
            let excluded_by_limit = prelude.end.as_ref().is_some_and(|limits| {
                ancestors.iter().take(proximity + 1).any(|candidate| {
                    limits.iter().any(|selector| {
                        matches_selector_boundary_cached(
                            candidate,
                            selector,
                            None,
                            selector_cache,
                            Some(&root),
                        )
                    })
                })
            });
            if !excluded_by_limit
                && !roots
                    .iter()
                    .any(|existing: &ScopeRoot| existing.node == root)
            {
                roots.push(ScopeRoot {
                    node: root,
                    proximity,
                });
            }
        }
    }
    (!roots.is_empty()).then_some(ActiveScope { roots })
}

fn selector_references_scope(selector: &Selector) -> bool {
    selector.parts.iter().any(|part| {
        part.simples.iter().any(|simple| match simple {
            SimpleSelector::PseudoClass(name) => name.eq_ignore_ascii_case("scope"),
            SimpleSelector::Is(selectors)
            | SimpleSelector::Where(selectors)
            | SimpleSelector::Not(selectors)
            | SimpleSelector::NthChild { selectors, .. } => {
                selectors.iter().any(selector_references_scope)
            }
            SimpleSelector::Has(relative) => relative
                .iter()
                .any(|relative| selector_references_scope(&relative.selector)),
            _ => false,
        })
    })
}

fn retain_best_scoped_match(
    current: &mut Option<(Specificity, usize)>,
    specificity: Specificity,
    proximity: usize,
) {
    let replace = current.is_none_or(|(current_specificity, current_proximity)| {
        specificity > current_specificity
            || specificity == current_specificity && proximity < current_proximity
    });
    if replace {
        *current = Some((specificity, proximity));
    }
}

/// Returns `true` when at least one query in a comma-separated media query list
/// matches the given viewport.  Falls back to `false` when the list cannot be
/// parsed (conservative, forward-compatible behaviour).
///
/// The `cache` parameter is a mutable reference to a parse-result cache keyed
/// by the normalized (trimmed) prelude string.  On a cache miss the prelude is
/// parsed and the result is stored so that subsequent calls with the same string
/// skip parsing.
fn media_query_matches(
    prelude: &str,
    viewport_width: f32,
    viewport_height: f32,
    media_environment: &MediaEnvironment,
    cache: &mut HashMap<String, Vec<MediaQuery>>,
) -> bool {
    let prelude = prelude.trim();
    if prelude.is_empty() {
        return true;
    }
    let queries = cache
        .entry(prelude.to_owned())
        .or_insert_with(|| parse_media_query_list(prelude).unwrap_or_default());
    queries.iter().any(|query| {
        evaluate_media_query_with_environment(
            query,
            viewport_width,
            viewport_height,
            media_environment,
        )
    })
}

/// Parses a structurally valid length expression without substituting DOM values.
pub(crate) fn parse_container_length_expression(text: &str) -> Option<Value> {
    let value = parse_registered_initial_value(text)?;
    (typed_math::infer(&value, None) == Some(typed_math::NumericType::LENGTH)).then_some(value)
}

/// Computes a query length from explicit units and a short-lived container scope input.
pub(crate) fn compute_container_length(
    value: &Value,
    units: super::ContainerUnitContext,
    element: Option<(&NodeHandle, Option<&NodeHandle>)>,
) -> Option<f32> {
    let value = if sibling_functions::contains(value) {
        let (node, scope) = element?;
        let value = sibling_functions::annotate(value, &sibling_functions::scope_ancestry(scope));
        sibling_functions::resolve(&value, node)
    } else {
        value.clone()
    };
    let ctx = ResolutionContext {
        parent_font_size: units.font_size,
        root_font_size: units.root_font_size,
        line_height: units.line_height,
        root_line_height: units.root_line_height,
        font_metrics: units.metrics,
        ..ResolutionContext::default()
    };
    match compute_value(&value, "width", ctx) {
        ComputedValue::Px(value) => Some(value),
        _ => None,
    }
}

fn container_query_matches(
    node: &NodeHandle,
    declaration_scope: Option<&NodeHandle>,
    prelude: &str,
    cache: &mut HashMap<String, Option<super::ContainerQuery>>,
    contexts: &HashMap<usize, ContainerContext>,
) -> bool {
    let prelude = prelude.trim();
    let query = cache
        .entry(prelude.to_string())
        .or_insert_with(|| super::parse_container_query(prelude));
    let Some(query) = query.as_ref() else {
        return false;
    };

    let mut ancestor = node.parent_node();
    while let Some(candidate) = ancestor {
        if candidate.node_type() == NodeType::Element
            && let Some(context) = contexts.get(&candidate.identity())
        {
            let supports_axis = !query.requires_size_containment()
                || context.container_type.eq_ignore_ascii_case("size")
                || (!query.requires_block_size()
                    && context.container_type.eq_ignore_ascii_case("inline-size"));
            let name_matches = query
                .name
                .as_ref()
                .is_none_or(|name| context.names.iter().any(|candidate| candidate == name));
            if supports_axis && name_matches {
                return query.matches_for_element(
                    context.width,
                    context.height,
                    context.units,
                    &candidate,
                    declaration_scope,
                    &context.style,
                );
            }
        }
        ancestor = candidate.parent_node();
    }
    false
}

/// Counts the total number of declarations inside a rule list (used for
/// source_order bookkeeping when a block is skipped due to a non-matching
/// media query).
fn count_declarations(rules: &[Rule]) -> usize {
    rules
        .iter()
        .map(|r| match r {
            Rule::Style(s) => s.declarations.len() + count_declarations(&s.rules),
            Rule::At(a) => {
                a.declarations.len() + a.block.as_deref().map(count_declarations).unwrap_or(0)
            }
            Rule::FontFace(_) => 0,
        })
        .sum()
}

fn is_length_property(name: &str) -> bool {
    matches!(
        name,
        "width"
            | "height"
            | "inline-size"
            | "block-size"
            | "min-inline-size"
            | "min-block-size"
            | "max-inline-size"
            | "max-block-size"
            | "min-width"
            | "min-height"
            | "max-width"
            | "max-height"
            | "margin-top"
            | "margin-inline-start"
            | "margin-inline-end"
            | "margin-block-start"
            | "margin-block-end"
            | "margin-right"
            | "margin-bottom"
            | "margin-left"
            | "padding-top"
            | "padding-inline-start"
            | "padding-inline-end"
            | "padding-block-start"
            | "padding-block-end"
            | "padding-right"
            | "padding-bottom"
            | "padding-left"
            | "border-top-width"
            | "border-right-width"
            | "border-bottom-width"
            | "border-left-width"
            | "border-inline-start-width"
            | "border-inline-end-width"
            | "border-block-start-width"
            | "border-block-end-width"
            | "top"
            | "right"
            | "bottom"
            | "left"
            | "inset-inline-start"
            | "inset-inline-end"
            | "inset-block-start"
            | "inset-block-end"
            | "border-spacing"
            | "flex-basis"
            | "column-width"
            | "column-gap"
            | "column-rule-width"
            | "outline-width"
            | "outline-offset"
            | "shape-margin"
    )
}

/// Collects `@keyframes` definitions and resolves name collisions by origin,
/// cascade layer, and source order within one tree scope.
#[allow(clippy::too_many_arguments)]
fn collect_keyframes(
    rules: &[Rule],
    origin: Origin,
    stylesheet_id: usize,
    scope_root: Option<usize>,
    scope_ancestry: &[Option<usize>],
    layer_order: &CascadeLayerOrder,
    active_layer: Option<&LayerPath>,
    source_order: &mut usize,
    keyframes: &mut HashMap<Option<usize>, HashMap<String, KeyframesDefinition>>,
    viewport_width: f32,
    viewport_height: f32,
    color_scheme_dark: bool,
    media_type: MediaType,
    media_environment: &MediaEnvironment,
) {
    for rule in rules {
        match rule {
            Rule::At(at_rule)
                if at_rule.name.eq_ignore_ascii_case("keyframes")
                    || at_rule.name.eq_ignore_ascii_case("-webkit-keyframes") =>
            {
                let Some(animation_name) = parse_keyframes_name(&at_rule.prelude) else {
                    *source_order += 1;
                    continue;
                };
                let raw_block = at_rule
                    .declarations
                    .iter()
                    .find(|declaration| declaration.name == "__keyframes_block")
                    .and_then(|declaration| match &declaration.value {
                        Value::Keyword(text) => Some(text.clone()),
                        _ => None,
                    });
                if let Some(block_text) = raw_block {
                    let candidate = KeyframesDefinition {
                        origin,
                        layer_order: layer_order.rank(active_layer),
                        source_order: *source_order,
                        steps: parse_scoped_keyframe_steps(&block_text, scope_ancestry),
                    };
                    let definitions = keyframes.entry(scope_root).or_default();
                    match definitions.entry(animation_name) {
                        std::collections::hash_map::Entry::Occupied(mut entry)
                            if compare_keyframes_priority(&candidate, entry.get()).is_gt() =>
                        {
                            entry.insert(candidate);
                        }
                        std::collections::hash_map::Entry::Vacant(entry) => {
                            entry.insert(candidate);
                        }
                        _ => {}
                    }
                }
                *source_order += 1;
            }
            Rule::At(at_rule) if at_rule.block.is_some() => {
                if !layer_group_rule_is_active(
                    at_rule,
                    viewport_width,
                    viewport_height,
                    media_environment,
                ) {
                    continue;
                }
                let block = at_rule.block.as_deref().unwrap();
                if at_rule.name.eq_ignore_ascii_case("layer") {
                    let Some(path) = layer_block_path(
                        at_rule,
                        stylesheet_id,
                        active_layer.map(Vec::as_slice).unwrap_or(&[]),
                    ) else {
                        continue;
                    };
                    collect_keyframes(
                        block,
                        origin,
                        stylesheet_id,
                        scope_root,
                        scope_ancestry,
                        layer_order,
                        Some(&path),
                        source_order,
                        keyframes,
                        viewport_width,
                        viewport_height,
                        color_scheme_dark,
                        media_type,
                        media_environment,
                    );
                } else {
                    collect_keyframes(
                        block,
                        origin,
                        stylesheet_id,
                        scope_root,
                        scope_ancestry,
                        layer_order,
                        active_layer,
                        source_order,
                        keyframes,
                        viewport_width,
                        viewport_height,
                        color_scheme_dark,
                        media_type,
                        media_environment,
                    );
                }
            }
            _ => {}
        }
    }
}

fn collect_registered_custom_properties(
    rules: &[Rule],
    registrations: &mut BTreeMap<String, RegisteredCustomProperty>,
    viewport_width: f32,
    viewport_height: f32,
    color_scheme_dark: bool,
    media_type: MediaType,
    media_environment: &MediaEnvironment,
) {
    for rule in rules {
        match rule {
            Rule::At(at_rule) if at_rule.name.eq_ignore_ascii_case("property") => {
                if let Some(registration) = registered_custom_property_from_rule(at_rule) {
                    registrations.insert(registration.name.clone(), registration);
                }
            }
            Rule::At(at_rule) if at_rule.block.is_some() => {
                if !layer_group_rule_is_active(
                    at_rule,
                    viewport_width,
                    viewport_height,
                    media_environment,
                ) {
                    continue;
                }
                collect_registered_custom_properties(
                    at_rule.block.as_deref().unwrap_or_default(),
                    registrations,
                    viewport_width,
                    viewport_height,
                    color_scheme_dark,
                    media_type,
                    media_environment,
                );
            }
            _ => {}
        }
    }
}

fn compare_keyframes_priority(
    left: &KeyframesDefinition,
    right: &KeyframesDefinition,
) -> std::cmp::Ordering {
    left.origin
        .cmp(&right.origin)
        .then(left.layer_order.cmp(&right.layer_order))
        .then(left.source_order.cmp(&right.source_order))
}

/// Collects `@font-face` definitions and resolves collisions between the
/// family/weight/style variants currently supported by the font registry.
#[allow(clippy::too_many_arguments)]
fn collect_font_faces(
    rules: &[Rule],
    origin: Origin,
    stylesheet_id: usize,
    scope_root: Option<usize>,
    layer_order: &CascadeLayerOrder,
    active_layer: Option<&LayerPath>,
    source_order: &mut usize,
    font_faces: &mut HashMap<Option<usize>, HashMap<FontFaceKey, FontFaceDefinition>>,
    active_font_faces: &mut Vec<(Option<usize>, super::FontFaceRule)>,
    viewport_width: f32,
    viewport_height: f32,
    color_scheme_dark: bool,
    media_type: MediaType,
    media_environment: &MediaEnvironment,
) {
    for rule in rules {
        match rule {
            Rule::FontFace(rule) => {
                active_font_faces.push((scope_root, rule.clone()));
                let candidate = FontFaceDefinition {
                    origin,
                    layer_order: layer_order.rank(active_layer),
                    source_order: *source_order,
                    rule: rule.clone(),
                };
                let key = FontFaceKey {
                    family: rule.font_family.to_ascii_lowercase(),
                    descriptors: crate::font::WebFontDescriptors {
                        weight: crate::font::FontWeightRange::parse(
                            rule.font_weight.as_deref().unwrap_or("normal"),
                        ),
                        style: crate::font::FontStyleRange::parse(
                            rule.font_style.as_deref().unwrap_or("normal"),
                        ),
                        stretch: crate::font::FontStretchRange::parse(
                            rule.font_stretch.as_deref().unwrap_or("normal"),
                        ),
                        unicode_range: rule
                            .unicode_range
                            .as_deref()
                            .and_then(crate::font::UnicodeRangeSet::parse)
                            .unwrap_or_default(),
                    },
                };
                let definitions = font_faces.entry(scope_root).or_default();
                match definitions.entry(key) {
                    std::collections::hash_map::Entry::Occupied(mut entry)
                        if compare_font_face_priority(&candidate, entry.get()).is_gt() =>
                    {
                        entry.insert(candidate);
                    }
                    std::collections::hash_map::Entry::Vacant(entry) => {
                        entry.insert(candidate);
                    }
                    _ => {}
                }
                *source_order += 1;
            }
            Rule::At(at_rule) if at_rule.block.is_some() => {
                if !layer_group_rule_is_active(
                    at_rule,
                    viewport_width,
                    viewport_height,
                    media_environment,
                ) {
                    continue;
                }
                let block = at_rule.block.as_deref().unwrap();
                if at_rule.name.eq_ignore_ascii_case("layer") {
                    let Some(path) = layer_block_path(
                        at_rule,
                        stylesheet_id,
                        active_layer.map(Vec::as_slice).unwrap_or(&[]),
                    ) else {
                        continue;
                    };
                    collect_font_faces(
                        block,
                        origin,
                        stylesheet_id,
                        scope_root,
                        layer_order,
                        Some(&path),
                        source_order,
                        font_faces,
                        active_font_faces,
                        viewport_width,
                        viewport_height,
                        color_scheme_dark,
                        media_type,
                        media_environment,
                    );
                } else {
                    collect_font_faces(
                        block,
                        origin,
                        stylesheet_id,
                        scope_root,
                        layer_order,
                        active_layer,
                        source_order,
                        font_faces,
                        active_font_faces,
                        viewport_width,
                        viewport_height,
                        color_scheme_dark,
                        media_type,
                        media_environment,
                    );
                }
            }
            _ => {}
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn collect_counter_styles(
    rules: &[Rule],
    origin: Origin,
    stylesheet_id: usize,
    scope_root: Option<usize>,
    layer_order: &CascadeLayerOrder,
    active_layer: Option<&LayerPath>,
    source_order: &mut usize,
    counter_styles: &mut HashMap<Option<usize>, HashMap<String, CounterStyleDefinition>>,
    viewport_width: f32,
    viewport_height: f32,
    color_scheme_dark: bool,
    media_type: MediaType,
    media_environment: &MediaEnvironment,
) {
    for rule in rules {
        match rule {
            Rule::At(at_rule) if at_rule.name.eq_ignore_ascii_case("counter-style") => {
                if let Some(definition) = parse_counter_style_definition(
                    at_rule,
                    origin,
                    layer_order.rank(active_layer),
                    *source_order,
                ) {
                    let styles = counter_styles.entry(scope_root).or_default();
                    match styles.entry(at_rule.prelude.trim().to_ascii_lowercase()) {
                        std::collections::hash_map::Entry::Occupied(mut entry)
                            if compare_counter_style_priority(&definition, entry.get()).is_gt() =>
                        {
                            entry.insert(definition);
                        }
                        std::collections::hash_map::Entry::Vacant(entry) => {
                            entry.insert(definition);
                        }
                        _ => {}
                    }
                }
                *source_order += 1;
            }
            Rule::At(at_rule) if at_rule.block.is_some() => {
                if !layer_group_rule_is_active(
                    at_rule,
                    viewport_width,
                    viewport_height,
                    media_environment,
                ) {
                    continue;
                }
                let block = at_rule.block.as_deref().unwrap_or_default();
                let next_layer = if at_rule.name.eq_ignore_ascii_case("layer") {
                    layer_block_path(
                        at_rule,
                        stylesheet_id,
                        active_layer.map(Vec::as_slice).unwrap_or(&[]),
                    )
                } else {
                    None
                };
                collect_counter_styles(
                    block,
                    origin,
                    stylesheet_id,
                    scope_root,
                    layer_order,
                    next_layer.as_ref().or(active_layer),
                    source_order,
                    counter_styles,
                    viewport_width,
                    viewport_height,
                    color_scheme_dark,
                    media_type,
                    media_environment,
                );
            }
            _ => {}
        }
    }
}

fn parse_counter_style_definition(
    rule: &super::AtRule,
    origin: Origin,
    layer_order: Vec<usize>,
    source_order: usize,
) -> Option<CounterStyleDefinition> {
    let name = rule.prelude.trim();
    if name.is_empty() || name.eq_ignore_ascii_case("default") || name.contains(char::is_whitespace)
    {
        return None;
    }
    let descriptor = |name: &str| {
        rule.declarations
            .iter()
            .rev()
            .find(|declaration| declaration.name.eq_ignore_ascii_case(name))
            .map(|declaration| &declaration.value)
    };
    let system = match descriptor("system")? {
        Value::Keyword(system) if system.eq_ignore_ascii_case("cyclic") => {
            CounterStyleSystem::Cyclic
        }
        Value::List(values) if values.len() == 2 => match (&values[0], &values[1]) {
            (Value::Keyword(system), Value::Keyword(base))
                if system.eq_ignore_ascii_case("extends") =>
            {
                CounterStyleSystem::Extends(base.to_ascii_lowercase())
            }
            _ => return None,
        },
        _ => return None,
    };
    let symbols = descriptor("symbols")
        .map(counter_style_symbols)
        .unwrap_or_default();
    if symbols.is_empty() && matches!(system, CounterStyleSystem::Cyclic) {
        return None;
    }
    let prefix = descriptor("prefix").map(render_value).unwrap_or_default();
    let suffix = descriptor("suffix").map(render_value).unwrap_or_default();
    Some(CounterStyleDefinition {
        origin,
        layer_order,
        source_order,
        system,
        symbols,
        prefix,
        suffix,
    })
}

fn counter_style_symbols(value: &Value) -> Vec<String> {
    match value {
        Value::CommaList(values) | Value::List(values) => values.iter().map(render_value).collect(),
        value => vec![render_value(value)],
    }
}

fn compare_counter_style_priority(
    left: &CounterStyleDefinition,
    right: &CounterStyleDefinition,
) -> std::cmp::Ordering {
    left.origin
        .cmp(&right.origin)
        .then(left.layer_order.cmp(&right.layer_order))
        .then(left.source_order.cmp(&right.source_order))
}

fn compare_font_face_priority(
    left: &FontFaceDefinition,
    right: &FontFaceDefinition,
) -> std::cmp::Ordering {
    left.origin
        .cmp(&right.origin)
        .then(left.layer_order.cmp(&right.layer_order))
        .then(left.source_order.cmp(&right.source_order))
}

fn parse_keyframes_name(prelude: &str) -> Option<String> {
    let mut tokens = super::tokenizer::tokenize(prelude)
        .ok()?
        .into_iter()
        .filter(|token| !matches!(token, CssToken::Whitespace));
    let token = tokens.next()?;
    if tokens.next().is_some() {
        return None;
    }
    match token {
        CssToken::Ident(name) => {
            let lower = name.to_ascii_lowercase();
            (!is_css_wide_keyword(&lower) && lower != "none" && lower != "default").then_some(name)
        }
        CssToken::String(name) => (!name.is_empty()).then_some(name),
        _ => None,
    }
}

fn parse_scoped_keyframe_steps(text: &str, ancestry: &[Option<usize>]) -> Vec<KeyframeStep> {
    let mut steps = parse_keyframe_steps(text);
    for step in &mut steps {
        for declaration in &mut step.declarations {
            if sibling_functions::contains(&declaration.value) {
                declaration.value = sibling_functions::annotate(&declaration.value, ancestry);
            }
        }
    }
    steps
}

fn parse_keyframe_steps(block_text: &str) -> Vec<KeyframeStep> {
    let mut steps = Vec::new();
    let mut position = 0;
    let chars: Vec<char> = block_text.chars().collect();

    while position < chars.len() {
        while position < chars.len() && chars[position].is_ascii_whitespace() {
            position += 1;
        }
        if position >= chars.len() {
            break;
        }
        let selector_start = position;
        while position < chars.len() && chars[position] != '{' {
            position += 1;
        }
        if position >= chars.len() {
            break;
        }
        let selector: String = chars[selector_start..position].iter().collect();
        position += 1;

        let declaration_start = position;
        let mut depth = 1;
        while position < chars.len() && depth > 0 {
            match chars[position] {
                '{' => depth += 1,
                '}' => depth -= 1,
                _ => {}
            }
            if depth > 0 {
                position += 1;
            }
        }
        let declaration_text: String = chars[declaration_start..position].iter().collect();
        if position < chars.len() {
            position += 1;
        }

        let offsets: Vec<f32> = selector
            .split(',')
            .filter_map(|part| match part.trim().to_ascii_lowercase().as_str() {
                "from" => Some(0.0),
                "to" => Some(1.0),
                percentage => percentage
                    .strip_suffix('%')
                    .and_then(|number| number.trim().parse::<f32>().ok())
                    .map(|number| (number / 100.0).clamp(0.0, 1.0)),
            })
            .collect();
        if offsets.is_empty() {
            continue;
        }

        let fake_rule = format!("x {{ {declaration_text} }}");
        let Ok(stylesheet) = super::parse_stylesheet(&fake_rule) else {
            continue;
        };
        let Some(declarations) = stylesheet.rules.iter().find_map(|rule| match rule {
            Rule::Style(style_rule) => Some(style_rule.declarations.clone()),
            _ => None,
        }) else {
            continue;
        };
        for offset in offsets {
            steps.push(KeyframeStep {
                offset,
                declarations: declarations.clone(),
            });
        }
    }

    steps.sort_by(|left, right| left.offset.total_cmp(&right.offset));
    steps
}

fn cascade_rank(candidate: &Candidate) -> (u8, u8) {
    let importance = if candidate.important { 1 } else { 0 };
    let origin = match (candidate.important, candidate.origin) {
        (true, Origin::UserAgent) => 5,
        (true, Origin::User) => 4,
        (true, Origin::Author) => 3,
        (false, Origin::Author) => 2,
        (false, Origin::User) => 1,
        (false, Origin::UserAgent) => 0,
    };
    (importance, origin)
}

fn compare_candidate_priority(left: &Candidate, right: &Candidate) -> std::cmp::Ordering {
    cascade_rank(left)
        .cmp(&cascade_rank(right))
        .then(encapsulation_rank(left).cmp(&encapsulation_rank(right)))
        .then(left.inline.cmp(&right.inline))
        .then_with(|| compare_layer_priority(left, right))
        .then(right.prefixed_alias.cmp(&left.prefixed_alias))
        .then(left.specificity.cmp(&right.specificity))
        .then_with(|| compare_scope_proximity(left, right))
        .then(left.source_order.cmp(&right.source_order))
}

fn expand_pending_shorthand_candidates(
    candidates: Vec<Candidate>,
    custom_properties: &BTreeMap<String, Value>,
) -> Vec<Candidate> {
    let mut expanded_candidates = Vec::with_capacity(candidates.len());
    for candidate in candidates {
        if !super::shorthand::is_deferred_var_shorthand(&candidate.name)
            || !value_contains_var_function(&candidate.value)
        {
            expanded_candidates.push(candidate);
            continue;
        }

        let resolved = resolve_value_with_custom_properties(&candidate.value, custom_properties);
        let mut declarations = resolved.map_or_else(Vec::new, |resolved| {
            if candidate.name == "transition" {
                super::expand_transition_shorthand(
                    Value::Keyword(render_value(&resolved)),
                    candidate.important,
                )
            } else {
                super::shorthand::expand_shorthand(&candidate.name, resolved, candidate.important)
            }
        });
        let invalid = declarations.is_empty()
            || declarations.len() == 1
                && declarations[0].name.eq_ignore_ascii_case(&candidate.name);
        if invalid {
            // A var()-dependent shorthand has already won the cascade. If its
            // substituted value is invalid at computed-value time, every
            // longhand receives unset; the engine must not fall back to an
            // earlier declaration.
            declarations = super::shorthand::expand_shorthand(
                &candidate.name,
                Value::Keyword("unset".to_string()),
                candidate.important,
            );
        }

        for declaration in declarations {
            let name = canonical_property_name(&declaration.name).to_string();
            expanded_candidates.push(Candidate {
                prefixed_alias: is_prefixed_property_alias(&declaration.name),
                name,
                value: declaration.value,
                important: declaration.important,
                ..candidate.clone()
            });
        }
    }
    expanded_candidates
}

fn compare_layer_priority(left: &Candidate, right: &Candidate) -> std::cmp::Ordering {
    if left.inline || right.inline {
        return std::cmp::Ordering::Equal;
    }
    let order = left.layer_order.cmp(&right.layer_order);
    if left.important {
        order.reverse()
    } else {
        order
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct RevertedLayer {
    important: bool,
    inline: bool,
    context: LayerContextKey,
    path: Option<LayerPath>,
}

impl From<&Candidate> for RevertedLayer {
    fn from(candidate: &Candidate) -> Self {
        Self {
            important: candidate.important,
            inline: candidate.inline,
            context: candidate.layer_context,
            path: candidate.layer_path.clone(),
        }
    }
}

fn remove_reverted_candidates(
    candidates: &mut Vec<Candidate>,
    custom_properties: Option<&BTreeMap<String, Value>>,
    flow: Option<super::logical::LogicalFlow>,
) {
    #[derive(Default)]
    struct PropertyRevertState {
        reverted_layers: HashSet<RevertedLayer>,
        reverted_origins: HashSet<Origin>,
        reverted_rules: HashSet<(Origin, usize)>,
        winner_found: bool,
    }

    let mut states: HashMap<String, PropertyRevertState> = HashMap::new();
    for candidate in candidates.iter().rev() {
        let state = states
            .entry(revert_property_group(&candidate.name, flow))
            .or_default();
        if state.winner_found {
            continue;
        }
        let origin = candidate.origin;
        if state.reverted_origins.contains(&origin) {
            continue;
        }
        let rule = (origin, candidate.rule_order);
        if state.reverted_rules.contains(&rule) {
            continue;
        }
        let layer = RevertedLayer::from(candidate);
        if state.reverted_layers.contains(&layer) {
            continue;
        }
        if is_revert_rule_value(&candidate.value) {
            state.reverted_rules.insert(rule);
        } else if is_revert_layer_value(&candidate.value) {
            state.reverted_layers.insert(layer);
        } else if is_revert_value(&candidate.value) {
            state.reverted_origins.insert(origin);
        } else if candidate_can_win_before_revert(candidate, custom_properties) {
            state.winner_found = true;
        }
    }

    candidates.retain(|candidate| {
        !states
            .get(&revert_property_group(&candidate.name, flow))
            .is_some_and(|state| {
                state
                    .reverted_layers
                    .contains(&RevertedLayer::from(candidate))
                    || state.reverted_origins.contains(&candidate.origin)
                    || state
                        .reverted_rules
                        .contains(&(candidate.origin, candidate.rule_order))
            })
    });
}

fn revert_property_group(name: &str, flow: Option<super::logical::LogicalFlow>) -> String {
    flow.and_then(|flow| flow.physical_name(name))
        .unwrap_or_else(|| name.to_string())
}

fn candidate_can_win_before_revert(
    candidate: &Candidate,
    custom_properties: Option<&BTreeMap<String, Value>>,
) -> bool {
    if candidate.name.starts_with("--") {
        return true;
    }
    let resolved_value = custom_properties
        .and_then(|properties| resolve_value_with_custom_properties(&candidate.value, properties));
    let value = match custom_properties {
        Some(_) => {
            let Some(value) = resolved_value.as_ref() else {
                return false;
            };
            value
        }
        None => &candidate.value,
    };
    match validate_declaration(&candidate.name, value) {
        DeclarationValidation::Invalid => false,
        DeclarationValidation::Valid(_) => true,
        DeclarationValidation::Unvalidated => {
            let computed = compute_value(value, &candidate.name, ResolutionContext::default());
            !should_skip_computed_property(&candidate.name, &computed)
        }
    }
}

fn is_revert_layer_value(value: &Value) -> bool {
    matches!(value, Value::Keyword(keyword) if keyword.eq_ignore_ascii_case("revert-layer"))
}

fn is_revert_rule_value(value: &Value) -> bool {
    matches!(value, Value::Keyword(keyword) if keyword.eq_ignore_ascii_case("revert-rule"))
}

fn is_revert_value(value: &Value) -> bool {
    matches!(value, Value::Keyword(keyword) if keyword.eq_ignore_ascii_case("revert"))
}

fn compare_scope_proximity(left: &Candidate, right: &Candidate) -> std::cmp::Ordering {
    match (left.scope_proximity, right.scope_proximity) {
        (Some(left), Some(right)) => right.cmp(&left),
        (Some(_), None) => std::cmp::Ordering::Greater,
        (None, Some(_)) => std::cmp::Ordering::Less,
        (None, None) => std::cmp::Ordering::Equal,
    }
}

fn encapsulation_rank(candidate: &Candidate) -> usize {
    if candidate.important {
        candidate.encapsulation_order
    } else {
        usize::MAX.saturating_sub(candidate.encapsulation_order)
    }
}

fn tree_scope_order(stylesheets: &[StylesheetScope], node: &NodeHandle) -> usize {
    let Some(mut scope) = node.containing_shadow_root() else {
        return 0;
    };
    let mut unregistered_inner_scopes = 0usize;
    loop {
        if let Some(order) = stylesheets
            .iter()
            .find(|input| input.root.as_ref() == Some(&scope))
            .map(|input| input.encapsulation_order)
        {
            return order.saturating_add(unregistered_inner_scopes);
        }
        // A ShadowRoot without a <style> element has no StylesheetScope entry,
        // but inline declarations in it still occupy an inner tree context.
        // Anchor at the nearest registered ancestor and move inward from it.
        unregistered_inner_scopes += 1;
        let Some(outer_scope) = scope
            .shadow_host()
            .and_then(|host| host.containing_shadow_root())
        else {
            return unregistered_inner_scopes;
        };
        scope = outer_scope;
    }
}

fn log_unsupported_css_if_enabled(property: &str, value: &Value) {
    let Some(category) = css_audit_category(property) else {
        return;
    };

    let config = unsupported_css_config();
    if !config.logging_enabled && config.sqlite_path.is_none() {
        return;
    }

    let rendered_value = sanitize_unsupported_css_log_value(&render_value(value));
    if let Some(path) = config.sqlite_path.as_deref() {
        persist_css_audit_to_sqlite(path, category, property, &rendered_value);
        if let Some(top_n) = config.top_n {
            emit_css_audit_top_n_summary_if_updated(path, top_n, category);
        }
    }

    if config.logging_enabled {
        let key = unsupported_css_dedup_key(property, &rendered_value);
        let logged = UNSUPPORTED_CSS_LOGGED.get_or_init(|| Mutex::new(HashSet::new()));
        let mut logged = logged.lock().expect("unsupported css log lock poisoned");
        if logged.len() >= MAX_UNSUPPORTED_LOG_KEYS {
            logged.clear();
        }
        if logged.insert(key) {
            let value = truncate_log_value(&rendered_value, MAX_UNSUPPORTED_LOG_VALUE_LEN);
            eprintln!("[omoikane][{}] {property}={value}", category.log_label());
        }
    }
}

fn unsupported_css_config() -> &'static UnsupportedCssConfig {
    UNSUPPORTED_CSS_CONFIG.get_or_init(|| UnsupportedCssConfig {
        logging_enabled: env_flag_true("OMOIKANE_LOG_UNSUPPORTED_CSS"),
        sqlite_path: std::env::var("OMOIKANE_UNSUPPORTED_CSS_SQLITE")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty()),
        top_n: std::env::var("OMOIKANE_UNSUPPORTED_CSS_TOP_N")
            .ok()
            .and_then(|value| value.trim().parse::<usize>().ok())
            .filter(|value| *value > 0)
            .or_else(|| {
                if env_flag_true("OMOIKANE_LOG_UNSUPPORTED_CSS_TOP_N") {
                    Some(DEFAULT_UNSUPPORTED_CSS_TOP_N)
                } else {
                    None
                }
            }),
    })
}

fn env_flag_true(name: &str) -> bool {
    std::env::var(name)
        .map(|value| {
            let value = value.trim();
            value == "1" || value.eq_ignore_ascii_case("true") || value.eq_ignore_ascii_case("yes")
        })
        .unwrap_or(false)
}

fn ensure_unsupported_css_sqlite_schema(conn: &Connection) -> Result<(), rusqlite::Error> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS unsupported_css_log (
            property TEXT NOT NULL,
            value TEXT NOT NULL,
            category TEXT NOT NULL DEFAULT 'unsupported',
            first_seen_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            last_seen_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            occurrences INTEGER NOT NULL DEFAULT 0,
            PRIMARY KEY (property, value)
        );
        CREATE INDEX IF NOT EXISTS idx_unsupported_css_log_occurrences
        ON unsupported_css_log (occurrences DESC);",
    )?;
    let has_category = conn
        .prepare("PRAGMA table_info(unsupported_css_log)")?
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<Vec<_>, _>>()?
        .iter()
        .any(|column| column == "category");
    if !has_category {
        conn.execute(
            "ALTER TABLE unsupported_css_log ADD COLUMN category TEXT NOT NULL DEFAULT 'unsupported'",
            [],
        )?;
    }
    Ok(())
}

#[cfg(test)]
fn persist_unsupported_css_to_sqlite(path: &str, property: &str, value: &str) {
    persist_css_audit_to_sqlite(path, CssAuditCategory::Unsupported, property, value);
}

fn persist_css_audit_to_sqlite(
    path: &str,
    category: CssAuditCategory,
    property: &str,
    value: &str,
) {
    let result: Result<(), rusqlite::Error> = SQLITE_CONNECTIONS.with(|connections| {
        let mut connections = connections.borrow_mut();
        if !connections.contains_key(path) {
            let mut conn = Connection::open(path)?;
            configure_sqlite_connection(&mut conn)?;
            ensure_unsupported_css_sqlite_schema(&conn)?;
            connections.insert(path.to_string(), conn);
        }

        let conn = connections
            .get_mut(path)
            .expect("sqlite connection must exist after initialization");
        conn.execute(
            "INSERT INTO unsupported_css_log (property, value, category, occurrences)
             VALUES (?1, ?2, ?3, 1)
             ON CONFLICT(property, value) DO UPDATE SET
               category = excluded.category,
               occurrences = unsupported_css_log.occurrences + 1,
               last_seen_at = CURRENT_TIMESTAMP",
            params![property, value, category.as_str()],
        )?;
        Ok(())
    });

    if let Err(error) = result {
        log_sqlite_error(&error);
    }
}

fn emit_css_audit_top_n_summary_if_updated(path: &str, top_n: usize, category: CssAuditCategory) {
    let rows = SQLITE_CONNECTIONS.with(|connections| {
        let mut connections = connections.borrow_mut();
        let Some(conn) = connections.get_mut(path) else {
            return Ok(Vec::new());
        };
        query_css_audit_top_n(conn, top_n, category)
    });
    let Ok(rows) = rows else {
        if let Err(error) = rows {
            log_sqlite_error(&error);
        }
        return;
    };
    if rows.is_empty() {
        return;
    }

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    path.hash(&mut hasher);
    top_n.hash(&mut hasher);
    category.as_str().hash(&mut hasher);
    for (property, value, occurrences) in &rows {
        property.hash(&mut hasher);
        value.hash(&mut hasher);
        occurrences.hash(&mut hasher);
    }
    let digest = hasher.finish();
    let key = format!("{path}#{top_n}#{}", category.as_str());
    let map = UNSUPPORTED_CSS_TOP_N_LAST_DIGEST.get_or_init(|| Mutex::new(HashMap::new()));
    let mut map = map
        .lock()
        .expect("unsupported css top-n digest lock poisoned");
    if map.get(&key).copied() == Some(digest) {
        return;
    }
    map.insert(key, digest);

    let label = category.log_label();
    eprintln!("[omoikane][{label}][top-n] top {top_n} candidates (site/url anonymized)");
    for (index, (property, value, occurrences)) in rows.iter().enumerate() {
        let value = truncate_log_value(value, MAX_UNSUPPORTED_LOG_VALUE_LEN);
        eprintln!(
            "[omoikane][{label}][top-n] {}. {}={} (count={})",
            index + 1,
            property,
            value,
            occurrences
        );
    }
}

#[cfg(test)]
fn query_unsupported_css_top_n(
    conn: &Connection,
    top_n: usize,
) -> Result<Vec<(String, String, i64)>, rusqlite::Error> {
    query_css_audit_top_n(conn, top_n, CssAuditCategory::Unsupported)
}

fn query_css_audit_top_n(
    conn: &Connection,
    top_n: usize,
    category: CssAuditCategory,
) -> Result<Vec<(String, String, i64)>, rusqlite::Error> {
    let limit = i64::try_from(top_n).unwrap_or(i64::MAX);
    let mut stmt = conn.prepare(
        "SELECT property, value, occurrences
         FROM unsupported_css_log
         WHERE category = ?1
         ORDER BY occurrences DESC, property ASC, value ASC
         LIMIT ?2",
    )?;
    stmt.query_map(params![category.as_str(), limit], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i64>(2)?,
        ))
    })?
    .collect::<Result<Vec<_>, _>>()
}

fn configure_sqlite_connection(conn: &mut Connection) -> Result<(), rusqlite::Error> {
    conn.busy_timeout(Duration::from_millis(5000))?;
    conn.execute_batch(
        "PRAGMA journal_mode=WAL;
         PRAGMA synchronous=NORMAL;",
    )?;
    Ok(())
}

#[cfg(test)]
fn close_sqlite_connection_for_path(path: &str) {
    SQLITE_CONNECTIONS.with(|connections| {
        connections.borrow_mut().remove(path);
    });
}

fn log_sqlite_error(error: &rusqlite::Error) {
    let error_key = format!("{error}");
    let errors = SQLITE_LOG_ERRORS.get_or_init(|| Mutex::new(HashSet::new()));
    let mut errors = errors.lock().expect("sqlite css log error lock poisoned");
    if errors.contains(&error_key) {
        return;
    }
    if errors.len() >= MAX_SQLITE_LOG_ERRORS {
        return;
    }
    errors.insert(error_key.clone());
    eprintln!("[omoikane][unsupported-css][sqlite-error] {error_key}");
}

fn should_ignore_unsupported_css_logging(property: &str) -> bool {
    property.starts_with("--")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CssAuditCategory {
    Unsupported,
    VendorPrefixed,
}

impl CssAuditCategory {
    fn as_str(self) -> &'static str {
        match self {
            Self::Unsupported => "unsupported",
            Self::VendorPrefixed => "vendor-prefixed",
        }
    }

    fn log_label(self) -> &'static str {
        match self {
            Self::Unsupported => "unsupported-css",
            Self::VendorPrefixed => "vendor-prefixed-css",
        }
    }
}

fn css_audit_category(property: &str) -> Option<CssAuditCategory> {
    if should_ignore_unsupported_css_logging(property) || is_supported_property(property) {
        None
    } else if property.starts_with('-') {
        Some(CssAuditCategory::VendorPrefixed)
    } else {
        Some(CssAuditCategory::Unsupported)
    }
}

fn unsupported_css_dedup_key(property: &str, value: &str) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    value.hash(&mut hasher);
    format!("{property}#{}#{}", value.len(), hasher.finish())
}

fn sanitize_unsupported_css_log_value(value: &str) -> String {
    const URL_PREFIXES: [&str; 6] = ["http://", "https://", "ws://", "wss://", "ftp://", "data:"];
    let mut out = String::with_capacity(value.len());
    let mut cursor = 0usize;

    while cursor < value.len() {
        let tail = &value[cursor..];
        let mut matched_prefix = false;
        for prefix in URL_PREFIXES {
            if tail.len() >= prefix.len() && tail[..prefix.len()].eq_ignore_ascii_case(prefix) {
                matched_prefix = true;
                out.push_str("[redacted-url]");
                let mut consumed = 0usize;
                for (offset, ch) in tail.char_indices() {
                    if offset > 0 && is_url_terminator(ch) {
                        break;
                    }
                    consumed = offset + ch.len_utf8();
                }
                cursor += consumed.max(prefix.len());
                break;
            }
        }
        if matched_prefix {
            continue;
        }

        let ch = tail
            .chars()
            .next()
            .expect("tail must have at least one char");
        out.push(ch);
        cursor += ch.len_utf8();
    }

    out
}

fn is_url_terminator(ch: char) -> bool {
    ch.is_ascii_whitespace() || matches!(ch, '"' | '\'' | ')' | '(' | '<' | '>')
}

fn truncate_log_value(value: &str, max_len: usize) -> String {
    if value.chars().count() <= max_len {
        return value.to_string();
    }
    let mut out = value.chars().take(max_len).collect::<String>();
    out.push_str("...");
    out
}

pub(super) fn is_supported_property(name: &str) -> bool {
    PropertyId::parse(name).is_some() || is_shorthand_or_legacy_alias(name)
}

/// Returns every implemented longhand affected by the CSS `all` shorthand.
///
/// The list is derived from the supported-property registry so adding an
/// implemented longhand automatically makes it participate in `all`. CSS
/// Cascade excludes `direction`, `unicode-bidi`, aliases, and custom
/// properties. Custom properties do not appear in the fixed registry.
pub(super) fn all_longhand_properties() -> impl Iterator<Item = &'static str> {
    SUPPORTED_PROPERTIES.iter().copied().filter(|name| {
        !matches!(*name, "direction" | "unicode-bidi")
            && !is_shorthand_or_legacy_alias(name)
            && canonical_property_name(name) == *name
    })
}

/// Returns whether a property/value pair is both syntactically valid and
/// implemented by Omoikane's style resolver.
///
/// This is the engine-side source of truth for the DOM `CSS.supports()` API.
/// Parsing a forgiving declaration list alone is insufficient: the CSS parser
/// deliberately retains unknown properties so they can be ignored by the
/// cascade and reported by diagnostics. Feature detection must additionally
/// require that every declaration produced by shorthand expansion is a
/// property the resolver understands.
pub(crate) fn supports_declaration(property: &str, value: &str) -> bool {
    let property = property.trim();
    let value = close_unfinished_functions(value.trim());
    let value = value.as_str();
    if property.is_empty() || value.is_empty() || contains_top_level_semicolon(value) {
        return false;
    }
    if matches!(
        property.to_ascii_lowercase().as_str(),
        "counter-reset" | "counter-increment" | "counter-set"
    ) {
        return normalize_counter_specified(property, value).is_some();
    }
    if (property.starts_with("inset")
        || property.starts_with("margin-")
        || property.starts_with("padding-")
        || property.starts_with("border-inline")
        || property.starts_with("border-block"))
        && super::split_top_level_commas(value).len() > 1
    {
        return false;
    }

    let declarations = super::parse_style_attribute(&format!("{property}: {value}"));
    if declarations.is_empty() {
        return false;
    }

    // Custom properties accept the general CSS component-value grammar and
    // are consumed by var() resolution rather than the fixed property table.
    if property.starts_with("--") {
        return declarations.len() == 1 && declarations[0].name == property.to_ascii_lowercase();
    }

    declarations.iter().all(|declaration| {
        let name = canonical_property_name(&declaration.name);
        if !is_supported_property(name) {
            return false;
        }
        // A declaration containing var() is syntactically valid at parse time;
        // its property grammar is checked after custom-property substitution.
        if value_contains_var_function(&declaration.value) {
            return true;
        }
        match validate_declaration(name, &declaration.value) {
            DeclarationValidation::Invalid => false,
            DeclarationValidation::Valid(_) => true,
            DeclarationValidation::Unvalidated => {
                let computed =
                    compute_value(&declaration.value, name, ResolutionContext::default());
                !should_skip_computed_property(name, &computed)
            }
        }
    })
}

pub(crate) fn normalize_counter_specified(property: &str, text: &str) -> Option<String> {
    let property = property.to_ascii_lowercase();
    let default = if property == "counter-increment" {
        1
    } else {
        0
    };
    if !matches!(
        property.as_str(),
        "counter-reset" | "counter-increment" | "counter-set"
    ) || super::split_top_level_commas(text).len() != 1
    {
        return None;
    }
    let declarations = super::parse_style_attribute(&format!("{property}: {text}"));
    let declaration = declarations
        .as_slice()
        .first()
        .filter(|_| declarations.len() == 1)?;
    if let Value::Keyword(keyword) = &declaration.value
        && (keyword.eq_ignore_ascii_case("none")
            || is_css_wide_keyword(&keyword.to_ascii_lowercase()))
    {
        return Some(keyword.to_ascii_lowercase());
    }
    let values = match &declaration.value {
        Value::List(values) => values.as_slice(),
        value => std::slice::from_ref(value),
    };
    let mut output = Vec::new();
    let mut index = 0;
    while index < values.len() {
        let Value::Keyword(name) = &values[index] else {
            return None;
        };
        let lower = name.to_ascii_lowercase();
        if is_css_wide_keyword(&lower) || matches!(lower.as_str(), "none" | "default") {
            return None;
        }
        output.push(name.clone());
        index += 1;
        match values.get(index) {
            Some(Value::Number(number)) if number.is_finite() && number.fract() == 0.0 => {
                output.push((*number as i64).to_string());
                index += 1;
            }
            Some(Value::Function { name, .. }) if name.eq_ignore_ascii_case("calc") => {
                counter_integer_with_context(&values[index], ResolutionContext::default())?;
                output.push(render_value(&values[index]));
                index += 1;
            }
            Some(Value::Number(_)) | Some(Value::Function { .. }) => return None,
            _ => output.push(default.to_string()),
        }
    }
    if text.contains('\\') || text.to_ascii_lowercase().contains("calc(") {
        Some(text.trim().to_owned())
    } else {
        Some(output.join(" "))
    }
}

fn close_unfinished_functions(value: &str) -> String {
    let Ok(tokens) = super::tokenize(value) else {
        return value.to_string();
    };
    let mut depth = 0usize;
    for token in tokens {
        match token {
            CssToken::ParenOpen => depth += 1,
            CssToken::ParenClose => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    let mut repaired = value.to_string();
    repaired.extend(std::iter::repeat_n(')', depth));
    repaired
}

/// Canonicalizes a logical border width assigned through CSSOM.
///
/// The specified-value serializer keeps relative units, but orders simple
/// `calc()` terms and turns unitless zero into a length as CSSOM requires.
pub(crate) fn normalize_logical_border_width(property: &str, text: &str) -> Option<String> {
    if !supports_declaration(property, text) {
        return None;
    }
    let declarations = super::parse_style_attribute(&format!("{property}: {text}"));
    let values = declarations
        .iter()
        .map(|declaration| normalize_border_width_component(&declaration.value))
        .collect::<Vec<_>>();
    match values.as_slice() {
        [start, end] if start == end => Some(start.clone()),
        [start, end] => Some(format!("{start} {end}")),
        [single] => Some(single.clone()),
        _ => None,
    }
}

fn normalize_border_width_component(value: &Value) -> String {
    if matches!(value, Value::Number(number) if *number == 0.0) {
        return "0px".to_string();
    }
    if let Value::Function { name, arguments } = value
        && name.eq_ignore_ascii_case("calc")
        && let [Value::List(terms)] = arguments.as_slice()
        && let [
            Value::Length(first, first_unit),
            Value::Keyword(operator),
            Value::Length(second, second_unit),
        ] = terms.as_slice()
        && matches!(operator.as_str(), "+" | "-")
        && second_unit < first_unit
    {
        let second = if operator == "-" { -*second } else { *second };
        return format!("calc({second}{second_unit} + {first}{first_unit})");
    }
    render_value(value)
}

/// Validates and canonicalizes inset and logical margin/padding lengths.
pub(crate) fn normalize_box_lengths(property: &str, text: &str) -> Option<String> {
    if !supports_declaration(property, text) {
        return None;
    }
    let declarations = super::parse_style_attribute(&format!("{property}: {text}"));
    let values = declarations
        .iter()
        .map(|declaration| normalize_border_width_component(&declaration.value))
        .collect::<Vec<_>>();
    match values.as_slice() {
        [single] => Some(single.clone()),
        [start, end] => Some(if start == end {
            start.clone()
        } else {
            format!("{start} {end}")
        }),
        [top, right, bottom, left] if top == right && top == bottom && top == left => {
            Some(top.clone())
        }
        [top, right, bottom, left] if top == bottom && right == left => {
            Some(format!("{top} {right}"))
        }
        [top, right, bottom, left] if right == left => Some(format!("{top} {right} {bottom}")),
        [top, right, bottom, left] => Some(format!("{top} {right} {bottom} {left}")),
        _ => None,
    }
}

/// Serializes a logical border color while preserving named color spellings.
pub(crate) fn normalize_logical_border_color(property: &str, text: &str) -> Option<String> {
    if !supports_declaration(property, text) {
        return None;
    }
    let declarations = super::parse_style_attribute(&format!("{property}: {text}"));
    let values = declarations
        .iter()
        .map(|declaration| {
            let value = render_value(&declaration.value);
            if value.starts_with('#') {
                let color = crate::paint::color::parse_color(&value)?;
                Some(format!("rgb({}, {}, {})", color.r, color.g, color.b))
            } else {
                Some(value)
            }
        })
        .collect::<Option<Vec<_>>>()?;
    match values.as_slice() {
        [single] => Some(single.clone()),
        [start, end] if start == end => Some(start.clone()),
        [start, end] => Some(format!("{start} {end}")),
        _ => None,
    }
}

/// Validates and serializes a logical border shorthand in width/style/color order.
pub(crate) fn normalize_logical_border_shorthand(property: &str, text: &str) -> Option<String> {
    if !supports_declaration(property, text) {
        return None;
    }
    if matches!(
        text.trim().to_ascii_lowercase().as_str(),
        "initial" | "inherit" | "unset" | "revert" | "revert-layer"
    ) {
        return Some(text.trim().to_ascii_lowercase());
    }
    let declarations = super::parse_style_attribute(&format!("{property}: {text}"));
    let mut width = None;
    let mut style = None;
    let mut color = None;
    for declaration in declarations {
        match declaration.name.rsplit('-').next()? {
            "width" => width = Some(normalize_border_width_component(&declaration.value)),
            "style" => style = Some(render_value(&declaration.value)),
            "color" => color = Some(render_value(&declaration.value)),
            _ => return None,
        }
    }
    let values = [width, style, color]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    (!values.is_empty()).then(|| values.join(" "))
}

/// Validates and canonicalizes the specified underline longhand value for CSSOM.
pub(crate) fn normalize_underline_value(property: &str, value: &str) -> Option<String> {
    if !supports_declaration(property, value) {
        return None;
    }
    let declarations = super::parse_style_attribute(&format!("{property}: {value}"));
    match validate_declaration(property, &declarations.first()?.value) {
        DeclarationValidation::Valid(computed) => Some(computed_value_css_text(&computed)),
        _ => Some(value.trim().to_string()),
    }
}

/// Validates and canonicalizes the specified CSSOM value of `text-indent` or
/// `word-spacing` while preserving unresolved relative units.
pub(crate) fn normalize_text_spacing_value(property: &str, text: &str) -> Option<String> {
    let repaired = close_unfinished_functions(text.trim());
    if !supports_declaration(property, &repaired) {
        return None;
    }
    let declarations = super::parse_style_attribute(&format!("{property}: {repaired}"));
    let value = &declarations.first()?.value;
    if property.eq_ignore_ascii_case("text-indent") {
        let (amount, hanging, each_line) = text_indent_components(value)?;
        let mut parts = vec![normalize_spacing_length_percentage(amount)];
        if hanging {
            parts.push("hanging".to_string());
        }
        if each_line {
            parts.push("each-line".to_string());
        }
        return Some(parts.join(" "));
    }
    Some(normalize_spacing_length_percentage(value))
}

fn normalize_spacing_length_percentage(value: &Value) -> String {
    if matches!(value, Value::Number(number) if *number == 0.0) {
        return "0px".to_string();
    }
    if let Value::Function { name, arguments } = value
        && name.eq_ignore_ascii_case("calc")
    {
        if spacing_math_uses_only_px_and_percentages(value) {
            let computed = compute_value(value, "word-spacing", ResolutionContext::default());
            if !matches!(computed, ComputedValue::Keyword(_)) {
                return computed.css_text();
            }
        }
        if let [Value::List(terms)] = arguments.as_slice()
            && let [
                Value::Length(length, unit),
                Value::Keyword(operator),
                Value::Percentage(percentage),
            ] = terms.as_slice()
            && matches!(operator.as_str(), "+" | "-")
        {
            let percentage = if operator == "-" {
                -*percentage
            } else {
                *percentage
            };
            return format!("calc({percentage}% + {length}{unit})");
        }
    }
    render_value(value)
}

fn spacing_math_uses_only_px_and_percentages(value: &Value) -> bool {
    match value {
        Value::Length(_, unit) => unit.eq_ignore_ascii_case("px"),
        Value::Percentage(_) | Value::Number(_) => true,
        Value::Keyword(operator) => matches!(operator.as_str(), "+" | "-" | "*" | "/"),
        Value::List(values) | Value::CommaList(values) => {
            values.iter().all(spacing_math_uses_only_px_and_percentages)
        }
        Value::Function { name, arguments } if name.eq_ignore_ascii_case("calc") => arguments
            .iter()
            .all(spacing_math_uses_only_px_and_percentages),
        _ => false,
    }
}

/// Evaluates fixed transform number math without DOM or layout placeholders.
pub(crate) fn transform_number_math(text: &str) -> Option<f32> {
    let declarations = super::parse_style_attribute(&format!("--transform: {text}"));
    individual_transform::fixed_number_expression(&declarations.first()?.value)
}

/// Evaluates fixed percentage math for a transform scale argument.
pub(crate) fn transform_percentage_math(text: &str) -> Option<f32> {
    let declarations = super::parse_style_attribute(&format!("--transform: {text}"));
    individual_transform::fixed_percentage_expression(&declarations.first()?.value)
}

/// Evaluates fixed transform angle math and converts the result to radians.
pub(crate) fn transform_angle_math(text: &str) -> Option<f32> {
    let declarations = super::parse_style_attribute(&format!("--transform: {text}"));
    let Value::Length(angle, _) =
        individual_transform::fixed_angle_expression(&declarations.first()?.value)?
    else {
        return None;
    };
    Some(angle.to_radians())
}

/// Resolves supported transform length math against explicit box/font inputs.
pub(crate) fn transform_length_math(
    text: &str,
    percentage_basis: Option<f32>,
    reference: super::TransformReferenceBox,
) -> Option<f32> {
    fn supported_units(value: &Value) -> bool {
        match value {
            Value::Length(_, unit) => {
                matches!(unit.to_ascii_lowercase().as_str(), "px" | "em" | "rem")
            }
            Value::Function { arguments, .. } | Value::List(arguments) => {
                arguments.iter().all(supported_units)
            }
            _ => true,
        }
    }
    let declarations = super::parse_style_attribute(&format!("--transform: {text}"));
    let value = &declarations.first()?.value;
    if !supported_units(value) {
        return None;
    }
    let hint = percentage_basis.map(|_| typed_math::NumericType::LENGTH);
    if typed_math::infer(value, hint) != Some(typed_math::NumericType::LENGTH) {
        return None;
    }
    let context = ResolutionContext {
        parent_font_size: reference.font_size,
        root_font_size: reference.root_font_size,
        ..ResolutionContext::default()
    };
    let result =
        evaluate_length_percentage_math(value, context)?.resolve(percentage_basis.unwrap_or(0.0));
    result.is_finite().then_some(result)
}

/// Computes context-independent, nonnegative integer font feature values.
/// Runtime values and units requiring font or layout inputs are rejected.
pub(crate) fn font_feature_integer_values(text: &str) -> Option<Vec<u32>> {
    let declarations = super::parse_style_attribute(&format!("--feature: {text}"));
    font_feature_value_numbers(&declarations.first()?.value)
}

/// Reports element-dependent tree counts, including nested component values.
pub(crate) fn value_contains_tree_counting(value: &Value) -> bool {
    sibling_functions::contains(value)
}

/// Reports whether substitution is required anywhere in a component value.
pub(crate) fn value_contains_var_function(value: &Value) -> bool {
    match value {
        Value::Function { name, arguments } => {
            name.eq_ignore_ascii_case("var") || arguments.iter().any(value_contains_var_function)
        }
        Value::List(values) | Value::CommaList(values) => {
            values.iter().any(value_contains_var_function)
        }
        _ => false,
    }
}

/// Resolves Color 4 components against an explicit geometry/font snapshot.
fn compute_color4_value(value: &Value, ctx: ResolutionContext) -> ComputedValue {
    let text = render_value(value);
    let context = crate::paint::color4::ColorContext {
        font: f64::from(ctx.parent_font_size),
        root_font: f64::from(ctx.root_font_size),
        viewport: [
            f64::from(ctx.viewport_width),
            f64::from(ctx.viewport_height),
        ],
        container: [
            ctx.container_size[0].unwrap_or(ctx.viewport_width),
            ctx.container_size[1].unwrap_or(ctx.viewport_height),
        ]
        .map(f64::from),
    };
    crate::paint::color4::CssColor::parse_with_context(&text, Some(context))
        .map(|color| ComputedValue::Color(color.serialize_computed()))
        .unwrap_or_else(|| ComputedValue::Keyword(text))
}

/// Reject a second top-level declaration while preserving semicolons inside
/// strings and functions such as data URLs.
fn contains_top_level_semicolon(value: &str) -> bool {
    let Ok(tokens) = super::tokenize(value) else {
        return true;
    };
    let mut depth = 0usize;
    for token in tokens {
        match token {
            super::CssToken::ParenOpen | super::CssToken::BracketOpen => depth += 1,
            super::CssToken::ParenClose | super::CssToken::BracketClose => {
                depth = depth.saturating_sub(1);
            }
            super::CssToken::Semicolon if depth == 0 => return true,
            _ => {}
        }
    }
    false
}

fn resolve_time_seconds(value: &Value) -> Option<f32> {
    if typed_math::infer(value, None) == Some(typed_math::NumericType::TIME)
        && let Value::Length(milliseconds, unit) =
            individual_transform::computed_numeric_expression(value, ResolutionContext::default())?
        && unit.eq_ignore_ascii_case("ms")
    {
        return Some(milliseconds / 1000.0);
    }
    match value {
        Value::Length(number, unit) if unit.eq_ignore_ascii_case("s") => Some(*number),
        Value::Length(number, unit) if unit.eq_ignore_ascii_case("ms") => Some(*number / 1000.0),
        Value::Number(number) if *number == 0.0 => Some(0.0),
        Value::Function { name, arguments } if name.eq_ignore_ascii_case("calc") => {
            resolve_time_calc(arguments.first()?)
        }
        Value::List(_) => resolve_time_calc(value),
        _ => None,
    }
}

fn resolve_time_calc(value: &Value) -> Option<f32> {
    let Value::List(values) = value else {
        return resolve_time_seconds(value);
    };
    let mut total = 0.0;
    let mut sign = 1.0;
    let mut expects_value = true;
    for value in values {
        match value {
            Value::Keyword(operator) if operator == "+" || operator == "-" => {
                if expects_value {
                    return None;
                }
                sign = if operator == "-" { -1.0 } else { 1.0 };
                expects_value = true;
            }
            value if expects_value => {
                total += sign * resolve_time_seconds(value)?;
                expects_value = false;
            }
            _ => return None,
        }
    }
    (!expects_value).then_some(total)
}

fn serialize_opentype_tag(tag: &str) -> Option<String> {
    (tag.len() == 4 && tag.bytes().all(|byte| (0x20..=0x7e).contains(&byte)))
        .then(|| format!("\"{}\"", tag.replace('\\', "\\\\").replace('"', "\\\"")))
}

fn serialize_opentype_number(number: f32) -> String {
    let rounded = (number * 1_000_000.0).round() / 1_000_000.0;
    rounded.to_string()
}

/// Canonicalizes an OpenType settings specified value for CSSOM.
pub(crate) fn normalize_opentype_settings_specified(
    property: &str,
    value: &Value,
) -> Option<String> {
    let feature = property.eq_ignore_ascii_case("font-feature-settings");
    if matches!(value, Value::Keyword(keyword) if keyword.eq_ignore_ascii_case("normal")) {
        return Some("normal".to_string());
    }
    let setting = |value: &Value| {
        let (tag, amount) = match value {
            Value::String(tag) if feature => (tag, None),
            Value::List(values) => match values.as_slice() {
                [Value::String(tag)] if feature => (tag, None),
                [Value::String(tag), Value::Keyword(keyword)]
                    if feature && keyword.eq_ignore_ascii_case("on") =>
                {
                    (tag, None)
                }
                [Value::String(tag), Value::Keyword(keyword)]
                    if feature && keyword.eq_ignore_ascii_case("off") =>
                {
                    (tag, Some("0".to_string()))
                }
                [Value::String(tag), Value::Number(number)] => {
                    let amount = if feature && *number == 1.0 {
                        None
                    } else {
                        Some(render_value(&Value::Number(*number)))
                    };
                    (tag, amount)
                }
                [Value::String(tag), amount]
                    if typed_math::infer(amount, None) == Some(typed_math::NumericType::NUMBER) =>
                {
                    (tag, Some(render_value(amount)))
                }
                _ => return None,
            },
            _ => return None,
        };
        let tag = serialize_opentype_tag(tag)?;
        Some(amount.map_or(tag.clone(), |amount| format!("{tag} {amount}")))
    };
    match value {
        Value::CommaList(values) => values
            .iter()
            .map(setting)
            .collect::<Option<Vec<_>>>()
            .map(|values| values.join(", ")),
        value => setting(value),
    }
}

/// Computes each axis value while preserving the quoted OpenType axis tag.
fn compute_font_variation_settings(value: &Value, ctx: ResolutionContext) -> Option<ComputedValue> {
    fn axis(value: &Value, ctx: ResolutionContext) -> Option<(String, String)> {
        let Value::List(values) = value else {
            return None;
        };
        let [Value::String(tag), amount] = values.as_slice() else {
            return None;
        };
        let serialized_tag = serialize_opentype_tag(tag)?;
        let LengthMathValue::Number(number) = evaluate_length_math_value(amount, ctx)? else {
            return None;
        };
        Some((
            tag.clone(),
            format!("{serialized_tag} {}", serialize_opentype_number(number)),
        ))
    }
    if matches!(value, Value::Keyword(keyword) if keyword.eq_ignore_ascii_case("normal")) {
        return Some(ComputedValue::Keyword("normal".to_string()));
    }
    let values = match value {
        Value::CommaList(values) => values.as_slice(),
        value => std::slice::from_ref(value),
    };
    let mut axes = std::collections::BTreeMap::new();
    for value in values {
        let (tag, serialized) = axis(value, ctx)?;
        axes.insert(tag, serialized);
    }
    Some(ComputedValue::Keyword(
        axes.into_values().collect::<Vec<_>>().join(", "),
    ))
}

/// Computes feature values while preserving quoted four-byte OpenType tags.
fn compute_font_feature_settings(value: &Value, ctx: ResolutionContext) -> Option<ComputedValue> {
    fn feature(value: &Value, ctx: ResolutionContext) -> Option<(String, String)> {
        let (tag, amount) = match value {
            Value::String(tag) => (tag, 1),
            Value::List(values) => match values.as_slice() {
                [Value::String(tag)] => (tag, 1),
                [Value::String(tag), Value::Keyword(keyword)]
                    if keyword.eq_ignore_ascii_case("on") =>
                {
                    (tag, 1)
                }
                [Value::String(tag), Value::Keyword(keyword)]
                    if keyword.eq_ignore_ascii_case("off") =>
                {
                    (tag, 0)
                }
                [Value::String(tag), amount] => {
                    let LengthMathValue::Number(number) = evaluate_length_math_value(amount, ctx)?
                    else {
                        return None;
                    };
                    if !number.is_finite() || number < 0.0 || number.fract() != 0.0 {
                        return None;
                    }
                    (tag, number.min(u32::MAX as f32) as u32)
                }
                _ => return None,
            },
            _ => return None,
        };
        let serialized_tag = serialize_opentype_tag(tag)?;
        let serialized = if amount == 1 {
            serialized_tag
        } else {
            format!("{serialized_tag} {amount}")
        };
        Some((tag.clone(), serialized))
    }
    if matches!(value, Value::Keyword(keyword) if keyword.eq_ignore_ascii_case("normal")) {
        return Some(ComputedValue::Keyword("normal".to_string()));
    }
    let values = match value {
        Value::CommaList(values) => values.as_slice(),
        value => std::slice::from_ref(value),
    };
    let mut features = std::collections::BTreeMap::new();
    for value in values {
        let (tag, serialized) = feature(value, ctx)?;
        features.insert(tag, serialized);
    }
    Some(ComputedValue::Keyword(
        features.into_values().collect::<Vec<_>>().join(", "),
    ))
}

fn serialized_opentype_settings(value: &ComputedValue) -> Vec<([u8; 4], &str)> {
    let ComputedValue::Keyword(text) = value else {
        return Vec::new();
    };
    if text.eq_ignore_ascii_case("normal") {
        return Vec::new();
    }
    let bytes = text.as_bytes();
    let mut cursor = 0;
    let mut settings = Vec::new();
    while cursor < bytes.len() {
        while cursor < bytes.len() && (bytes[cursor] == b' ' || bytes[cursor] == b',') {
            cursor += 1;
        }
        if bytes.get(cursor) != Some(&b'"') {
            return Vec::new();
        }
        cursor += 1;
        let mut tag = [0; 4];
        for byte in &mut tag {
            let Some(&next) = bytes.get(cursor) else {
                return Vec::new();
            };
            if next == b'\\' {
                cursor += 1;
                let Some(&escaped) = bytes.get(cursor) else {
                    return Vec::new();
                };
                *byte = escaped;
            } else {
                *byte = next;
            }
            cursor += 1;
        }
        if bytes.get(cursor) != Some(&b'"') {
            return Vec::new();
        }
        cursor += 1;
        while bytes.get(cursor) == Some(&b' ') {
            cursor += 1;
        }
        let value_start = cursor;
        while cursor < bytes.len() && bytes[cursor] != b',' {
            cursor += 1;
        }
        let setting_value = text[value_start..cursor].trim();
        let setting_value = if setting_value.is_empty() {
            "1"
        } else {
            setting_value
        };
        settings.push((tag, setting_value));
    }
    settings
}

fn computed_font_synthesis(properties: &PropertyMap) -> crate::font::FontSynthesis {
    let Some(ComputedValue::Keyword(value)) = properties.get(&PropertyId::FontSynthesis) else {
        return crate::font::FontSynthesis::default();
    };
    let words = value.split_ascii_whitespace().collect::<HashSet<_>>();
    crate::font::FontSynthesis {
        weight: words.contains("weight"),
        style: if words.contains("style") {
            crate::font::FontStyleSynthesis::Auto
        } else if words.contains("oblique-only") {
            crate::font::FontStyleSynthesis::ObliqueOnly
        } else {
            crate::font::FontStyleSynthesis::None
        },
        small_caps: words.contains("small-caps"),
        position: words.contains("position"),
    }
}

/// Validates and canonicalizes a keyword-list font value for CSSOM.
pub(crate) fn normalize_font_keyword_list_specified(
    property: &str,
    value: &Value,
) -> Option<String> {
    match validate_font_variant_declaration(property, value)? {
        DeclarationValidation::Valid(ComputedValue::Keyword(value)) => Some(value),
        _ => None,
    }
}

#[derive(Default)]
struct FontPaletteValues {
    named: HashMap<(String, FontFamilyKey), crate::font::FontPalette>,
}

impl FontPaletteValues {
    fn resolve(&self, name: &str, families: &[FontFamilyKey]) -> Option<crate::font::FontPalette> {
        families.iter().find_map(|family| {
            self.named
                .get(&(name.to_ascii_lowercase(), *family))
                .cloned()
        })
    }
}

fn resolve_font_palette_value(
    value: &Value,
    definitions: &FontPaletteValues,
    families: &[FontFamilyKey],
    depth: usize,
) -> Option<crate::font::FontPalette> {
    if depth >= 16 {
        return None;
    }
    if let Value::Keyword(name) = value {
        if name.eq_ignore_ascii_case("normal") {
            return Some(crate::font::FontPalette::default());
        }
        if name.eq_ignore_ascii_case("light") || name.eq_ignore_ascii_case("dark") {
            return Some(crate::font::FontPalette {
                base: if name.eq_ignore_ascii_case("light") {
                    crate::font::FontPaletteBase::Light
                } else {
                    crate::font::FontPaletteBase::Dark
                },
                overrides: Vec::new(),
                mix: None,
            });
        }
        return name
            .starts_with("--")
            .then(|| definitions.resolve(name, families))
            .flatten();
    }
    let (method, components) = font_palette::mix_components(value)?;
    let components = components
        .into_iter()
        .map(|(value, weight)| {
            Some(crate::font::FontPaletteMixComponent {
                palette: resolve_font_palette_value(value, definitions, families, depth + 1)?,
                weight_bits: weight.to_bits(),
            })
        })
        .collect::<Option<Vec<_>>>()?;
    Some(crate::font::FontPalette {
        base: crate::font::FontPaletteBase::default(),
        overrides: Vec::new(),
        mix: Some(Box::new(crate::font::FontPaletteMix { method, components })),
    })
}

fn collect_font_palette_values(
    rules: &[Rule],
    values: &mut FontPaletteValues,
    viewport_width: f32,
    viewport_height: f32,
    media_environment: &MediaEnvironment,
) {
    for rule in rules {
        let Rule::At(rule) = rule else {
            continue;
        };
        if rule.name.eq_ignore_ascii_case("font-palette-values") {
            let family = rule
                .declarations
                .iter()
                .rev()
                .find(|declaration| declaration.name.eq_ignore_ascii_case("font-family"))
                .map(|declaration| FontFamilyKey::new(&render_value(&declaration.value)));
            let Some(family) = family else {
                continue;
            };
            let base = rule
                .declarations
                .iter()
                .rev()
                .find(|declaration| declaration.name.eq_ignore_ascii_case("base-palette"))
                .and_then(|declaration| match &declaration.value {
                    Value::Number(number)
                        if number.is_finite()
                            && *number >= 0.0
                            && number.fract() == 0.0
                            && *number <= f32::from(u16::MAX) =>
                    {
                        Some(crate::font::FontPaletteBase::Index(*number as u16))
                    }
                    Value::Keyword(keyword) if keyword.eq_ignore_ascii_case("light") => {
                        Some(crate::font::FontPaletteBase::Light)
                    }
                    Value::Keyword(keyword) if keyword.eq_ignore_ascii_case("dark") => {
                        Some(crate::font::FontPaletteBase::Dark)
                    }
                    _ => None,
                })
                .unwrap_or_default();
            let overrides = rule
                .declarations
                .iter()
                .rev()
                .find(|declaration| declaration.name.eq_ignore_ascii_case("override-colors"))
                .and_then(|declaration| font_palette_overrides(&declaration.value))
                .unwrap_or_default();
            for family in family.member_keys() {
                values.named.insert(
                    (rule.prelude.to_ascii_lowercase(), family),
                    crate::font::FontPalette {
                        base,
                        overrides: overrides.clone(),
                        mix: None,
                    },
                );
            }
        } else if let Some(block) = &rule.block
            && !rule.name.eq_ignore_ascii_case("container")
            && layer_group_rule_is_active(rule, viewport_width, viewport_height, media_environment)
        {
            collect_font_palette_values(
                block,
                values,
                viewport_width,
                viewport_height,
                media_environment,
            );
        }
    }
}

fn font_palette_overrides(value: &Value) -> Option<Vec<(u16, [u8; 4])>> {
    let entries = match value {
        Value::CommaList(values) => values.as_slice(),
        value => std::slice::from_ref(value),
    };
    let mut overrides = Vec::new();
    for entry in entries {
        let Value::List(values) = entry else {
            return None;
        };
        let [Value::Number(index), color] = values.as_slice() else {
            return None;
        };
        if !index.is_finite() || *index < 0.0 || index.fract() != 0.0 {
            return None;
        }
        let color = crate::paint::color::parse_color(&render_value(color))?;
        if *index <= f32::from(u16::MAX) {
            overrides.push((*index as u16, [color.r, color.g, color.b, color.a]));
        }
    }
    Some(overrides)
}

#[derive(Default)]
struct FontFeatureValues {
    named: HashMap<(FontFamilyKey, String, String), Vec<u32>>,
}

impl FontFeatureValues {
    fn resolve(
        &self,
        families: Option<FontFamilyKey>,
        category: &str,
        name: &str,
    ) -> Option<&[u32]> {
        families?.member_keys().into_iter().find_map(|family| {
            self.named
                .get(&(family, category.to_string(), name.to_string()))
                .map(Vec::as_slice)
        })
    }
}

fn collect_font_feature_values(
    rules: &[Rule],
    values: &mut FontFeatureValues,
    viewport_width: f32,
    viewport_height: f32,
    media_environment: &MediaEnvironment,
) {
    for rule in rules {
        let Rule::At(rule) = rule else {
            continue;
        };
        if rule.name.eq_ignore_ascii_case("font-feature-values") {
            let families = FontFamilyKey::new(&rule.prelude).member_keys();
            for nested in rule.block.iter().flatten() {
                let Rule::At(nested) = nested else {
                    continue;
                };
                let category = nested.name.to_ascii_lowercase();
                if !matches!(
                    category.as_str(),
                    "stylistic"
                        | "styleset"
                        | "character-variant"
                        | "swash"
                        | "ornaments"
                        | "annotation"
                ) {
                    continue;
                }
                for declaration in &nested.declarations {
                    let Some(numbers) = font_feature_value_numbers(&declaration.value) else {
                        continue;
                    };
                    for family in &families {
                        values.named.insert(
                            (*family, category.clone(), declaration.name.to_lowercase()),
                            numbers.clone(),
                        );
                    }
                }
            }
        } else if let Some(block) = &rule.block
            && !rule.name.eq_ignore_ascii_case("container")
            && layer_group_rule_is_active(rule, viewport_width, viewport_height, media_environment)
        {
            collect_font_feature_values(
                block,
                values,
                viewport_width,
                viewport_height,
                media_environment,
            );
        }
    }
}

fn font_feature_value_numbers(value: &Value) -> Option<Vec<u32>> {
    let values = match value {
        Value::List(values) => values.as_slice(),
        value => std::slice::from_ref(value),
    };
    (!values.is_empty())
        .then(|| {
            values
                .iter()
                .map(|value| {
                    if matches!(value, Value::Number(number) if number.fract() != 0.0)
                        || matches!(value, Value::Keyword(_))
                    {
                        return None;
                    }
                    let number = individual_transform::fixed_number_expression(value)?;
                    (number >= 0.0 && f64::from(number) <= f64::from(u32::MAX))
                        .then_some(number.round() as u32)
                })
                .collect()
        })
        .flatten()
}

fn computed_shaping_options(
    properties: &PropertyMap,
    font_feature_values: &FontFeatureValues,
) -> crate::font::ShapingOptions {
    let words = |id| match properties.get(&id) {
        Some(ComputedValue::Keyword(value)) => value.split_ascii_whitespace().collect::<Vec<_>>(),
        _ => Vec::new(),
    };
    let mut features = Vec::new();
    let mut feature = |tag: &[u8; 4], value| {
        features.push(crate::font::OpenTypeFeature::new(*tag, value));
    };

    for value in words(PropertyId::FontVariantCaps) {
        match value {
            "small-caps" => feature(b"smcp", 1),
            "all-small-caps" => {
                feature(b"c2sc", 1);
                feature(b"smcp", 1);
            }
            "petite-caps" => feature(b"pcap", 1),
            "all-petite-caps" => {
                feature(b"c2pc", 1);
                feature(b"pcap", 1);
            }
            "unicase" => feature(b"unic", 1),
            "titling-caps" => feature(b"titl", 1),
            _ => {}
        }
    }
    for value in words(PropertyId::FontVariantLigatures) {
        match value {
            "none" => {
                for tag in [b"liga", b"clig", b"dlig", b"hlig", b"calt"] {
                    feature(tag, 0);
                }
            }
            "common-ligatures" => {
                feature(b"liga", 1);
                feature(b"clig", 1);
            }
            "no-common-ligatures" => {
                feature(b"liga", 0);
                feature(b"clig", 0);
            }
            "discretionary-ligatures" => feature(b"dlig", 1),
            "no-discretionary-ligatures" => feature(b"dlig", 0),
            "historical-ligatures" => feature(b"hlig", 1),
            "no-historical-ligatures" => feature(b"hlig", 0),
            "contextual" => feature(b"calt", 1),
            "no-contextual" => feature(b"calt", 0),
            _ => {}
        }
    }
    for value in words(PropertyId::FontVariantNumeric) {
        let tag = match value {
            "lining-nums" => Some(b"lnum"),
            "oldstyle-nums" => Some(b"onum"),
            "proportional-nums" => Some(b"pnum"),
            "tabular-nums" => Some(b"tnum"),
            "diagonal-fractions" => Some(b"frac"),
            "stacked-fractions" => Some(b"afrc"),
            "ordinal" => Some(b"ordn"),
            "slashed-zero" => Some(b"zero"),
            _ => None,
        };
        if let Some(tag) = tag {
            feature(tag, 1);
        }
    }
    for value in words(PropertyId::FontVariantEastAsian) {
        let tag = match value {
            "jis78" => Some(b"jp78"),
            "jis83" => Some(b"jp83"),
            "jis90" => Some(b"jp90"),
            "jis04" => Some(b"jp04"),
            "simplified" => Some(b"smpl"),
            "traditional" => Some(b"trad"),
            "full-width" => Some(b"fwid"),
            "proportional-width" => Some(b"pwid"),
            "ruby" => Some(b"ruby"),
            _ => None,
        };
        if let Some(tag) = tag {
            feature(tag, 1);
        }
    }
    match words(PropertyId::FontVariantPosition).as_slice() {
        ["sub"] => feature(b"subs", 1),
        ["super"] => feature(b"sups", 1),
        _ => {}
    }
    append_alternate_features(properties, font_feature_values, &mut feature);
    match words(PropertyId::FontKerning).as_slice() {
        ["normal"] => feature(b"kern", 1),
        ["none"] => feature(b"kern", 0),
        _ => {}
    }

    features.extend(
        properties
            .get(&PropertyId::FontFeatureSettings)
            .into_iter()
            .flat_map(serialized_opentype_settings)
            .filter_map(|(tag, value)| {
                value
                    .parse::<u32>()
                    .ok()
                    .map(|value| crate::font::OpenTypeFeature::new(tag, value))
            }),
    );

    let mut variations = Vec::new();
    let weight = match properties.get(&PropertyId::FontWeight) {
        Some(ComputedValue::Number(value)) => (*value).clamp(1.0, 1000.0),
        Some(ComputedValue::Keyword(value) | ComputedValue::String(value)) => {
            f32::from(crate::font::FontWeight::parse(value).0)
        }
        _ => 400.0,
    };
    variations.push(crate::font::OpenTypeVariation::new(*b"wght", weight));
    let stretch = match properties.get(&PropertyId::FontStretch) {
        Some(ComputedValue::Percentage(value)) => *value,
        Some(ComputedValue::Keyword(value) | ComputedValue::String(value)) => {
            crate::font::FontStretch::parse(value).0 as f32 / 1_000.0
        }
        _ => 100.0,
    };
    variations.push(crate::font::OpenTypeVariation::new(*b"wdth", stretch));
    let style = match properties.get(&PropertyId::FontStyle) {
        Some(ComputedValue::Keyword(value) | ComputedValue::String(value)) => {
            crate::font::FontStyleRange::parse(value)
        }
        _ => crate::font::FontStyleRange::exact(crate::font::FontStyle::Normal),
    };
    let (italic, slant) = match style.style {
        crate::font::FontStyle::Normal => (0.0, 0.0),
        crate::font::FontStyle::Italic => (1.0, 0.0),
        crate::font::FontStyle::Oblique => (0.0, -(style.requested_angle() as f32 / 1_000.0)),
    };
    variations.push(crate::font::OpenTypeVariation::new(*b"ital", italic));
    variations.push(crate::font::OpenTypeVariation::new(*b"slnt", slant));
    if words(PropertyId::FontOpticalSizing).as_slice() == ["auto"]
        && let Some(ComputedValue::Px(size)) = properties.get(&PropertyId::FontSize)
    {
        variations.push(crate::font::OpenTypeVariation::new(*b"opsz", *size));
    }
    variations.extend(
        properties
            .get(&PropertyId::FontVariationSettings)
            .into_iter()
            .flat_map(serialized_opentype_settings)
            .filter_map(|(tag, value)| {
                value
                    .parse::<f32>()
                    .ok()
                    .filter(|value| value.is_finite())
                    .map(|value| crate::font::OpenTypeVariation::new(tag, value))
            }),
    );
    let synthesis = computed_font_synthesis(properties);
    let synthetic_caps = if synthesis.small_caps {
        match words(PropertyId::FontVariantCaps).as_slice() {
            ["small-caps"] | ["petite-caps"] => crate::font::SyntheticCaps::Lowercase,
            ["all-small-caps"] | ["all-petite-caps"] => crate::font::SyntheticCaps::All,
            _ => crate::font::SyntheticCaps::None,
        }
    } else {
        crate::font::SyntheticCaps::None
    };
    let synthetic_position = if synthesis.position {
        match words(PropertyId::FontVariantPosition).as_slice() {
            ["sub"] => crate::font::SyntheticPosition::Subscript,
            ["super"] => crate::font::SyntheticPosition::Superscript,
            _ => crate::font::SyntheticPosition::None,
        }
    } else {
        crate::font::SyntheticPosition::None
    };
    crate::font::ShapingOptions {
        features,
        variations,
        synthetic_caps,
        synthetic_position,
    }
}

fn append_alternate_features(
    properties: &PropertyMap,
    values: &FontFeatureValues,
    feature: &mut impl FnMut(&[u8; 4], u32),
) {
    let Some(ComputedValue::Keyword(text)) = properties.get(&PropertyId::FontVariantAlternates)
    else {
        return;
    };
    if text.eq_ignore_ascii_case("normal") {
        return;
    }
    let families = match properties.get(&PropertyId::FontFamily) {
        Some(ComputedValue::Keyword(value) | ComputedValue::String(value)) => {
            Some(FontFamilyKey::new(value))
        }
        _ => None,
    };
    let declarations = super::parse_style_attribute(&format!("--alternates: {text}"));
    let Some(value) = declarations.first().map(|declaration| &declaration.value) else {
        return;
    };
    let components = match value {
        Value::List(values) => values.as_slice(),
        value => std::slice::from_ref(value),
    };
    for component in components {
        match component {
            Value::Keyword(keyword) if keyword.eq_ignore_ascii_case("historical-forms") => {
                feature(b"hist", 1);
            }
            Value::Function { name, arguments } => {
                for argument in arguments {
                    let Value::Keyword(identifier) = argument else {
                        continue;
                    };
                    let category = name.to_ascii_lowercase();
                    let Some(numbers) =
                        values.resolve(families, &category, &identifier.to_ascii_lowercase())
                    else {
                        continue;
                    };
                    match category.as_str() {
                        "stylistic" => {
                            if let [value] = numbers {
                                feature(b"salt", *value);
                            }
                        }
                        "styleset" => {
                            for number in numbers
                                .iter()
                                .copied()
                                .filter(|value| (1..=20).contains(value))
                            {
                                let tag = format!("ss{number:02}").into_bytes();
                                feature(
                                    tag.as_slice().try_into().expect("four-byte styleset tag"),
                                    1,
                                );
                            }
                        }
                        "character-variant" => {
                            if let Some(number @ 1..=99) = numbers.first().copied() {
                                let tag = format!("cv{number:02}").into_bytes();
                                feature(
                                    tag.as_slice()
                                        .try_into()
                                        .expect("four-byte character variant tag"),
                                    numbers.get(1).copied().unwrap_or(1),
                                );
                            }
                        }
                        "swash" if numbers.len() == 1 => feature(b"swsh", numbers[0]),
                        "ornaments" if numbers.len() == 1 => feature(b"ornm", numbers[0]),
                        "annotation" if numbers.len() == 1 => feature(b"nalt", numbers[0]),
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
}

fn round_css_integer(value: f32) -> f32 {
    (f64::from(value) + 0.5).floor() as f32
}

fn normalize_numeric_property(name: &str, value: f32) -> f32 {
    let value = if value.is_nan() {
        0.0
    } else {
        value.clamp(-f32::MAX, f32::MAX)
    };
    match name {
        "opacity" => value.clamp(0.0, 1.0),
        "flex-grow" | "flex-shrink" | "tab-size" => value.max(0.0),
        "order" | "z-index" => round_css_integer(value),
        "column-count" | "orphans" | "widows" => round_css_integer(value).max(1.0),
        _ => value,
    }
}

fn compute_standalone_numeric_math(
    value: &Value,
    name: &str,
    ctx: ResolutionContext,
) -> Option<ComputedValue> {
    let Value::Function { name: function, .. } = value else {
        return None;
    };
    if !matches!(
        function.to_ascii_lowercase().as_str(),
        "round"
            | "hypot"
            | "abs"
            | "sign"
            | "mod"
            | "rem"
            | "sin"
            | "cos"
            | "tan"
            | "asin"
            | "acos"
            | "atan"
            | "atan2"
            | "pow"
            | "sqrt"
            | "log"
            | "exp"
    ) {
        return None;
    }
    if let Some(length) = evaluate_length_percentage_math(value, ctx) {
        return Some(computed_length_percentage_math(length, name, ctx));
    }
    Some(
        match individual_transform::computed_numeric_expression(value, ctx)? {
            Value::Number(value) => ComputedValue::Number(normalize_numeric_property(name, value)),
            Value::Percentage(value) => ComputedValue::Percentage(value),
            Value::Length(value, unit) => ComputedValue::Keyword(format!("{value}{unit}")),
            _ => return None,
        },
    )
}

fn compute_font_size_adjust(value: &Value, ctx: ResolutionContext) -> Option<ComputedValue> {
    if matches!(value, Value::Keyword(keyword) if keyword.eq_ignore_ascii_case("none")) {
        return Some(ComputedValue::Keyword("none".to_string()));
    }
    let (metric, ratio) = parse_font_size_adjust(value)?;
    let ratio = match ratio {
        FontSizeAdjustRatio::FromFont => {
            let font_size = ctx.parent_font_size;
            if !font_size.is_finite() || font_size <= 0.0 {
                return None;
            }
            match metric {
                FontSizeAdjustMetric::ExHeight => ctx.font_metrics.ex / font_size,
                FontSizeAdjustMetric::CapHeight => ctx.font_metrics.cap / font_size,
                FontSizeAdjustMetric::ChWidth => ctx.font_metrics.ch / font_size,
                FontSizeAdjustMetric::IcWidth => ctx.font_metrics.ic / font_size,
                FontSizeAdjustMetric::IcHeight => 1.0,
            }
        }
        FontSizeAdjustRatio::Value(value) => {
            let LengthMathValue::Number(value) = evaluate_length_math_value(value, ctx)? else {
                return None;
            };
            value.max(0.0)
        }
    };
    let ratio = serialize_opentype_number(ratio);
    Some(ComputedValue::Keyword(match metric {
        FontSizeAdjustMetric::ExHeight => ratio,
        FontSizeAdjustMetric::CapHeight => format!("cap-height {ratio}"),
        FontSizeAdjustMetric::ChWidth => format!("ch-width {ratio}"),
        FontSizeAdjustMetric::IcWidth => format!("ic-width {ratio}"),
        FontSizeAdjustMetric::IcHeight => format!("ic-height {ratio}"),
    }))
}

fn compute_nine_slice_value(
    value: &Value,
    property_name: &str,
    ctx: ResolutionContext,
) -> Option<NineSliceComputedValue> {
    let values = match value {
        Value::List(values) => values.as_slice(),
        value => std::slice::from_ref(value),
    };
    if property_name.ends_with("-repeat") {
        let values = values
            .iter()
            .map(|value| match value {
                Value::Keyword(keyword) if keyword.eq_ignore_ascii_case("stretch") => {
                    Some(NineSliceRepeat::Stretch)
                }
                Value::Keyword(keyword) if keyword.eq_ignore_ascii_case("repeat") => {
                    Some(NineSliceRepeat::Repeat)
                }
                Value::Keyword(keyword) if keyword.eq_ignore_ascii_case("round") => {
                    Some(NineSliceRepeat::Round)
                }
                Value::Keyword(keyword) if keyword.eq_ignore_ascii_case("space") => {
                    Some(NineSliceRepeat::Space)
                }
                _ => None,
            })
            .collect::<Option<Vec<_>>>()?;
        return Some(NineSliceComputedValue::Repeat(values));
    }

    let mut fill = false;
    let values = values
        .iter()
        .filter_map(|value| match value {
            Value::Keyword(keyword)
                if property_name.ends_with("-slice") && keyword.eq_ignore_ascii_case("fill") =>
            {
                fill = true;
                None
            }
            Value::Keyword(keyword)
                if property_name.ends_with("-width") && keyword.eq_ignore_ascii_case("auto") =>
            {
                Some(Some(NineSliceLength::Auto))
            }
            Value::Number(value) => Some(Some(NineSliceLength::Number(*value))),
            Value::Percentage(value) => Some(Some(NineSliceLength::Percentage(*value))),
            Value::Length(value, unit) => {
                Some(resolve_length_to_px(*value, unit, ctx).map(NineSliceLength::Px))
            }
            Value::Function { name, .. } if is_length_percentage_math_function(name) => {
                Some(match evaluate_length_math_value(value, ctx)? {
                    LengthMathValue::Number(value) => Some(NineSliceLength::Number(value.max(0.0))),
                    LengthMathValue::Length(value) if value.is_pure_px() => {
                        Some(NineSliceLength::Px(value.resolve(0.0).max(0.0)))
                    }
                    LengthMathValue::Length(value) => Some(NineSliceLength::Math(value)),
                })
            }
            _ => Some(None),
        })
        .collect::<Option<Vec<_>>>()?;
    if property_name.ends_with("-slice") {
        Some(NineSliceComputedValue::Slice { values, fill })
    } else if property_name.ends_with("-width") {
        Some(NineSliceComputedValue::Width(values))
    } else {
        Some(NineSliceComputedValue::Outset(values))
    }
}

fn compute_value(value: &Value, property_name: &str, ctx: ResolutionContext) -> ComputedValue {
    if matches!(
        property_name,
        "counter-reset" | "counter-increment" | "counter-set"
    ) {
        let default = if property_name == "counter-increment" {
            1
        } else {
            0
        };
        if let Some(pairs) = counter_pairs_with_context(value, default, ctx) {
            return if pairs.is_empty() {
                ComputedValue::Keyword("none".to_string())
            } else {
                ComputedValue::Keyword(
                    pairs
                        .iter()
                        .map(|(name, value)| format!("{name} {value}"))
                        .collect::<Vec<_>>()
                        .join(" "),
                )
            };
        }
    }
    if property_name.eq_ignore_ascii_case("font-size")
        && matches!(value, Value::Number(number) if *number == 0.0)
    {
        return ComputedValue::Px(0.0);
    }
    // Percentages inherit as absolute lengths; unitless factors remain numbers.
    if property_name.eq_ignore_ascii_case("line-height") {
        if let Some(length) = evaluate_length_percentage_math(value, ctx) {
            return ComputedValue::Px(length.resolve(ctx.parent_font_size).max(0.0));
        }
    }
    if property_name.eq_ignore_ascii_case("transform") {
        return ComputedValue::Keyword(
            generic_transform::compute(value, ctx)
                .as_ref()
                .map(render_value)
                .unwrap_or_else(|| "none".into()),
        );
    }
    if property_name.eq_ignore_ascii_case("font-palette") {
        return ComputedValue::Keyword(font_palette::compute(value));
    }
    if property_name.eq_ignore_ascii_case("font-size-adjust")
        && let Some(value) = compute_font_size_adjust(value, ctx)
    {
        return value;
    }
    if property_name.eq_ignore_ascii_case("font-feature-settings") {
        if let Some(value) = compute_font_feature_settings(value, ctx) {
            return value;
        }
    }
    if property_name.eq_ignore_ascii_case("font-variation-settings") {
        if let Some(value) = compute_font_variation_settings(value, ctx) {
            return value;
        }
    }
    if property_name.eq_ignore_ascii_case("font-style") {
        if let Value::List(values) = value {
            if let [Value::Keyword(keyword), angle] = values.as_slice() {
                if keyword.eq_ignore_ascii_case("oblique") {
                    if let Some(angle) =
                        compute_registered_dimension(angle, &RegisteredSyntaxKind::Angle, ctx)
                    {
                        return ComputedValue::Keyword(format!("oblique {}", render_value(&angle)));
                    }
                }
            }
        }
    }
    if property_name.eq_ignore_ascii_case("word-spacing")
        && matches!(value, Value::Keyword(keyword) if keyword.eq_ignore_ascii_case("normal"))
    {
        return ComputedValue::Px(0.0);
    }
    if property_name.eq_ignore_ascii_case("text-indent")
        && let Some((amount, hanging, each_line)) = text_indent_components(value)
    {
        return ComputedValue::TextIndent {
            value: Box::new(compute_value(amount, "left", ctx)),
            hanging,
            each_line,
        };
    }
    if matches!(property_name, "translate" | "rotate" | "scale") {
        return ComputedValue::Keyword(individual_transform::compute(property_name, value, ctx));
    }
    if property_name == "text-shadow" {
        return text_shadow::compute(value, ctx)
            .unwrap_or_else(|| ComputedValue::Keyword(render_value(value)));
    }
    if matches!(
        property_name,
        "border-image-slice"
            | "border-image-width"
            | "border-image-outset"
            | "border-image-repeat"
            | "mask-border-slice"
            | "mask-border-width"
            | "mask-border-outset"
            | "mask-border-repeat"
    ) {
        if let Some(value) = compute_nine_slice_value(value, property_name, ctx) {
            return ComputedValue::NineSlice(value);
        }
    }
    if property_name.eq_ignore_ascii_case("content") {
        return match value {
            Value::String(value) => ComputedValue::String(value.clone()),
            Value::Keyword(value) => ComputedValue::Keyword(value.clone()),
            _ => ComputedValue::Keyword(render_content_value(value)),
        };
    }
    if matches!(
        property_name,
        "border-width"
            | "border-top-width"
            | "border-right-width"
            | "border-bottom-width"
            | "border-left-width"
            | "border-inline-start-width"
            | "border-inline-end-width"
            | "border-block-start-width"
            | "border-block-end-width"
            | "column-rule-width"
            | "outline-width"
    ) && let Value::Keyword(keyword) = value
    {
        let pixels = match keyword.to_ascii_lowercase().as_str() {
            "thin" => Some(1.0),
            "medium" => Some(3.0),
            "thick" => Some(5.0),
            _ => None,
        };
        if let Some(pixels) = pixels {
            return ComputedValue::Px(pixels);
        }
    }
    if property_name.eq_ignore_ascii_case("animation-duration")
        || property_name.eq_ignore_ascii_case("animation-delay")
    {
        return resolve_time_seconds(value)
            .map(ComputedValue::Number)
            .unwrap_or_else(|| ComputedValue::Keyword(render_value(value)));
    }
    if let Some(computed) = compute_standalone_numeric_math(value, property_name, ctx) {
        return computed;
    }
    if property_name.eq_ignore_ascii_case("shape-outside") {
        return ComputedValue::Keyword(render_shape_outside_value(value, ctx));
    }
    if property_name.eq_ignore_ascii_case("clip-path") {
        return ComputedValue::Keyword(render_clip_path_value(value, ctx));
    }
    if property_name.eq_ignore_ascii_case("object-position") {
        if matches!(value, Value::Keyword(keyword) if is_css_wide_keyword(&keyword.to_ascii_lowercase()))
        {
            return ComputedValue::Keyword(render_value(value));
        }
        if let Some((x, y)) = object_position_components(value)
            && let (Some(x), Some(y)) = (
                compute_object_position_component(&x, ctx),
                compute_object_position_component(&y, ctx),
            )
        {
            return ComputedValue::Position {
                x: Box::new(x),
                y: Box::new(y),
            };
        }
        return ComputedValue::Keyword(render_value(value));
    }
    if property_name.eq_ignore_ascii_case("aspect-ratio") {
        return ComputedValue::Keyword(render_aspect_ratio_value(value, ctx));
    }
    if property_name.eq_ignore_ascii_case("overflow-clip-margin") {
        let components = match value {
            Value::List(values) => values.as_slice(),
            value => std::slice::from_ref(value),
        };
        let mut visual_box = "padding-box";
        let mut length = 0.0;
        for component in components {
            match component {
                Value::Keyword(keyword)
                    if matches!(
                        keyword.to_ascii_lowercase().as_str(),
                        "content-box" | "padding-box" | "border-box"
                    ) =>
                {
                    visual_box = match keyword.to_ascii_lowercase().as_str() {
                        "content-box" => "content-box",
                        "border-box" => "border-box",
                        _ => "padding-box",
                    };
                }
                Value::Length(number, unit) => {
                    length = resolve_length_to_px(*number, unit, ctx).unwrap_or(0.0);
                }
                Value::Number(number) if *number == 0.0 => length = 0.0,
                Value::Function { .. } => {
                    length = evaluate_length_percentage_math(component, ctx)
                        .map(|value| value.resolve(0.0))
                        .unwrap_or(0.0);
                }
                _ => {}
            }
        }
        let length = format!("{}px", super::tokenizer::trimmed_number(length));
        let rendered = match (visual_box, length.as_str()) {
            ("padding-box", length) => length.to_string(),
            (visual_box, "0px") => visual_box.to_string(),
            (visual_box, length) => format!("{visual_box} {length}"),
        };
        return ComputedValue::Keyword(rendered);
    }
    if property_name.starts_with("contain-intrinsic-") {
        return compute_contain_intrinsic_value(value, ctx);
    }
    if property_name.eq_ignore_ascii_case("grid-template-areas") {
        return ComputedValue::Keyword(render_grid_template_areas(value));
    }
    if property_name.eq_ignore_ascii_case("grid-auto-columns")
        || property_name.eq_ignore_ascii_case("grid-auto-rows")
    {
        return ComputedValue::Keyword(grid_properties::compute(value, ctx));
    }
    if property_name.eq_ignore_ascii_case("grid-template-columns")
        || property_name.eq_ignore_ascii_case("grid-template-rows")
    {
        return ComputedValue::Keyword(render_grid_track_value(value, ctx));
    }
    if property_name.eq_ignore_ascii_case("grid-column-start")
        || property_name.eq_ignore_ascii_case("grid-column-end")
        || property_name.eq_ignore_ascii_case("grid-row-start")
        || property_name.eq_ignore_ascii_case("grid-row-end")
    {
        return ComputedValue::Keyword(render_value(value));
    }
    match value {
        Value::Keyword(keyword) => {
            // CSS-wide keywords must remain as Keyword for inherit/initial resolution.
            let lower = keyword.to_ascii_lowercase();
            if matches!(lower.as_str(), "inherit" | "initial" | "unset" | "revert") {
                ComputedValue::Keyword(keyword.clone())
            } else if is_color_keyword(keyword)
                || property_name.ends_with("color")
                || property_name == "color"
            {
                ComputedValue::Color(keyword.clone())
            } else {
                ComputedValue::Keyword(keyword.clone())
            }
        }
        Value::Length(number, unit) => {
            let px = resolve_length_to_px(*number, unit, ctx).unwrap_or(*number);
            ComputedValue::Px(px)
        }
        Value::Percentage(percent) => {
            if property_name == "font-size" {
                let px = ctx.parent_font_size * (*percent / 100.0);
                ComputedValue::Px(px)
            } else {
                ComputedValue::Percentage(*percent)
            }
        }
        Value::Color(color) => ComputedValue::Color(color.clone()),
        Value::String(value) => ComputedValue::String(value.clone()),
        Value::Number(value) => {
            ComputedValue::Number(normalize_numeric_property(property_name, *value))
        }
        Value::TreeCounting { .. } => ComputedValue::Keyword(render_value(value)),
        Value::Function { name, .. }
            if matches!(
                name.to_ascii_lowercase().as_str(),
                "hwb" | "lab" | "lch" | "oklab" | "oklch" | "color"
            ) =>
        {
            compute_color4_value(value, ctx)
        }
        Value::Function { name, arguments }
            if name.eq_ignore_ascii_case("rgb") || name.eq_ignore_ascii_case("rgba") =>
        {
            if let Some(hex) = compute_rgb_function(arguments) {
                ComputedValue::Color(hex)
            } else {
                ComputedValue::Keyword(render_value(value))
            }
        }
        Value::Function { name, arguments }
            if name.eq_ignore_ascii_case("hsl") || name.eq_ignore_ascii_case("hsla") =>
        {
            if let Some(hex) = compute_hsl_function(arguments) {
                ComputedValue::Color(hex)
            } else {
                ComputedValue::Keyword(render_value(value))
            }
        }
        Value::Function { name, arguments } if name.eq_ignore_ascii_case("calc") => {
            if let Some(value) = evaluate_length_percentage_math(value, ctx) {
                return computed_length_percentage_math(value, property_name, ctx);
            }
            if let Some(quantity) = evaluate_calc(arguments, ctx) {
                let value = if is_non_negative_sizing_property(property_name)
                    && quantity.unit != CalcUnit::Unitless
                {
                    quantity.value.max(0.0)
                } else {
                    quantity.value
                };
                return match quantity.unit {
                    CalcUnit::Px => ComputedValue::Px(value),
                    CalcUnit::Percentage => {
                        if property_name == "font-size" {
                            ComputedValue::Px(ctx.parent_font_size * (value / 100.0))
                        } else {
                            ComputedValue::Percentage(value)
                        }
                    }
                    CalcUnit::Unitless => {
                        ComputedValue::Number(normalize_numeric_property(property_name, value))
                    }
                };
            }
            ComputedValue::Keyword(render_value(value))
        }
        Value::Function { name, .. }
            if name.eq_ignore_ascii_case("min") || name.eq_ignore_ascii_case("max") =>
        {
            compute_math_comparison(value, property_name, ctx)
                .unwrap_or_else(|| ComputedValue::Keyword(render_value(value)))
        }
        Value::Function { name, arguments } if name.eq_ignore_ascii_case("clamp") => {
            if let Some(value) = evaluate_length_percentage_math(value, ctx) {
                computed_length_percentage_math(value, property_name, ctx)
            } else {
                compute_clamp_function(arguments, property_name, ctx)
                    .map(|computed| clamp_sizing_computed_value(property_name, computed))
                    .unwrap_or_else(|| ComputedValue::Keyword(render_value(value)))
            }
        }
        Value::Function { .. } => ComputedValue::Keyword(render_value(value)),
        Value::List(values) => {
            if property_name.eq_ignore_ascii_case("transform")
                || property_name.eq_ignore_ascii_case("transform-origin")
                || property_name.eq_ignore_ascii_case("perspective-origin")
                || property_name.eq_ignore_ascii_case("overflow")
                || property_name.eq_ignore_ascii_case("box-shadow")
                || property_name.eq_ignore_ascii_case("background-size")
                || property_name.eq_ignore_ascii_case("background-repeat")
                || property_name.eq_ignore_ascii_case("mask-size")
                || property_name.eq_ignore_ascii_case("border-spacing")
                || property_name.eq_ignore_ascii_case("font-style")
            {
                return ComputedValue::Keyword(render_value(value));
            }
            if property_name.eq_ignore_ascii_case("font-family") {
                return ComputedValue::Keyword(render_font_family_value(values));
            }
            if let Some(first) = values.first() {
                compute_value(first, property_name, ctx)
            } else {
                ComputedValue::Keyword(String::new())
            }
        }
        Value::CommaList(values) if property_name.starts_with("background-") => {
            let rendered = values
                .iter()
                .map(|value| compute_background_layer_value(value, property_name, ctx))
                .collect::<Vec<_>>()
                .join(", ");
            ComputedValue::Keyword(rendered)
        }
        Value::CommaList(_) => ComputedValue::Keyword(render_value(value)),
    }
}

fn compute_contain_intrinsic_value(value: &Value, ctx: ResolutionContext) -> ComputedValue {
    let values = match value {
        Value::List(values) => values.as_slice(),
        value => std::slice::from_ref(value),
    };
    let compute_fallback = |value: &Value| match value {
        Value::Keyword(keyword) if keyword.eq_ignore_ascii_case("none") => {
            ComputedValue::Keyword("none".to_string())
        }
        Value::Number(number) if *number == 0.0 => ComputedValue::Px(0.0),
        _ => compute_value(value, "width", ctx),
    };
    match values {
        [fallback] => compute_fallback(fallback),
        [Value::Keyword(auto), fallback] if auto.eq_ignore_ascii_case("auto") => {
            let fallback = compute_fallback(fallback);
            ComputedValue::Keyword(format!("auto {}", computed_value_css_text(&fallback)))
        }
        _ => ComputedValue::Keyword("none".to_string()),
    }
}

fn compute_background_layer_value(
    value: &Value,
    property_name: &str,
    ctx: ResolutionContext,
) -> String {
    if let Value::List(values) = value {
        return values
            .iter()
            .map(|value| computed_value_css_text(&compute_value(value, property_name, ctx)))
            .collect::<Vec<_>>()
            .join(" ");
    }
    computed_value_css_text(&compute_value(value, property_name, ctx))
}

fn render_content_value(value: &Value) -> String {
    match value {
        Value::String(value) => {
            let escaped = value.replace('\\', "\\\\").replace('"', "\\\"");
            format!("\"{escaped}\"")
        }
        Value::Function { name, arguments } => format!(
            "{name}({})",
            arguments
                .iter()
                .map(render_content_value)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Value::List(values) => values
            .iter()
            .map(render_content_value)
            .collect::<Vec<_>>()
            .join(" "),
        Value::CommaList(values) => values
            .iter()
            .map(render_content_value)
            .collect::<Vec<_>>()
            .join(", "),
        _ => render_value(value),
    }
}

pub(crate) fn counter_pairs(value: &Value, default: i32) -> Option<Vec<(String, i32)>> {
    counter_pairs_with_context(value, default, ResolutionContext::default())
}

fn counter_pairs_with_context(
    value: &Value,
    default: i32,
    ctx: ResolutionContext,
) -> Option<Vec<(String, i32)>> {
    if matches!(value, Value::Keyword(keyword) if keyword.eq_ignore_ascii_case("none")) {
        return Some(Vec::new());
    }
    let values = match value {
        Value::List(values) => values.as_slice(),
        value => std::slice::from_ref(value),
    };
    let mut result = Vec::new();
    let mut index = 0usize;
    while index < values.len() {
        let Value::Keyword(name) = &values[index] else {
            return None;
        };
        let lower = name.to_ascii_lowercase();
        if is_css_wide_keyword(&lower) || matches!(lower.as_str(), "none" | "default") {
            return None;
        }
        index += 1;
        let amount = match values
            .get(index)
            .and_then(|value| counter_integer_with_context(value, ctx))
        {
            Some(amount) => {
                index += 1;
                amount
            }
            None => default,
        };
        result.push((name.clone(), amount));
    }
    Some(result)
}

fn counter_value_contains_calc(value: &Value) -> bool {
    match value {
        Value::Function { name, .. } => name.eq_ignore_ascii_case("calc"),
        Value::List(values) => values.iter().any(counter_value_contains_calc),
        _ => false,
    }
}

fn counter_pair_syntax_is_valid(value: &Value) -> bool {
    let values = match value {
        Value::List(values) => values.as_slice(),
        value => std::slice::from_ref(value),
    };
    let mut index = 0;
    while index < values.len() {
        let Some(Value::Keyword(name)) = values.get(index) else {
            return false;
        };
        let lower = name.to_ascii_lowercase();
        if is_css_wide_keyword(&lower) || matches!(lower.as_str(), "none" | "default") {
            return false;
        }
        index += 1;
        match values.get(index) {
            Some(Value::Number(number)) if number.is_finite() && number.fract() == 0.0 => {
                index += 1
            }
            Some(value @ Value::Function { name, .. }) if name.eq_ignore_ascii_case("calc") => {
                if counter_integer_with_context(value, ResolutionContext::default()).is_none() {
                    return false;
                }
                index += 1;
            }
            Some(Value::Number(_)) | Some(Value::Function { .. }) => return false,
            _ => {}
        }
    }
    true
}

fn counter_integer_with_context(value: &Value, ctx: ResolutionContext) -> Option<i32> {
    let number = match value {
        Value::Number(number) if number.is_finite() && number.fract() == 0.0 => *number,
        Value::Function { name, .. } if name.eq_ignore_ascii_case("calc") => {
            let LengthMathValue::Number(number) = evaluate_length_math_value(value, ctx)? else {
                return None;
            };
            round_css_integer(number)
        }
        _ => return None,
    };
    (number >= i32::MIN as f32 && number <= i32::MAX as f32).then_some(number as i32)
}

fn clamp_sizing_computed_value(property_name: &str, value: ComputedValue) -> ComputedValue {
    if !is_non_negative_sizing_property(property_name) {
        return value;
    }
    match value {
        ComputedValue::Px(number) => ComputedValue::Px(number.max(0.0)),
        ComputedValue::Percentage(number) => ComputedValue::Percentage(number.max(0.0)),
        other => other,
    }
}

fn compute_clamp_function(
    arguments: &[Value],
    property_name: &str,
    ctx: ResolutionContext,
) -> Option<ComputedValue> {
    let [minimum, preferred, maximum] = arguments else {
        return None;
    };
    let minimum = resolve_clamp_quantity(minimum, property_name, ctx)?;
    let preferred = resolve_clamp_quantity(preferred, property_name, ctx)?;
    let maximum = resolve_clamp_quantity(maximum, property_name, ctx)?;
    if minimum.unit != preferred.unit || preferred.unit != maximum.unit {
        return None;
    }

    // CSS Values 4 defines clamp(MIN, VAL, MAX) as max(MIN, min(VAL, MAX)).
    let value = preferred.value.min(maximum.value).max(minimum.value);
    Some(match minimum.unit {
        CalcUnit::Px => ComputedValue::Px(value),
        CalcUnit::Percentage => ComputedValue::Percentage(value),
        CalcUnit::Unitless => ComputedValue::Number(value),
    })
}

fn resolve_clamp_quantity(
    value: &Value,
    property_name: &str,
    ctx: ResolutionContext,
) -> Option<CalcQuantity> {
    match value {
        Value::Length(number, unit) => Some(CalcQuantity {
            value: resolve_length_to_px(*number, unit, ctx)?,
            unit: CalcUnit::Px,
        }),
        Value::Percentage(number) if property_name.eq_ignore_ascii_case("font-size") => {
            Some(CalcQuantity {
                value: ctx.parent_font_size * (*number / 100.0),
                unit: CalcUnit::Px,
            })
        }
        Value::Percentage(number) => Some(CalcQuantity {
            value: *number,
            unit: CalcUnit::Percentage,
        }),
        Value::Number(number) => Some(CalcQuantity {
            value: *number,
            unit: CalcUnit::Unitless,
        }),
        Value::Function { name, arguments } if name.eq_ignore_ascii_case("calc") => {
            let mut quantity = evaluate_calc(arguments, ctx)?;
            if property_name.eq_ignore_ascii_case("font-size")
                && quantity.unit == CalcUnit::Percentage
            {
                quantity.value = ctx.parent_font_size * (quantity.value / 100.0);
                quantity.unit = CalcUnit::Px;
            }
            Some(quantity)
        }
        _ => None,
    }
}

fn is_prefixed_property_alias(name: &str) -> bool {
    !canonical_property_name(name).eq_ignore_ascii_case(name)
}

fn canonical_property_name(name: &str) -> &str {
    if name.eq_ignore_ascii_case("-webkit-align-items")
        || name.eq_ignore_ascii_case("-ms-flex-align")
        || name.eq_ignore_ascii_case("-webkit-box-align")
    {
        "align-items"
    } else if name.eq_ignore_ascii_case("-webkit-justify-content")
        || name.eq_ignore_ascii_case("-ms-flex-pack")
        || name.eq_ignore_ascii_case("-webkit-box-pack")
    {
        "justify-content"
    } else if name.eq_ignore_ascii_case("-webkit-flex-shrink")
        || name.eq_ignore_ascii_case("-ms-flex-negative")
    {
        "flex-shrink"
    } else if name.eq_ignore_ascii_case("-webkit-flex-grow") {
        "flex-grow"
    } else if name.eq_ignore_ascii_case("-webkit-flex-direction") {
        "flex-direction"
    } else if name.eq_ignore_ascii_case("-webkit-flex-wrap") {
        "flex-wrap"
    } else if name.eq_ignore_ascii_case("-webkit-clip-path") {
        "clip-path"
    } else if name.eq_ignore_ascii_case("-webkit-transform") {
        "transform"
    } else if name.eq_ignore_ascii_case("-webkit-mask") {
        "mask"
    } else if name.eq_ignore_ascii_case("-webkit-mask-image") {
        "mask-image"
    } else if name.eq_ignore_ascii_case("-webkit-mask-position") {
        "mask-position"
    } else if name.eq_ignore_ascii_case("-webkit-mask-position-x") {
        "mask-position-x"
    } else if name.eq_ignore_ascii_case("-webkit-mask-position-y") {
        "mask-position-y"
    } else if name.eq_ignore_ascii_case("-webkit-mask-repeat") {
        "mask-repeat"
    } else if name.eq_ignore_ascii_case("-webkit-mask-size") {
        "mask-size"
    } else if name.eq_ignore_ascii_case("-webkit-mask-mode") {
        "mask-mode"
    } else if name.eq_ignore_ascii_case("-webkit-mask-composite") {
        "mask-composite"
    } else {
        name
    }
}

/// One `object-position` component: a keyword naming an edge or the centre, or a
/// `<length-percentage>` offset.
#[derive(Debug, Clone, Copy, PartialEq)]
enum PositionAxis {
    /// The component may only name a horizontal edge.
    Horizontal,
    /// The component may only name a vertical edge.
    Vertical,
    /// `center` or a length-percentage, which fits either axis.
    Either,
}

/// Splits an `object-position` value into its `(x, y)` components, or `None`
/// when the value does not match `<position>`'s one- and two-value forms.
///
/// A single component centres the other axis. In the two-value form the axes are
/// assigned by the keywords, so `top center` names y first and comes back as
/// `(center, top)`; two components that name the same axis (`left right`) or
/// three or more components are rejected. The three- and four-value forms with
/// edge offsets (`left 10px top 20px`) are not supported yet.
fn object_position_components(value: &Value) -> Option<(Value, Value)> {
    let center = || Value::Keyword("center".to_string());
    let components: Vec<&Value> = match value {
        Value::List(values) => values.iter().collect(),
        single => vec![single],
    };
    let axes: Vec<PositionAxis> = components
        .iter()
        .map(|component| object_position_axis(component))
        .collect::<Option<Vec<_>>>()?;
    match components.as_slice() {
        [single] => {
            // A lone vertical keyword sets y; everything else sets x.
            if axes[0] == PositionAxis::Vertical {
                Some((center(), (*single).clone()))
            } else {
                Some(((*single).clone(), center()))
            }
        }
        [first, second] => match (axes[0], axes[1]) {
            (PositionAxis::Vertical, PositionAxis::Vertical)
            | (PositionAxis::Horizontal, PositionAxis::Horizontal) => None,
            (PositionAxis::Vertical, _) => Some(((*second).clone(), (*first).clone())),
            (_, PositionAxis::Horizontal) => Some(((*second).clone(), (*first).clone())),
            _ => Some(((*first).clone(), (*second).clone())),
        },
        _ => None,
    }
}

/// Returns which axis a single `object-position` component can name, or `None`
/// when it is not a valid component.
fn object_position_axis(value: &Value) -> Option<PositionAxis> {
    match value {
        Value::Keyword(keyword) => match keyword.to_ascii_lowercase().as_str() {
            "left" | "right" => Some(PositionAxis::Horizontal),
            "top" | "bottom" => Some(PositionAxis::Vertical),
            "center" => Some(PositionAxis::Either),
            _ => None,
        },
        Value::Percentage(_) | Value::Length(..) => Some(PositionAxis::Either),
        // A bare `0` is a length; other bare numbers are not valid offsets.
        Value::Number(number) if *number == 0.0 => Some(PositionAxis::Either),
        // A math function must compute to a typed length-percentage; bare
        // numeric `calc(1)` and `calc(0)` remain invalid offsets.
        Value::Function { name, .. } if is_length_percentage_math_function(name) => {
            evaluate_length_percentage_math(value, ResolutionContext::default())
                .map(|_| PositionAxis::Either)
        }
        _ => None,
    }
}

/// One component of an `aspect-ratio` value.
#[derive(Debug, Clone, PartialEq)]
enum AspectRatioPart {
    Auto,
    Slash,
    Number(Value),
}

/// Splits an `aspect-ratio` value into `auto` / `<ratio>` components, or `None`
/// when it does not match `auto || <ratio>`.
///
/// The tokenizer glues `1/1` into one keyword but keeps `2 / 1` as three
/// components, so both shapes are flattened into the same part list before the
/// grammar is checked. Numbers must be non-negative; a degenerate ratio such as
/// `0 / 1` is a valid value that layout then ignores.
fn aspect_ratio_parts(value: &Value) -> Option<(bool, Option<(Value, Value)>)> {
    let components: Vec<&Value> = match value {
        Value::List(values) => values.iter().collect(),
        single => vec![single],
    };
    let mut parts = Vec::new();
    for component in components {
        match component {
            Value::Keyword(keyword) if keyword.eq_ignore_ascii_case("auto") => {
                parts.push(AspectRatioPart::Auto)
            }
            Value::Keyword(keyword) if keyword == "/" => parts.push(AspectRatioPart::Slash),
            // A glued `1/1`, or a number the tokenizer kept as a keyword.
            Value::Keyword(keyword) => {
                let mut pieces = keyword.split('/');
                let first = pieces.next()?;
                parts.push(AspectRatioPart::Number(Value::Number(non_negative_number(
                    first,
                )?)));
                for piece in pieces {
                    parts.push(AspectRatioPart::Slash);
                    parts.push(AspectRatioPart::Number(Value::Number(non_negative_number(
                        piece,
                    )?)));
                }
            }
            // A literal negative number is invalid; an overflowing one is
            // clamped when the value is computed.
            Value::Number(number) if *number >= 0.0 => {
                parts.push(AspectRatioPart::Number(component.clone()))
            }
            // A `calc()` is a ratio component when it evaluates to a bare
            // number. Out-of-range results are clamped rather than invalid (CSS
            // Values 4), so the sign is not checked here.
            Value::Function { name, arguments }
                if name.eq_ignore_ascii_case("calc")
                    && calc_unitless_number(arguments).is_some() =>
            {
                parts.push(AspectRatioPart::Number(component.clone()))
            }
            _ => return None,
        }
    }

    let ratio_from = |parts: &[AspectRatioPart]| -> Option<Option<(Value, Value)>> {
        match parts {
            [] => Some(None),
            [AspectRatioPart::Number(width)] => Some(Some((width.clone(), Value::Number(1.0)))),
            [
                AspectRatioPart::Number(width),
                AspectRatioPart::Slash,
                AspectRatioPart::Number(height),
            ] => Some(Some((width.clone(), height.clone()))),
            _ => None,
        }
    };
    match parts.first() {
        Some(AspectRatioPart::Auto) => Some((true, ratio_from(&parts[1..])?)),
        _ => match parts.last() {
            Some(AspectRatioPart::Auto) => Some((true, ratio_from(&parts[..parts.len() - 1])?)),
            _ => {
                let ratio = ratio_from(&parts)?;
                // A bare `auto` is the only value with neither part.
                ratio.map(|ratio| (false, Some(ratio)))
            }
        },
    }
}

fn non_negative_number(text: &str) -> Option<f32> {
    let number = text.trim().parse::<f32>().ok()?;
    (number.is_finite() && number >= 0.0).then_some(number)
}

/// Evaluates a `calc()` that must produce a bare number, for grammar checks that
/// run before a resolution context exists.
///
/// A length or percentage anywhere in the expression makes the result carry that
/// unit, which is rejected here, so the placeholder context cannot change the
/// outcome: only the unit decides, and units do not depend on it.
fn calc_unitless_number(arguments: &[Value]) -> Option<f32> {
    let placeholder = ResolutionContext {
        parent_font_size: 16.0,
        root_font_size: 16.0,
        line_height: 19.2,
        root_line_height: 19.2,
        font_metrics: CssRelativeFontMetrics::fallback(16.0, false),
        viewport_width: 0.0,
        viewport_height: 0.0,
        container_size: [None, None],
        vertical_writing: false,
    };
    match evaluate_calc(arguments, placeholder) {
        Some(quantity) if quantity.unit == CalcUnit::Unitless => Some(quantity.value),
        _ => None,
    }
}

/// Clamps a ratio component into the `<number [0,∞]>` range the grammar allows.
///
/// A `calc()` resolving out of range is clamped rather than dropped, and a
/// literal that overflows the float range saturates the same way. Firefox 152
/// reports `calc(-1)` as `0`, and both `1e40` and `calc(1/0)` as the largest
/// float.
fn clamp_ratio_number(value: f32) -> f32 {
    if value.is_nan() {
        0.0
    } else {
        value.clamp(0.0, f32::MAX)
    }
}

/// Renders `aspect-ratio` the way getComputedStyle reports it: `auto`, a
/// `W / H` ratio, or `auto W / H` with `auto` first whichever order it was
/// written in (Firefox 152).
fn render_aspect_ratio_value(value: &Value, ctx: ResolutionContext) -> String {
    if let Value::Keyword(keyword) = value
        && is_css_wide_keyword(&keyword.to_ascii_lowercase())
    {
        return keyword.clone();
    }
    let Some((auto, ratio)) = aspect_ratio_parts(value) else {
        return render_value(value);
    };
    let ratio = ratio.and_then(|(width, height)| {
        Some(format!(
            "{} / {}",
            aspect_ratio_number(&width, ctx)?,
            aspect_ratio_number(&height, ctx)?
        ))
    });
    match (auto, ratio) {
        (true, Some(ratio)) => format!("auto {ratio}"),
        (true, None) => "auto".to_string(),
        (false, Some(ratio)) => ratio,
        // Neither part is not a value the grammar accepts.
        (false, None) => "auto".to_string(),
    }
}

fn aspect_ratio_number(value: &Value, ctx: ResolutionContext) -> Option<f32> {
    match value {
        Value::Number(number) => Some(clamp_ratio_number(*number)),
        Value::Function { name, arguments } if name.eq_ignore_ascii_case("calc") => {
            match evaluate_calc(arguments, ctx) {
                Some(quantity) if quantity.unit == CalcUnit::Unitless => {
                    Some(clamp_ratio_number(quantity.value))
                }
                _ => None,
            }
        }
        _ => None,
    }
}

/// Computes one already axis-assigned `object-position` component. Edge
/// keywords become percentages; typed math stays symbolic until paint knows
/// the free space on that axis.
fn compute_object_position_component(
    value: &Value,
    ctx: ResolutionContext,
) -> Option<ComputedValue> {
    match value {
        Value::Keyword(keyword) => match keyword.to_ascii_lowercase().as_str() {
            "left" | "top" => Some(ComputedValue::Percentage(0.0)),
            "right" | "bottom" => Some(ComputedValue::Percentage(100.0)),
            "center" => Some(ComputedValue::Percentage(50.0)),
            _ => None,
        },
        Value::Percentage(percentage) => Some(ComputedValue::Percentage(*percentage)),
        Value::Number(number) if *number == 0.0 => Some(ComputedValue::Px(0.0)),
        Value::Length(number, unit) => {
            resolve_length_to_px(*number, unit, ctx).map(ComputedValue::Px)
        }
        Value::Function { name, .. } if is_length_percentage_math_function(name) => {
            let computed = compute_value(value, "object-position-component", ctx);
            matches!(
                computed,
                ComputedValue::Px(_)
                    | ComputedValue::Percentage(_)
                    | ComputedValue::LengthPercentage(_)
            )
            .then_some(computed)
        }
        _ => None,
    }
}

fn render_clip_path_value(value: &Value, ctx: ResolutionContext) -> String {
    match value {
        Value::Length(number, unit) => resolve_length_to_px(*number, unit, ctx)
            .map(|px| format!("{px}px"))
            .unwrap_or_else(|| format!("{number}{unit}")),
        Value::Function { name, arguments } if is_length_percentage_math_function(name) => {
            if let Some(value) = evaluate_length_percentage_math(value, ctx) {
                return value.css_text();
            }
            if name.eq_ignore_ascii_case("calc")
                && let Some(quantity) = evaluate_calc(arguments, ctx)
            {
                return match quantity.unit {
                    CalcUnit::Px => format!("{}px", quantity.value),
                    CalcUnit::Percentage => format!("{}%", quantity.value),
                    CalcUnit::Unitless => quantity.value.to_string(),
                };
            }
            render_value(value)
        }
        Value::Function { name, arguments }
            if matches!(
                name.to_ascii_lowercase().as_str(),
                "inset" | "circle" | "ellipse" | "polygon"
            ) =>
        {
            let canonical_name = name.to_ascii_lowercase();
            format!(
                "{}({})",
                canonical_name,
                arguments
                    .iter()
                    .map(|argument| render_clip_path_value(argument, ctx))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }
        Value::List(values) => values
            .iter()
            .map(|value| render_clip_path_value(value, ctx))
            .collect::<Vec<_>>()
            .join(" "),
        Value::CommaList(values) => values
            .iter()
            .map(render_value)
            .collect::<Vec<_>>()
            .join(", "),
        _ => render_value(value),
    }
}

fn render_shape_outside_value(value: &Value, ctx: ResolutionContext) -> String {
    let Value::List(values) = value else {
        return render_clip_path_value(value, ctx);
    };
    let shape = values
        .iter()
        .find(|value| matches!(value, Value::Function { .. }));
    let reference_box = values.iter().find(|value| {
        matches!(value, Value::Keyword(keyword) if matches!(keyword.to_ascii_lowercase().as_str(), "margin-box" | "border-box" | "padding-box" | "content-box"))
    });
    let reference_box = reference_box.filter(|value| {
        shape.is_none()
            || !matches!(value, Value::Keyword(keyword) if keyword.eq_ignore_ascii_case("margin-box"))
    });
    [shape, reference_box]
        .into_iter()
        .flatten()
        .map(|value| render_clip_path_value(value, ctx))
        .collect::<Vec<_>>()
        .join(" ")
}

fn render_grid_template_areas(value: &Value) -> String {
    fn quoted(row: &str) -> String {
        format!("\"{}\"", row.replace('\\', "\\\\").replace('"', "\\\""))
    }
    match value {
        Value::String(row) => quoted(row),
        Value::List(rows) => rows
            .iter()
            .map(|row| match row {
                Value::String(row) => quoted(row),
                value => render_value(value),
            })
            .collect::<Vec<_>>()
            .join(" "),
        value => render_value(value),
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum CalcUnit {
    Px,
    Percentage,
    Unitless,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct CalcQuantity {
    value: f32,
    unit: CalcUnit,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum CalcToken {
    Value(CalcQuantity),
    Operator(char),
}

#[derive(Debug, Clone, PartialEq)]
enum LengthMathValue {
    Length(LengthPercentageMath),
    Number(f32),
}

#[derive(Debug, Clone, PartialEq)]
enum LengthMathToken {
    Value(LengthMathValue),
    Operator(char),
}

fn evaluate_length_percentage_math(
    value: &Value,
    ctx: ResolutionContext,
) -> Option<LengthPercentageMath> {
    let value = evaluate_length_math_value(value, ctx)?;
    match value {
        LengthMathValue::Length(value) => Some(value),
        LengthMathValue::Number(_) => None,
    }
}

fn evaluate_length_math_value(value: &Value, ctx: ResolutionContext) -> Option<LengthMathValue> {
    match value {
        Value::Length(number, unit) => {
            Some(LengthMathValue::Length(LengthPercentageMath::Linear {
                px: resolve_length_to_px(*number, unit, ctx)?,
                percentage: 0.0,
            }))
        }
        Value::Percentage(percentage) => {
            Some(LengthMathValue::Length(LengthPercentageMath::Linear {
                px: 0.0,
                percentage: *percentage,
            }))
        }
        Value::Number(number) if number.is_finite() => Some(LengthMathValue::Number(*number)),
        Value::List(_) => evaluate_length_math_expression(value, ctx),
        Value::Function { name, arguments } if name.eq_ignore_ascii_case("calc") => {
            let expression = if let [expression] = arguments.as_slice() {
                expression.clone()
            } else {
                Value::List(arguments.clone())
            };
            evaluate_length_math_expression(&expression, ctx)
                .or_else(|| scalar_length_math_value(value, ctx))
        }
        Value::Function { name, arguments }
            if name.eq_ignore_ascii_case("min") || name.eq_ignore_ascii_case("max") =>
        {
            if arguments.is_empty() {
                return None;
            }
            let values = arguments
                .iter()
                .map(|argument| evaluate_length_math_value(argument, ctx))
                .collect::<Option<Vec<_>>>()?;
            compare_math_values(values, name.eq_ignore_ascii_case("min"))
        }
        Value::Function { name, arguments } if name.eq_ignore_ascii_case("clamp") => {
            let [minimum, preferred, maximum] = arguments.as_slice() else {
                return None;
            };
            clamp_math_values(
                evaluate_length_math_value(minimum, ctx)?,
                evaluate_length_math_value(preferred, ctx)?,
                evaluate_length_math_value(maximum, ctx)?,
            )
        }
        Value::Function { name, arguments }
            if matches!(
                name.to_ascii_lowercase().as_str(),
                "round" | "hypot" | "abs" | "mod" | "rem"
            ) =>
        {
            scalar_length_math_value(value, ctx)
                .or_else(|| evaluate_deferred_length_function(name, arguments, ctx))
        }
        _ => scalar_length_math_value(value, ctx),
    }
}

/// Keeps percentages symbolic until a consuming property supplies its layout basis.
fn evaluate_deferred_length_function(
    name: &str,
    arguments: &[Value],
    ctx: ResolutionContext,
) -> Option<LengthMathValue> {
    let (strategy, arguments) = if name.eq_ignore_ascii_case("round") {
        match arguments {
            [Value::Keyword(strategy), _, _] => {
                let strategy = match strategy.to_ascii_lowercase().as_str() {
                    "nearest" => MathRoundingStrategy::Nearest,
                    "up" => MathRoundingStrategy::Up,
                    "down" => MathRoundingStrategy::Down,
                    "to-zero" => MathRoundingStrategy::ToZero,
                    _ => return None,
                };
                (strategy, &arguments[1..])
            }
            [_, _] => (MathRoundingStrategy::Nearest, arguments),
            _ => return None,
        }
    } else {
        (MathRoundingStrategy::Nearest, arguments)
    };
    let values = arguments
        .iter()
        .map(|value| match evaluate_length_math_value(value, ctx)? {
            LengthMathValue::Length(value) => Some(value),
            _ => None,
        })
        .collect::<Option<Vec<_>>>()?;
    let value = match name.to_ascii_lowercase().as_str() {
        "hypot" if !values.is_empty() => LengthPercentageMath::Hypot(values),
        "abs" if values.len() == 1 => {
            LengthPercentageMath::Abs(Box::new(values.into_iter().next()?))
        }
        "mod" | "rem" if values.len() == 2 => {
            let mut values = values.into_iter();
            LengthPercentageMath::Modulo {
                dividend: Box::new(values.next()?),
                divisor: Box::new(values.next()?),
                remainder: name.eq_ignore_ascii_case("rem"),
            }
        }
        "round" if values.len() == 2 => {
            let mut values = values.into_iter();
            LengthPercentageMath::Round {
                strategy,
                value: Box::new(values.next()?),
                step: Box::new(values.next()?),
            }
        }
        _ => return None,
    };
    Some(LengthMathValue::Length(value))
}

fn scalar_length_math_value(value: &Value, ctx: ResolutionContext) -> Option<LengthMathValue> {
    match individual_transform::computed_numeric_expression(value, ctx)? {
        Value::Number(number) => Some(LengthMathValue::Number(number)),
        Value::Percentage(percentage) => {
            Some(LengthMathValue::Length(LengthPercentageMath::Linear {
                px: 0.0,
                percentage,
            }))
        }
        Value::Length(px, unit) if unit == "px" => {
            Some(LengthMathValue::Length(LengthPercentageMath::Linear {
                px,
                percentage: 0.0,
            }))
        }
        _ => None,
    }
}

fn compare_math_values(values: Vec<LengthMathValue>, minimum: bool) -> Option<LengthMathValue> {
    if matches!(values.first()?, LengthMathValue::Number(_)) {
        let numbers = values
            .into_iter()
            .map(|value| match value {
                LengthMathValue::Number(number) => Some(number),
                _ => None,
            })
            .collect::<Option<Vec<_>>>()?;
        let result = numbers
            .into_iter()
            .reduce(if minimum { f32::min } else { f32::max })?;
        return Some(LengthMathValue::Number(result));
    }
    let values = values
        .into_iter()
        .map(|value| match value {
            LengthMathValue::Length(value) => Some(value),
            _ => None,
        })
        .collect::<Option<Vec<_>>>()?;
    Some(LengthMathValue::Length(if minimum {
        LengthPercentageMath::Min(values)
    } else {
        LengthPercentageMath::Max(values)
    }))
}

fn clamp_math_values(
    minimum: LengthMathValue,
    preferred: LengthMathValue,
    maximum: LengthMathValue,
) -> Option<LengthMathValue> {
    match (minimum, preferred, maximum) {
        (
            LengthMathValue::Number(min),
            LengthMathValue::Number(value),
            LengthMathValue::Number(max),
        ) => Some(LengthMathValue::Number(value.min(max).max(min))),
        (
            LengthMathValue::Length(minimum),
            LengthMathValue::Length(preferred),
            LengthMathValue::Length(maximum),
        ) => Some(LengthMathValue::Length(LengthPercentageMath::Clamp {
            minimum: Box::new(minimum),
            preferred: Box::new(preferred),
            maximum: Box::new(maximum),
        })),
        _ => None,
    }
}

fn compute_math_comparison(
    value: &Value,
    property: &str,
    ctx: ResolutionContext,
) -> Option<ComputedValue> {
    match evaluate_length_math_value(value, ctx)? {
        LengthMathValue::Number(value) => Some(ComputedValue::Number(value)),
        LengthMathValue::Length(value) => {
            Some(computed_length_percentage_math(value, property, ctx))
        }
    }
}

fn evaluate_length_math_expression(
    expression: &Value,
    ctx: ResolutionContext,
) -> Option<LengthMathValue> {
    let mut tokens = Vec::new();
    collect_length_math_tokens(expression, ctx, &mut tokens)?;
    if tokens.is_empty() {
        return None;
    }
    let mut index = 0usize;
    let value = parse_length_math_add_sub(&tokens, &mut index)?;
    (index == tokens.len()).then_some(value)
}

fn collect_length_math_tokens(
    value: &Value,
    ctx: ResolutionContext,
    out: &mut Vec<LengthMathToken>,
) -> Option<()> {
    if let Value::List(values) = value {
        for item in values {
            collect_length_math_tokens(item, ctx, out)?;
        }
        return Some(());
    }
    if let Value::Keyword(operator) = value
        && matches!(operator.as_str(), "+" | "-" | "*" | "/")
    {
        out.push(LengthMathToken::Operator(operator.chars().next()?));
        return Some(());
    }
    out.push(LengthMathToken::Value(evaluate_length_math_value(
        value, ctx,
    )?));
    Some(())
}

fn parse_length_math_add_sub(
    tokens: &[LengthMathToken],
    index: &mut usize,
) -> Option<LengthMathValue> {
    let mut left = parse_length_math_mul_div(tokens, index)?;
    while let Some(LengthMathToken::Operator(operator @ ('+' | '-'))) = tokens.get(*index) {
        let subtract = *operator == '-';
        *index += 1;
        let right = parse_length_math_mul_div(tokens, index)?;
        left = match (left, right) {
            (LengthMathValue::Length(left), LengthMathValue::Length(right)) => {
                LengthMathValue::Length(left.add(if subtract { right.scaled(-1.0) } else { right }))
            }
            (LengthMathValue::Number(left), LengthMathValue::Number(right)) => {
                LengthMathValue::Number(left + if subtract { -right } else { right })
            }
            _ => return None,
        };
    }
    Some(left)
}

fn parse_length_math_mul_div(
    tokens: &[LengthMathToken],
    index: &mut usize,
) -> Option<LengthMathValue> {
    let mut left = parse_length_math_factor(tokens, index)?;
    while let Some(LengthMathToken::Operator(operator @ ('*' | '/'))) = tokens.get(*index) {
        let operator = *operator;
        *index += 1;
        let right = parse_length_math_factor(tokens, index)?;
        left = match (left, operator, right) {
            (LengthMathValue::Length(value), '*', LengthMathValue::Number(factor))
            | (LengthMathValue::Number(factor), '*', LengthMathValue::Length(value)) => {
                LengthMathValue::Length(value.scaled(factor))
            }
            (LengthMathValue::Length(value), '/', LengthMathValue::Number(divisor))
                if divisor != 0.0 =>
            {
                LengthMathValue::Length(value.scaled(1.0 / divisor))
            }
            (LengthMathValue::Number(left), '*', LengthMathValue::Number(right)) => {
                LengthMathValue::Number(left * right)
            }
            (LengthMathValue::Number(left), '/', LengthMathValue::Number(right))
                if right != 0.0 =>
            {
                LengthMathValue::Number(left / right)
            }
            _ => return None,
        };
    }
    Some(left)
}

fn parse_length_math_factor(
    tokens: &[LengthMathToken],
    index: &mut usize,
) -> Option<LengthMathValue> {
    let LengthMathToken::Value(value) = tokens.get(*index)? else {
        return None;
    };
    *index += 1;
    Some(value.clone())
}

fn computed_length_percentage_math(
    value: LengthPercentageMath,
    property_name: &str,
    ctx: ResolutionContext,
) -> ComputedValue {
    if property_name.eq_ignore_ascii_case("font-size") {
        return ComputedValue::Px(value.resolve(ctx.parent_font_size).max(0.0));
    }
    if value.is_pure_px() {
        let px = value.resolve(0.0);
        return ComputedValue::Px(if is_non_negative_sizing_property(property_name) {
            px.max(0.0)
        } else {
            px
        });
    }
    if let Some((px, percentage)) = value.linear_components()
        && px == 0.0
    {
        return ComputedValue::Percentage(if is_non_negative_sizing_property(property_name) {
            percentage.max(0.0)
        } else {
            percentage
        });
    }
    ComputedValue::LengthPercentage(value)
}

fn evaluate_calc(arguments: &[Value], ctx: ResolutionContext) -> Option<CalcQuantity> {
    let quantity = evaluate_calc_unclamped(arguments, ctx).or_else(|| {
        let value = Value::Function {
            name: "calc".into(),
            arguments: arguments.to_vec(),
        };
        match individual_transform::computed_numeric_expression(&value, ctx)? {
            Value::Number(value) => Some(CalcQuantity {
                value,
                unit: CalcUnit::Unitless,
            }),
            Value::Percentage(value) => Some(CalcQuantity {
                value,
                unit: CalcUnit::Percentage,
            }),
            Value::Length(value, unit) if unit == "px" => Some(CalcQuantity {
                value,
                unit: CalcUnit::Px,
            }),
            _ => None,
        }
    })?;
    Some(CalcQuantity {
        value: if quantity.value.is_nan() {
            0.0
        } else {
            quantity.value.clamp(-f32::MAX, f32::MAX)
        },
        unit: quantity.unit,
    })
}

fn evaluate_calc_unclamped(arguments: &[Value], ctx: ResolutionContext) -> Option<CalcQuantity> {
    let expression = arguments.first()?;
    let mut tokens = Vec::new();
    collect_calc_tokens(expression, ctx, &mut tokens)?;
    if tokens.is_empty() {
        return None;
    }

    let mut index = 0usize;
    let value = parse_calc_add_sub(&tokens, &mut index)?;
    if index == tokens.len() {
        Some(value)
    } else {
        None
    }
}

fn collect_calc_tokens(
    value: &Value,
    ctx: ResolutionContext,
    out: &mut Vec<CalcToken>,
) -> Option<()> {
    match value {
        Value::List(values) => {
            for item in values {
                collect_calc_tokens(item, ctx, out)?;
            }
            Some(())
        }
        Value::Keyword(op) if matches!(op.as_str(), "+" | "-" | "*" | "/") => {
            out.push(CalcToken::Operator(op.chars().next()?));
            Some(())
        }
        Value::Length(number, unit) => {
            let px = resolve_length_to_px(*number, unit, ctx)?;
            out.push(CalcToken::Value(CalcQuantity {
                value: px,
                unit: CalcUnit::Px,
            }));
            Some(())
        }
        Value::Percentage(number) => {
            out.push(CalcToken::Value(CalcQuantity {
                value: *number,
                unit: CalcUnit::Percentage,
            }));
            Some(())
        }
        Value::Number(number) => {
            out.push(CalcToken::Value(CalcQuantity {
                value: *number,
                unit: CalcUnit::Unitless,
            }));
            Some(())
        }
        Value::Function { name, arguments } if name.eq_ignore_ascii_case("calc") => {
            out.push(CalcToken::Value(evaluate_calc_unclamped(arguments, ctx)?));
            Some(())
        }
        Value::Function { .. } => {
            let number = individual_transform::unclamped_number_expression(value, ctx)?;
            out.push(CalcToken::Value(CalcQuantity {
                value: number,
                unit: CalcUnit::Unitless,
            }));
            Some(())
        }
        _ => None,
    }
}

fn resolve_length_to_px(number: f32, unit: &str, ctx: ResolutionContext) -> Option<f32> {
    Some(match unit.to_ascii_lowercase().as_str() {
        "px" => number,
        "em" => number * ctx.parent_font_size,
        "rem" => number * ctx.root_font_size,
        "ex" => number * ctx.font_metrics.ex,
        "ch" => number * ctx.font_metrics.ch,
        "cap" => number * ctx.font_metrics.cap,
        "ic" => number * ctx.font_metrics.ic,
        "lh" => number * ctx.line_height,
        "rlh" => number * ctx.root_line_height,
        "vw" => number * ctx.viewport_width / 100.0,
        "vh" => number * ctx.viewport_height / 100.0,
        "svw" | "lvw" | "dvw" => number * ctx.viewport_width / 100.0,
        "svh" | "lvh" | "dvh" => number * ctx.viewport_height / 100.0,
        "vi" | "svi" | "lvi" | "dvi" => number * ctx.viewport_width / 100.0,
        "vb" | "svb" | "lvb" | "dvb" => number * ctx.viewport_height / 100.0,
        "cqw" => number * ctx.container_size[0].unwrap_or(ctx.viewport_width) / 100.0,
        "cqh" => number * ctx.container_size[1].unwrap_or(ctx.viewport_height) / 100.0,
        "cqi" => {
            let axis = usize::from(ctx.vertical_writing);
            number
                * ctx.container_size[axis]
                    .unwrap_or([ctx.viewport_width, ctx.viewport_height][axis])
                / 100.0
        }
        "cqb" => {
            let axis = 1 - usize::from(ctx.vertical_writing);
            number
                * ctx.container_size[axis]
                    .unwrap_or([ctx.viewport_width, ctx.viewport_height][axis])
                / 100.0
        }
        "cqmin" | "cqmax" => {
            let inline_axis = usize::from(ctx.vertical_writing);
            let block_axis = 1 - inline_axis;
            let viewport = [ctx.viewport_width, ctx.viewport_height];
            let inline = ctx.container_size[inline_axis].unwrap_or(viewport[inline_axis]);
            let block = ctx.container_size[block_axis].unwrap_or(viewport[block_axis]);
            number
                * if unit.eq_ignore_ascii_case("cqmin") {
                    inline.min(block)
                } else {
                    inline.max(block)
                }
                / 100.0
        }
        "vmin" => number * ctx.viewport_width.min(ctx.viewport_height) / 100.0,
        "vmax" => number * ctx.viewport_width.max(ctx.viewport_height) / 100.0,
        "mm" => number * (96.0 / 25.4),
        "q" => number * (96.0 / 101.6),
        "cm" => number * (96.0 / 2.54),
        "in" => number * 96.0,
        "pt" => number * (96.0 / 72.0),
        "pc" => number * (96.0 / 6.0),
        _ => return None,
    })
}

fn parse_calc_add_sub(tokens: &[CalcToken], index: &mut usize) -> Option<CalcQuantity> {
    let mut left = parse_calc_mul_div(tokens, index)?;
    while let Some(CalcToken::Operator(op @ ('+' | '-'))) = tokens.get(*index) {
        let op = *op;
        *index += 1;
        let right = parse_calc_mul_div(tokens, index)?;
        left = apply_calc_operator(left, op, right)?;
    }
    Some(left)
}

fn parse_calc_mul_div(tokens: &[CalcToken], index: &mut usize) -> Option<CalcQuantity> {
    let mut left = parse_calc_factor(tokens, index)?;
    while let Some(CalcToken::Operator(op @ ('*' | '/'))) = tokens.get(*index) {
        let op = *op;
        *index += 1;
        let right = parse_calc_factor(tokens, index)?;
        left = apply_calc_operator(left, op, right)?;
    }
    Some(left)
}

fn parse_calc_factor(tokens: &[CalcToken], index: &mut usize) -> Option<CalcQuantity> {
    let value = match tokens.get(*index) {
        Some(CalcToken::Value(value)) => *value,
        _ => return None,
    };
    *index += 1;
    Some(value)
}

fn apply_calc_operator(left: CalcQuantity, op: char, right: CalcQuantity) -> Option<CalcQuantity> {
    match op {
        '+' => add_or_sub_calc_quantities(left, right, false),
        '-' => add_or_sub_calc_quantities(left, right, true),
        '*' => multiply_calc_quantities(left, right),
        '/' => divide_calc_quantities(left, right),
        _ => None,
    }
}

fn add_or_sub_calc_quantities(
    left: CalcQuantity,
    right: CalcQuantity,
    subtract: bool,
) -> Option<CalcQuantity> {
    if left.unit != right.unit {
        return None;
    }
    let rhs = if subtract { -right.value } else { right.value };
    Some(CalcQuantity {
        value: left.value + rhs,
        unit: left.unit,
    })
}

fn multiply_calc_quantities(left: CalcQuantity, right: CalcQuantity) -> Option<CalcQuantity> {
    match (left.unit, right.unit) {
        (CalcUnit::Unitless, unit) => Some(CalcQuantity {
            value: left.value * right.value,
            unit,
        }),
        (unit, CalcUnit::Unitless) => Some(CalcQuantity {
            value: left.value * right.value,
            unit,
        }),
        _ => None,
    }
}

fn divide_calc_quantities(left: CalcQuantity, right: CalcQuantity) -> Option<CalcQuantity> {
    if right.unit != CalcUnit::Unitless {
        return None;
    }
    Some(CalcQuantity {
        value: left.value / right.value,
        unit: left.unit,
    })
}

fn is_svg_element_for_presentational_hints(node: &NodeHandle) -> bool {
    if node.namespace_uri().as_deref() == Some("http://www.w3.org/2000/svg") {
        return true;
    }
    let is_svg_tag = node.with_tag_name(|tag| {
        tag.is_some_and(|tag| {
            [
                "svg", "g", "rect", "circle", "ellipse", "line", "polyline", "polygon", "path",
                "text", "tspan", "textpath", "use",
            ]
            .iter()
            .any(|expected| tag.eq_ignore_ascii_case(expected))
        })
    });
    if !is_svg_tag {
        return false;
    }
    let mut current = Some(node.clone());
    while let Some(candidate) = current {
        if candidate
            .with_tag_name(|tag| tag.is_some_and(|tag| tag.eq_ignore_ascii_case("foreignobject")))
        {
            return false;
        }
        if candidate.with_tag_name(|tag| tag.is_some_and(|tag| tag.eq_ignore_ascii_case("svg"))) {
            return true;
        }
        current = candidate.parent_node();
    }
    false
}

fn apply_presentational_hints(
    node: &NodeHandle,
    properties: &mut PropertyMap,
    pseudo: Option<PseudoElement>,
) {
    if pseudo.is_some() || node.node_type() != NodeType::Element {
        return;
    }

    // HTML defines directionality independently of the CSS `direction`
    // property. Its rendering rule behaves like a low-priority
    // presentational hint and remains overridable by author declarations.
    if !properties.contains_key(&PropertyId::Direction)
        && let Some(direction) = node.directionality_presentational_hint()
    {
        properties.insert(
            PropertyId::Direction,
            ComputedValue::Keyword(direction.to_owned()),
        );
    }

    node.with_attributes(|attributes| {
        if let Some(attributes) = attributes {
            apply_presentational_hints_from_attributes(node, attributes, properties);
        }
    });
}

fn apply_presentational_hints_from_attributes(
    node: &NodeHandle,
    attributes: &BTreeMap<String, String>,
    properties: &mut PropertyMap,
) {
    // SVG presentation attributes participate in the CSS cascade below author
    // declarations. Expose pointer-events through computed style so hit
    // testing can distinguish a local attribute from an inherited value and
    // still honor explicit CSS overrides, including `auto`.
    let is_svg_element = is_svg_element_for_presentational_hints(node);
    if is_svg_element
        && !properties.contains_key(&PropertyId::PointerEvents)
        && let Some(value) = attributes
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("pointer-events"))
            .map(|(_, value)| value.trim())
            .filter(|value| !value.is_empty())
            .filter(|value| {
                let lower = value.to_ascii_lowercase();
                is_css_wide_keyword(&lower) || is_supported_pointer_events_keyword(&lower)
            })
    {
        properties.insert(
            PropertyId::PointerEvents,
            ComputedValue::Keyword(value.to_ascii_lowercase()),
        );
    }

    if !properties.contains_key(&PropertyId::BackgroundColor)
        && let Some(background) = attributes
            .get("bgcolor")
            .and_then(|value| parse_legacy_color_hint(value))
    {
        properties.insert(
            PropertyId::BackgroundColor,
            ComputedValue::Color(background),
        );
    }

    if !properties.contains_key(&PropertyId::BackgroundImage)
        && let Some(background) = attributes
            .get("background")
            .map(|value| value.trim())
            .filter(|value| !value.is_empty())
    {
        let escaped = background.replace('\\', "\\\\").replace('"', "\\\"");
        properties.insert(
            PropertyId::BackgroundImage,
            ComputedValue::Keyword(format!("url(\"{escaped}\")")),
        );
    }

    if !properties.contains_key(&PropertyId::Color)
        && node.with_tag_name(|name| name.is_some_and(|name| name.eq_ignore_ascii_case("body")))
        && let Some(color) = attributes
            .get("text")
            .and_then(|value| parse_legacy_color_hint(value))
    {
        properties.insert(PropertyId::Color, ComputedValue::Color(color));
    }

    if let Some(align) = attributes
        .get("align")
        .map(|value| value.trim().to_ascii_lowercase())
        .filter(|value| matches!(value.as_str(), "left" | "right" | "center" | "justify"))
    {
        if !properties.contains_key(&PropertyId::TextAlign) {
            properties.insert(PropertyId::TextAlign, ComputedValue::Keyword(align.clone()));
        }
        // For block/table elements, align="center" means auto margins (structural centering)
        if align == "center" {
            let is_table_or_block = node.with_tag_name(|tag| {
                tag.is_some_and(|tag| {
                    matches!(
                        tag.to_ascii_lowercase().as_str(),
                        "table" | "div" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "p"
                    )
                })
            });
            if is_table_or_block {
                if !properties.contains_key(&PropertyId::MarginLeft) {
                    properties.insert(
                        PropertyId::MarginLeft,
                        ComputedValue::Keyword("auto".to_string()),
                    );
                }
                if !properties.contains_key(&PropertyId::MarginRight) {
                    properties.insert(
                        PropertyId::MarginRight,
                        ComputedValue::Keyword("auto".to_string()),
                    );
                }
            }
        }
    }

    if !properties.contains_key(&PropertyId::Width)
        && let Some(width) = attributes
            .get("width")
            .and_then(|value| parse_legacy_dimension_hint(value))
    {
        properties.insert(PropertyId::Width, width);
    }

    if !properties.contains_key(&PropertyId::Height)
        && let Some(height) = attributes
            .get("height")
            .and_then(|value| parse_legacy_dimension_hint(value))
    {
        properties.insert(PropertyId::Height, height);
    }

    if !properties.contains_key(&PropertyId::Color)
        && let Some(color) = attributes
            .get("color")
            .and_then(|value| parse_legacy_color_hint(value))
    {
        properties.insert(PropertyId::Color, ComputedValue::Color(color));
    }

    if !properties.contains_key(&PropertyId::FontFamily)
        && let Some(face) = attributes
            .get("face")
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    {
        properties.insert(PropertyId::FontFamily, ComputedValue::Keyword(face));
    }
}

fn parse_legacy_color_hint(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }

    if let Some(hex) = value.strip_prefix('#') {
        return if is_hex_color(hex) {
            Some(format!("#{hex}").to_ascii_lowercase())
        } else {
            None
        };
    }

    if is_hex_color(value) {
        return Some(format!("#{value}").to_ascii_lowercase());
    }

    if value.chars().all(|ch| ch.is_ascii_alphabetic()) {
        return Some(value.to_ascii_lowercase());
    }

    None
}

fn parse_legacy_dimension_hint(value: &str) -> Option<ComputedValue> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }

    if let Some(percent) = value
        .strip_suffix('%')
        .and_then(|v| v.trim().parse::<f32>().ok())
    {
        return Some(ComputedValue::Percentage(percent));
    }

    if let Some(px) = value
        .strip_suffix("px")
        .and_then(|v| v.trim().parse::<f32>().ok())
    {
        return Some(ComputedValue::Px(px.max(0.0)));
    }

    value
        .parse::<f32>()
        .ok()
        .map(|px| ComputedValue::Px(px.max(0.0)))
}

fn is_hex_color(value: &str) -> bool {
    (value.len() == 3 || value.len() == 6) && value.chars().all(|ch| ch.is_ascii_hexdigit())
}

fn apply_ua_defaults(
    node: &NodeHandle,
    properties: &mut PropertyMap,
    pseudo: Option<PseudoElement>,
    parent_style: Option<&ComputedStyle>,
) {
    if node.node_type() != NodeType::Element {
        return;
    }
    if pseudo == Some(PseudoElement::Backdrop) {
        if node.is_fullscreen() {
            properties
                .entry(PropertyId::BackgroundColor)
                .or_insert(ComputedValue::Keyword("black".to_string()));
        } else if node.is_modal_dialog() {
            // HTML: `dialog:modal::backdrop { background: rgba(0, 0, 0, 0.1) }`.
            properties
                .entry(PropertyId::BackgroundColor)
                .or_insert(ComputedValue::Color("rgba(0, 0, 0, 0.1)".to_string()));
        }
        return;
    }
    if pseudo.is_some() {
        return;
    }
    forced_colors::apply_svg_ua_defaults(node, properties);
    let tag = match node.tag_name() {
        Some(tag) => tag.to_ascii_lowercase(),
        None => return,
    };
    // These UA display values must be visible to CSSOM as well as layout.
    // Explicit author display values have already won the cascade above.
    let default_display = match tag.as_str() {
        "address" | "article" | "aside" | "blockquote" | "body" | "dd" | "details" | "div"
        | "dl" | "dt" | "fieldset" | "figcaption" | "figure" | "footer" | "form" | "h1" | "h2"
        | "h3" | "h4" | "h5" | "h6" | "header" | "hgroup" | "hr" | "html" | "legend" | "main"
        | "menu" | "nav" | "ol" | "p" | "pre" | "section" | "ul" => Some("block"),
        "area" | "base" | "basefont" | "datalist" | "head" | "link" | "meta" | "noembed"
        | "noframes" | "noscript" | "param" | "rp" | "script" | "style" | "template" | "title"
        | "track" => Some("none"),
        "caption" => Some("table-caption"),
        "col" => Some("table-column"),
        "colgroup" => Some("table-column-group"),
        _ => None,
    };
    if let Some(display) = default_display {
        properties
            .entry(PropertyId::Display)
            .or_insert_with(|| ComputedValue::Keyword(display.to_string()));
    }
    let parent_font_size = inherited_font_size(parent_style, properties);
    if tag == "br" {
        properties
            .entry(PropertyId::Display)
            .or_insert_with(|| ComputedValue::Keyword("inline".to_string()));
    }

    // Fullscreen's UA rules fill the viewport and suppress transforms. These
    // declarations are mandatory overrides in the Fullscreen specification.
    if node.is_fullscreen() {
        for (name, value) in [
            ("position", ComputedValue::Keyword("fixed".to_string())),
            (
                "box-sizing",
                ComputedValue::Keyword("border-box".to_string()),
            ),
            ("left", ComputedValue::Px(0.0)),
            ("right", ComputedValue::Px(0.0)),
            ("top", ComputedValue::Px(0.0)),
            ("bottom", ComputedValue::Px(0.0)),
            ("margin-top", ComputedValue::Px(0.0)),
            ("margin-right", ComputedValue::Px(0.0)),
            ("margin-bottom", ComputedValue::Px(0.0)),
            ("margin-left", ComputedValue::Px(0.0)),
            ("width", ComputedValue::Percentage(100.0)),
            ("height", ComputedValue::Percentage(100.0)),
            ("min-width", ComputedValue::Px(0.0)),
            ("min-height", ComputedValue::Px(0.0)),
            ("max-width", ComputedValue::Keyword("none".to_string())),
            ("max-height", ComputedValue::Keyword("none".to_string())),
            ("transform", ComputedValue::Keyword("none".to_string())),
        ] {
            properties.insert(name.to_string(), value);
        }
    }

    if node.get_attribute("popover").is_some() {
        if !node.is_popover_open() {
            properties.insert(
                PropertyId::Display,
                ComputedValue::Keyword("none".to_string()),
            );
            return;
        }
        properties
            .entry(PropertyId::Display)
            .or_insert(ComputedValue::Keyword("block".to_string()));
        properties
            .entry(PropertyId::Position)
            .or_insert(ComputedValue::Keyword("fixed".to_string()));
        properties
            .entry(PropertyId::Left)
            .or_insert(ComputedValue::Percentage(50.0));
        properties
            .entry(PropertyId::Top)
            .or_insert(ComputedValue::Percentage(50.0));
        properties
            .entry(PropertyId::Transform)
            .or_insert(ComputedValue::Keyword("translate(-50%, -50%)".to_string()));
    }

    if tag != "summary"
        && node.parent_node().is_some_and(|parent| {
            parent.tag_name().as_deref() == Some("details")
                && parent.get_attribute("open").is_none()
        })
    {
        properties.insert(
            PropertyId::Display,
            ComputedValue::Keyword("none".to_string()),
        );
        return;
    }

    // UA stylesheet defaults per CSS 2.1 Appendix D / HTML spec
    struct UaDefaults {
        font_size_em: f32,
        font_weight_bold: bool,
        margin_em: f32,
    }

    let defaults = match tag.as_str() {
        "h1" => Some(UaDefaults {
            font_size_em: 2.0,
            font_weight_bold: true,
            margin_em: 0.67,
        }),
        "h2" => Some(UaDefaults {
            font_size_em: 1.5,
            font_weight_bold: true,
            margin_em: 0.83,
        }),
        "h3" => Some(UaDefaults {
            font_size_em: 1.17,
            font_weight_bold: true,
            margin_em: 1.0,
        }),
        "h4" => Some(UaDefaults {
            font_size_em: 1.0,
            font_weight_bold: true,
            margin_em: 1.33,
        }),
        "h5" => Some(UaDefaults {
            font_size_em: 0.83,
            font_weight_bold: true,
            margin_em: 1.67,
        }),
        "h6" => Some(UaDefaults {
            font_size_em: 0.67,
            font_weight_bold: true,
            margin_em: 2.33,
        }),
        _ => None,
    };

    if let Some(defaults) = defaults {
        // Determine the element's final font size: use existing CSS value if present,
        // otherwise apply the UA default multiplier to the inherited size.
        let element_font_size =
            if let Some(ComputedValue::Px(existing_px)) = properties.get(&PropertyId::FontSize) {
                *existing_px
            } else {
                let computed = defaults.font_size_em * parent_font_size;
                properties
                    .entry(PropertyId::FontSize)
                    .or_insert(ComputedValue::Px(computed));
                computed
            };
        let margin_px = defaults.margin_em * element_font_size;
        if defaults.font_weight_bold {
            properties
                .entry(PropertyId::FontWeight)
                .or_insert(ComputedValue::Keyword("bold".to_string()));
        }
        properties
            .entry(PropertyId::MarginTop)
            .or_insert(ComputedValue::Px(margin_px));
        properties
            .entry(PropertyId::MarginBottom)
            .or_insert(ComputedValue::Px(margin_px));
        return;
    }

    match tag.as_str() {
        "iframe" => apply_iframe_ua_defaults(properties),
        "video" | "canvas" | "picture" => apply_video_like_ua_defaults(properties),
        "audio" => apply_audio_ua_defaults(node, properties),
        "source" => {
            properties.insert(
                PropertyId::Display,
                ComputedValue::Keyword("none".to_string()),
            );
        }
        "details" => {
            properties
                .entry(PropertyId::Display)
                .or_insert(ComputedValue::Keyword("block".to_string()));
        }
        "summary" => {
            properties
                .entry(PropertyId::Display)
                .or_insert(ComputedValue::Keyword("list-item".to_string()));
        }
        "dialog" => apply_dialog_ua_defaults(node, properties, parent_font_size),
        "time" => {
            properties
                .entry(PropertyId::Display)
                .or_insert(ComputedValue::Keyword("inline".to_string()));
        }
        "rt" => {
            properties
                .entry(PropertyId::FontSize)
                .or_insert(ComputedValue::Px(parent_font_size * 0.5));
            properties
                .entry(PropertyId::LineHeight)
                .or_insert(ComputedValue::Number(1.0));
            properties
                .entry(PropertyId::TextEmphasisStyle)
                .or_insert(ComputedValue::Keyword("none".to_string()));
        }
        "progress" | "meter" => apply_progress_or_meter_ua_defaults(properties),
        "form" => {
            properties
                .entry(PropertyId::Display)
                .or_insert(ComputedValue::Keyword("block".to_string()));
        }
        "input" => apply_input_ua_defaults(node, properties),
        "button" => apply_button_ua_defaults(properties),
        "textarea" => apply_textarea_ua_defaults(properties),
        "select" => apply_select_ua_defaults(properties),
        "p" => apply_p_ua_defaults(properties, parent_font_size),
        "b" | "strong" => {
            properties
                .entry(PropertyId::FontWeight)
                .or_insert(ComputedValue::Keyword("bold".to_string()));
        }
        "i" | "em" => {
            properties
                .entry(PropertyId::FontStyle)
                .or_insert(ComputedValue::Keyword("italic".to_string()));
        }
        "hr" => apply_hr_ua_defaults(properties, parent_font_size),
        "ul" => apply_list_ua_defaults(properties, parent_font_size, "disc"),
        "ol" => apply_list_ua_defaults(properties, parent_font_size, "decimal"),
        "li" => {
            properties
                .entry(PropertyId::Display)
                .or_insert(ComputedValue::Keyword("list-item".to_string()));
        }
        "blockquote" => apply_blockquote_ua_defaults(properties, parent_font_size),
        "pre" => apply_pre_ua_defaults(properties, parent_font_size),
        "code" | "kbd" | "samp" | "tt" => {
            properties
                .entry(PropertyId::FontFamily)
                .or_insert(ComputedValue::Keyword("monospace".to_string()));
            properties
                .entry(PropertyId::Display)
                .or_insert(ComputedValue::Keyword("inline".to_string()));
        }
        "dd" => {
            properties
                .entry(PropertyId::MarginLeft)
                .or_insert(ComputedValue::Px(40.0));
        }
        "th" => apply_th_ua_defaults(properties),
        "td" => {
            properties
                .entry(PropertyId::Display)
                .or_insert(ComputedValue::Keyword("table-cell".to_string()));
        }
        "a" => {
            properties
                .entry(PropertyId::TextDecorationLine)
                .or_insert(ComputedValue::Keyword("underline".to_string()));
            properties
                .entry(PropertyId::Color)
                .or_insert(ComputedValue::Color("#0000ee".to_string()));
        }
        "sub" => apply_scaled_inline_ua_defaults(properties, parent_font_size, Some("sub")),
        "sup" => apply_scaled_inline_ua_defaults(properties, parent_font_size, Some("super")),
        "small" => apply_scaled_inline_ua_defaults(properties, parent_font_size, None),
        "center" => {
            properties
                .entry(PropertyId::TextAlign)
                .or_insert(ComputedValue::Keyword("center".to_string()));
        }
        "table" => {
            properties
                .entry(PropertyId::Display)
                .or_insert(ComputedValue::Keyword("table".to_string()));
        }
        "tr" => {
            properties
                .entry(PropertyId::Display)
                .or_insert(ComputedValue::Keyword("table-row".to_string()));
        }
        "thead" => {
            properties
                .entry(PropertyId::Display)
                .or_insert(ComputedValue::Keyword("table-header-group".to_string()));
        }
        "tbody" => {
            properties
                .entry(PropertyId::Display)
                .or_insert(ComputedValue::Keyword("table-row-group".to_string()));
        }
        "tfoot" => {
            properties
                .entry(PropertyId::Display)
                .or_insert(ComputedValue::Keyword("table-footer-group".to_string()));
        }
        _ => {}
    }
}

/// Applies a uniform border (same style/width, and optionally the same
/// color) to all four sides. Several UA default blocks below repeat this
/// per-side loop for their tag's default border.
fn apply_uniform_border(
    properties: &mut PropertyMap,
    style: &'static str,
    width: f32,
    color: Option<&'static str>,
) {
    for side in ["top", "right", "bottom", "left"] {
        properties
            .entry(format!("border-{side}-style"))
            .or_insert(ComputedValue::Keyword(style.to_string()));
        properties
            .entry(format!("border-{side}-width"))
            .or_insert(ComputedValue::Px(width));
        if let Some(color) = color {
            properties
                .entry(format!("border-{side}-color"))
                .or_insert(ComputedValue::Color(color.to_string()));
        }
    }
}

/// `<iframe>`'s UA default: HTML's rendering defaults give the replaced
/// element a 2px inset border.
fn apply_iframe_ua_defaults(properties: &mut PropertyMap) {
    apply_uniform_border(properties, "inset", 2.0, None);
}

/// `<video>`/`<canvas>`/`<picture>`'s shared UA default: `inline-block`.
fn apply_video_like_ua_defaults(properties: &mut PropertyMap) {
    properties
        .entry(PropertyId::Display)
        .or_insert(ComputedValue::Keyword("inline-block".to_string()));
}

/// `<audio>`'s UA default: hidden unless it has a `controls` attribute.
fn apply_audio_ua_defaults(node: &NodeHandle, properties: &mut PropertyMap) {
    if node.get_attribute("controls").is_none() {
        properties.insert(
            PropertyId::Display,
            ComputedValue::Keyword("none".to_string()),
        );
    } else {
        properties
            .entry(PropertyId::Display)
            .or_insert(ComputedValue::Keyword("inline-block".to_string()));
    }
}

/// `<dialog>`'s UA defaults from the HTML rendering section.
///
/// Closed dialogs are hidden. An open dialog is an absolutely positioned,
/// fit-content box centered inline by auto margins; a modal dialog is fixed
/// and also centered in the block axis.
fn apply_dialog_ua_defaults(
    node: &NodeHandle,
    properties: &mut PropertyMap,
    parent_font_size: f32,
) {
    if node.get_attribute("open").is_none() {
        properties.insert(
            PropertyId::Display,
            ComputedValue::Keyword("none".to_string()),
        );
        return;
    }
    let keyword = |value: &str| ComputedValue::Keyword(value.to_string());
    let modal = node.is_modal_dialog();
    let mut defaults = vec![
        (PropertyId::Display, keyword("block")),
        (
            PropertyId::Position,
            keyword(if modal { "fixed" } else { "absolute" }),
        ),
        (PropertyId::Left, ComputedValue::Px(0.0)),
        (PropertyId::Right, ComputedValue::Px(0.0)),
        (PropertyId::Width, keyword("fit-content")),
        (PropertyId::Height, keyword("fit-content")),
        (PropertyId::MarginTop, keyword("auto")),
        (PropertyId::MarginRight, keyword("auto")),
        (PropertyId::MarginBottom, keyword("auto")),
        (PropertyId::MarginLeft, keyword("auto")),
        (
            PropertyId::BackgroundColor,
            ComputedValue::Color("white".to_string()),
        ),
        (PropertyId::Color, ComputedValue::Color("black".to_string())),
    ];
    let font_size = match properties.get(&PropertyId::FontSize) {
        Some(ComputedValue::Px(size)) => *size,
        _ => parent_font_size,
    };
    for side in [
        PropertyId::PaddingTop,
        PropertyId::PaddingRight,
        PropertyId::PaddingBottom,
        PropertyId::PaddingLeft,
    ] {
        defaults.push((side, ComputedValue::Px(font_size)));
    }
    if modal {
        defaults.extend([
            (PropertyId::Top, ComputedValue::Px(0.0)),
            (PropertyId::Bottom, ComputedValue::Px(0.0)),
            (PropertyId::OverflowX, keyword("auto")),
            (PropertyId::OverflowY, keyword("auto")),
        ]);
    }
    for (property, value) in defaults {
        properties.entry(property).or_insert(value);
    }
    // `border: solid` uses the initial medium width and currentColor.
    apply_uniform_border(properties, "solid", 3.0, None);
}

/// `<progress>`/`<meter>`'s shared UA default: a fixed-size inline-block box
/// with a light gray fill and a thin solid border.
fn apply_progress_or_meter_ua_defaults(properties: &mut PropertyMap) {
    properties
        .entry(PropertyId::Display)
        .or_insert(ComputedValue::Keyword("inline-block".to_string()));
    properties
        .entry(PropertyId::Width)
        .or_insert(ComputedValue::Px(160.0));
    properties
        .entry(PropertyId::Height)
        .or_insert(ComputedValue::Px(16.0));
    properties
        .entry(PropertyId::BackgroundColor)
        .or_insert(ComputedValue::Color("#e6e6e6".to_string()));
    apply_uniform_border(properties, "solid", 1.0, Some("#767676"));
}

/// `<input>`'s UA default: `type=hidden` is hidden; other types render as a
/// bordered, padded inline-block box.
fn apply_input_ua_defaults(node: &NodeHandle, properties: &mut PropertyMap) {
    let input_type = node
        .get_attribute("type")
        .unwrap_or_else(|| "text".to_string())
        .trim()
        .to_ascii_lowercase();
    if input_type == "hidden" {
        properties.insert(
            PropertyId::Display,
            ComputedValue::Keyword("none".to_string()),
        );
        return;
    }
    properties
        .entry(PropertyId::Display)
        .or_insert(ComputedValue::Keyword("inline-block".to_string()));
    properties
        .entry(PropertyId::Appearance)
        .or_insert(ComputedValue::Keyword("auto".to_string()));
    if matches!(input_type.as_str(), "checkbox" | "radio" | "range") {
        properties
            .entry(PropertyId::BackgroundColor)
            .or_insert(ComputedValue::Color("transparent".to_string()));
        return;
    }
    properties
        .entry(PropertyId::BackgroundColor)
        .or_insert(ComputedValue::Color("white".to_string()));
    apply_uniform_border(properties, "solid", 2.0, Some("#767676"));
    properties
        .entry(PropertyId::PaddingTop)
        .or_insert(ComputedValue::Px(1.0));
    properties
        .entry(PropertyId::PaddingRight)
        .or_insert(ComputedValue::Px(2.0));
    properties
        .entry(PropertyId::PaddingBottom)
        .or_insert(ComputedValue::Px(1.0));
    properties
        .entry(PropertyId::PaddingLeft)
        .or_insert(ComputedValue::Px(2.0));
}

/// `<button>`'s UA default: a centered, bordered, padded inline-block box.
fn apply_button_ua_defaults(properties: &mut PropertyMap) {
    properties
        .entry(PropertyId::Display)
        .or_insert(ComputedValue::Keyword("inline-block".to_string()));
    properties
        .entry(PropertyId::Appearance)
        .or_insert(ComputedValue::Keyword("auto".to_string()));
    properties
        .entry(PropertyId::BackgroundColor)
        .or_insert(ComputedValue::Color("#efefef".to_string()));
    properties
        .entry(PropertyId::TextAlign)
        .or_insert(ComputedValue::Keyword("center".to_string()));
    apply_uniform_border(properties, "solid", 2.0, Some("#767676"));
    properties
        .entry(PropertyId::PaddingTop)
        .or_insert(ComputedValue::Px(1.0));
    properties
        .entry(PropertyId::PaddingRight)
        .or_insert(ComputedValue::Px(6.0));
    properties
        .entry(PropertyId::PaddingBottom)
        .or_insert(ComputedValue::Px(1.0));
    properties
        .entry(PropertyId::PaddingLeft)
        .or_insert(ComputedValue::Px(6.0));
}

/// `<textarea>`'s UA default: a bordered, uniformly padded inline-block box.
fn apply_textarea_ua_defaults(properties: &mut PropertyMap) {
    properties
        .entry(PropertyId::Display)
        .or_insert(ComputedValue::Keyword("inline-block".to_string()));
    properties
        .entry(PropertyId::Appearance)
        .or_insert(ComputedValue::Keyword("auto".to_string()));
    properties
        .entry(PropertyId::BackgroundColor)
        .or_insert(ComputedValue::Color("white".to_string()));
    apply_uniform_border(properties, "solid", 1.0, Some("#767676"));
    for side in ["top", "right", "bottom", "left"] {
        properties
            .entry(format!("padding-{side}"))
            .or_insert(ComputedValue::Px(2.0));
    }
}

/// `<select>`'s UA default: a bordered, padded inline-block box.
fn apply_select_ua_defaults(properties: &mut PropertyMap) {
    properties
        .entry(PropertyId::Display)
        .or_insert(ComputedValue::Keyword("inline-block".to_string()));
    properties
        .entry(PropertyId::Appearance)
        .or_insert(ComputedValue::Keyword("auto".to_string()));
    properties
        .entry(PropertyId::BackgroundColor)
        .or_insert(ComputedValue::Color("#efefef".to_string()));
    apply_uniform_border(properties, "solid", 1.0, Some("#767676"));
    properties
        .entry(PropertyId::PaddingTop)
        .or_insert(ComputedValue::Px(1.0));
    properties
        .entry(PropertyId::PaddingRight)
        .or_insert(ComputedValue::Px(4.0));
    properties
        .entry(PropertyId::PaddingBottom)
        .or_insert(ComputedValue::Px(1.0));
    properties
        .entry(PropertyId::PaddingLeft)
        .or_insert(ComputedValue::Px(4.0));
}

/// `<p>`'s UA default: one line of vertical margin on each side.
fn apply_p_ua_defaults(properties: &mut PropertyMap, parent_font_size: f32) {
    let em = parent_font_size;
    properties
        .entry(PropertyId::MarginTop)
        .or_insert(ComputedValue::Px(em));
    properties
        .entry(PropertyId::MarginBottom)
        .or_insert(ComputedValue::Px(em));
}

/// `<hr>`'s UA default: an inset top border and half-line vertical margins.
fn apply_hr_ua_defaults(properties: &mut PropertyMap, parent_font_size: f32) {
    properties
        .entry(PropertyId::BorderTopStyle)
        .or_insert(ComputedValue::Keyword("inset".to_string()));
    properties
        .entry(PropertyId::BorderTopWidth)
        .or_insert(ComputedValue::Px(1.0));
    let half_em = parent_font_size * 0.5;
    properties
        .entry(PropertyId::MarginTop)
        .or_insert(ComputedValue::Px(half_em));
    properties
        .entry(PropertyId::MarginBottom)
        .or_insert(ComputedValue::Px(half_em));
}

/// `<ul>`/`<ol>`'s shared UA default: a marker style/position, one line of
/// vertical margin, and an indented left padding. `list_style_type` is the
/// only difference between the two tags (`disc` vs. `decimal`).
fn apply_list_ua_defaults(
    properties: &mut PropertyMap,
    parent_font_size: f32,
    list_style_type: &'static str,
) {
    properties
        .entry(PropertyId::ListStyleType)
        .or_insert(ComputedValue::Keyword(list_style_type.to_string()));
    properties
        .entry(PropertyId::ListStylePosition)
        .or_insert(ComputedValue::Keyword("outside".to_string()));
    let em = parent_font_size;
    properties
        .entry(PropertyId::MarginTop)
        .or_insert(ComputedValue::Px(em));
    properties
        .entry(PropertyId::MarginBottom)
        .or_insert(ComputedValue::Px(em));
    properties
        .entry(PropertyId::PaddingLeft)
        .or_insert(ComputedValue::Px(em * 2.5));
}

/// `<blockquote>`'s UA default: one line of vertical margin and a 40px
/// horizontal inset on each side.
fn apply_blockquote_ua_defaults(properties: &mut PropertyMap, parent_font_size: f32) {
    let em = parent_font_size;
    properties
        .entry(PropertyId::MarginTop)
        .or_insert(ComputedValue::Px(em));
    properties
        .entry(PropertyId::MarginBottom)
        .or_insert(ComputedValue::Px(em));
    properties
        .entry(PropertyId::MarginLeft)
        .or_insert(ComputedValue::Px(40.0));
    properties
        .entry(PropertyId::MarginRight)
        .or_insert(ComputedValue::Px(40.0));
}

/// `<pre>`'s UA default: a monospace, whitespace-preserving block with one
/// line of vertical margin.
fn apply_pre_ua_defaults(properties: &mut PropertyMap, parent_font_size: f32) {
    properties
        .entry(PropertyId::FontFamily)
        .or_insert(ComputedValue::Keyword("monospace".to_string()));
    properties
        .entry(PropertyId::WhiteSpace)
        .or_insert(ComputedValue::Keyword("pre".to_string()));
    let em = parent_font_size;
    properties
        .entry(PropertyId::MarginTop)
        .or_insert(ComputedValue::Px(em));
    properties
        .entry(PropertyId::MarginBottom)
        .or_insert(ComputedValue::Px(em));
}

/// `<th>`'s UA default: bold, centered table-cell text.
fn apply_th_ua_defaults(properties: &mut PropertyMap) {
    properties
        .entry(PropertyId::FontWeight)
        .or_insert(ComputedValue::Keyword("bold".to_string()));
    properties
        .entry(PropertyId::TextAlign)
        .or_insert(ComputedValue::Keyword("center".to_string()));
    properties
        .entry(PropertyId::Display)
        .or_insert(ComputedValue::Keyword("table-cell".to_string()));
}

/// `<sub>`/`<sup>`/`<small>`'s shared UA default: all three render inline at
/// 0.833x the parent font size; `sub`/`sup` additionally set
/// `vertical-align` (`vertical_align` is `None` for `small`).
fn apply_scaled_inline_ua_defaults(
    properties: &mut PropertyMap,
    parent_font_size: f32,
    vertical_align: Option<&'static str>,
) {
    properties
        .entry(PropertyId::Display)
        .or_insert(ComputedValue::Keyword("inline".to_string()));
    if let Some(vertical_align) = vertical_align {
        properties
            .entry(PropertyId::VerticalAlign)
            .or_insert(ComputedValue::Keyword(vertical_align.to_string()));
    }
    let smaller = parent_font_size * 0.833;
    properties
        .entry(PropertyId::FontSize)
        .or_insert(ComputedValue::Px(smaller));
}

fn collect_builtin_ua_candidates(
    node: &NodeHandle,
    pseudo: Option<PseudoElement>,
    source_order: &mut usize,
    candidates: &mut Vec<Candidate>,
) {
    let html_namespace = node.namespace_uri().map_or_else(
        || node.is_html_element(),
        |namespace| namespace == "http://www.w3.org/1999/xhtml",
    );
    if !html_namespace {
        return;
    }
    if let Some(pseudo) = pseudo {
        if node.local_name().as_deref() == Some("q") {
            let keyword = match pseudo {
                PseudoElement::Before => Some("open-quote"),
                PseudoElement::After => Some("close-quote"),
                _ => None,
            };
            if let Some(keyword) = keyword {
                let rule_order = *source_order;
                push_builtin_ua_candidate(
                    "content".to_owned(),
                    Value::Keyword(keyword.to_owned()),
                    Specificity {
                        ids: 0,
                        classes: 0,
                        elements: 2,
                    },
                    rule_order,
                    source_order,
                    candidates,
                );
            }
        }
        return;
    }
    if let Some(hidden) = node.get_attribute("hidden") {
        let (name, keyword) = if hidden.eq_ignore_ascii_case("until-found") {
            ("content-visibility", "hidden")
        } else {
            ("display", "none")
        };
        let rule_order = *source_order;
        push_builtin_ua_candidate(
            name.to_owned(),
            Value::Keyword(keyword.to_owned()),
            Specificity {
                ids: 0,
                classes: 1,
                elements: 0,
            },
            rule_order,
            source_order,
            candidates,
        );
    }
    if node.local_name().as_deref() == Some("body") {
        let rule_order = *source_order;
        for side in ["top", "right", "bottom", "left"] {
            push_builtin_ua_candidate(
                format!("margin-{side}"),
                Value::Length(8.0, "px".to_owned()),
                Specificity {
                    ids: 0,
                    classes: 0,
                    elements: 1,
                },
                rule_order,
                source_order,
                candidates,
            );
        }
    }
}

// Browser defaults participate at UA origin, so author rules and `revert`
// expose the appropriate value without adding rules to author CSSOM.
fn push_builtin_ua_candidate(
    name: String,
    value: Value,
    specificity: Specificity,
    rule_order: usize,
    source_order: &mut usize,
    candidates: &mut Vec<Candidate>,
) {
    candidates.push(Candidate {
        name,
        prefixed_alias: false,
        value,
        important: false,
        origin: Origin::UserAgent,
        inline: false,
        specificity,
        scope_proximity: None,
        source_order: *source_order,
        rule_order,
        encapsulation_order: 0,
        layer_context: LayerContextKey {
            origin: Origin::UserAgent,
            scope_root: None,
        },
        layer_path: None,
        layer_order: vec![usize::MAX],
    });
    *source_order += 1;
}

/// A property's CSS initial value, expressed so it can live in a `static`
/// table. [`InitialValue::resolve`] converts each variant into the concrete
/// [`ComputedValue`] `apply_initial_values` inserts when the cascade left a
/// property unset.
enum InitialValue {
    /// A CSS keyword, e.g. `none` or `auto`.
    Keyword(&'static str),
    /// A pixel length.
    Px(f32),
    /// A plain number, e.g. `orphans`/`widows`.
    Number(f32),
    /// A named color.
    Color(&'static str),
    /// A `<position>` whose x/y components are both percentages.
    Position(f32, f32),
    /// The initial value is `currentcolor`, resolved against the element's
    /// already-computed `color` (itself possibly just defaulted). Rows with
    /// this variant are applied in a second pass, once `color` is settled.
    CurrentColor,
}

impl InitialValue {
    /// Converts a non-[`InitialValue::CurrentColor`] row into a
    /// [`ComputedValue`]. Callers filter `CurrentColor` out beforehand and
    /// resolve it separately with the element's computed `color`.
    fn resolve(&self) -> ComputedValue {
        match self {
            InitialValue::Keyword(keyword) => ComputedValue::Keyword((*keyword).to_string()),
            InitialValue::Px(px) => ComputedValue::Px(*px),
            InitialValue::Number(n) => ComputedValue::Number(*n),
            InitialValue::Color(color) => ComputedValue::Color((*color).to_string()),
            InitialValue::Position(x, y) => ComputedValue::Position {
                x: Box::new(ComputedValue::Percentage(*x)),
                y: Box::new(ComputedValue::Percentage(*y)),
            },
            InitialValue::CurrentColor => {
                unreachable!("CurrentColor rows are resolved separately, see apply_initial_values")
            }
        }
    }
}

/// CSS initial values keyed by property name, applied by `entry(name)
/// .or_insert_with(...)` so an already-resolved value is left untouched.
/// This intentionally excludes the handful of properties whose initial value
/// depends on another property's resolved value or on the number of layers
/// in a shorthand — those stay as explicit code in `apply_initial_values`.
const INITIAL_VALUES: &[(PropertyId, InitialValue)] = &[
    (PropertyId::AccentColor, InitialValue::Keyword("auto")),
    (PropertyId::Appearance, InitialValue::Keyword("none")),
    (PropertyId::CaretColor, InitialValue::Keyword("auto")),
    (PropertyId::ColorScheme, InitialValue::Keyword("normal")),
    (PropertyId::FieldSizing, InitialValue::Keyword("fixed")),
    (PropertyId::Resize, InitialValue::Keyword("none")),
    (PropertyId::TouchAction, InitialValue::Keyword("auto")),
    (PropertyId::UserSelect, InitialValue::Keyword("auto")),
    (PropertyId::Translate, InitialValue::Keyword("none")),
    (PropertyId::Rotate, InitialValue::Keyword("none")),
    (PropertyId::Scale, InitialValue::Keyword("none")),
    (PropertyId::Order, InitialValue::Number(0.0)),
    (PropertyId::FlexGrow, InitialValue::Number(0.0)),
    (PropertyId::FlexShrink, InitialValue::Number(1.0)),
    (PropertyId::GridAutoFlow, InitialValue::Keyword("row")),
    (PropertyId::GridAutoColumns, InitialValue::Keyword("auto")),
    (PropertyId::GridAutoRows, InitialValue::Keyword("auto")),
    (PropertyId::CounterReset, InitialValue::Keyword("none")),
    (PropertyId::CounterSet, InitialValue::Keyword("none")),
    (PropertyId::Quotes, InitialValue::Keyword("auto")),
    (PropertyId::CounterIncrement, InitialValue::Keyword("none")),
    (PropertyId::ShapeOutside, InitialValue::Keyword("none")),
    (PropertyId::ShapeMargin, InitialValue::Px(0.0)),
    (PropertyId::BorderImageSource, InitialValue::Keyword("none")),
    (PropertyId::BorderImageSlice, InitialValue::Keyword("100%")),
    (PropertyId::BorderImageWidth, InitialValue::Number(1.0)),
    (PropertyId::BorderImageOutset, InitialValue::Number(0.0)),
    (
        PropertyId::BorderImageRepeat,
        InitialValue::Keyword("stretch"),
    ),
    (
        PropertyId::BackgroundBlendMode,
        InitialValue::Keyword("normal"),
    ),
    (
        PropertyId::BackgroundClip,
        InitialValue::Keyword("border-box"),
    ),
    (
        PropertyId::BackgroundColor,
        InitialValue::Color("transparent"),
    ),
    (
        PropertyId::BackgroundOrigin,
        InitialValue::Keyword("padding-box"),
    ),
    (PropertyId::Color, InitialValue::Color("black")),
    (PropertyId::FontSize, InitialValue::Px(16.0)),
    (
        PropertyId::FontFeatureSettings,
        InitialValue::Keyword("normal"),
    ),
    (
        PropertyId::FontVariationSettings,
        InitialValue::Keyword("normal"),
    ),
    (PropertyId::FontPalette, InitialValue::Keyword("normal")),
    (
        PropertyId::ImageOrientation,
        InitialValue::Keyword("from-image"),
    ),
    (PropertyId::ImageRendering, InitialValue::Keyword("auto")),
    (PropertyId::FontKerning, InitialValue::Keyword("auto")),
    (PropertyId::FontOpticalSizing, InitialValue::Keyword("auto")),
    (PropertyId::FontSizeAdjust, InitialValue::Keyword("none")),
    (
        PropertyId::FontSynthesis,
        InitialValue::Keyword("weight style small-caps position"),
    ),
    (
        PropertyId::FontVariantAlternates,
        InitialValue::Keyword("normal"),
    ),
    (PropertyId::FontVariantCaps, InitialValue::Keyword("normal")),
    (
        PropertyId::FontVariantEastAsian,
        InitialValue::Keyword("normal"),
    ),
    (
        PropertyId::FontVariantLigatures,
        InitialValue::Keyword("normal"),
    ),
    (
        PropertyId::FontVariantNumeric,
        InitialValue::Keyword("normal"),
    ),
    (
        PropertyId::FontVariantPosition,
        InitialValue::Keyword("normal"),
    ),
    (PropertyId::Direction, InitialValue::Keyword("ltr")),
    (
        PropertyId::WritingMode,
        InitialValue::Keyword("horizontal-tb"),
    ),
    (PropertyId::UnicodeBidi, InitialValue::Keyword("normal")),
    (PropertyId::TextOrientation, InitialValue::Keyword("mixed")),
    (
        PropertyId::TextCombineUpright,
        InitialValue::Keyword("none"),
    ),
    (PropertyId::RubyPosition, InitialValue::Keyword("alternate")),
    (PropertyId::RubyAlign, InitialValue::Keyword("space-around")),
    (PropertyId::TextAutospace, InitialValue::Keyword("normal")),
    (PropertyId::TextSpacingTrim, InitialValue::Keyword("normal")),
    (PropertyId::TextEmphasisStyle, InitialValue::Keyword("none")),
    (
        PropertyId::TextEmphasisPosition,
        InitialValue::Keyword("over"),
    ),
    (PropertyId::TextEmphasisColor, InitialValue::CurrentColor),
    (PropertyId::TextTransform, InitialValue::Keyword("none")),
    (PropertyId::TextIndent, InitialValue::Px(0.0)),
    (PropertyId::WordSpacing, InitialValue::Px(0.0)),
    (PropertyId::WhiteSpace, InitialValue::Keyword("normal")),
    (
        PropertyId::WhiteSpaceCollapse,
        InitialValue::Keyword("collapse"),
    ),
    (PropertyId::TextWrapMode, InitialValue::Keyword("wrap")),
    (PropertyId::TextWrapStyle, InitialValue::Keyword("auto")),
    (PropertyId::LineBreak, InitialValue::Keyword("auto")),
    (PropertyId::Hyphens, InitialValue::Keyword("manual")),
    (
        PropertyId::HangingPunctuation,
        InitialValue::Keyword("none"),
    ),
    (
        PropertyId::HyphenateCharacter,
        InitialValue::Keyword("auto"),
    ),
    (PropertyId::TabSize, InitialValue::Number(8.0)),
    (PropertyId::TextAlignLast, InitialValue::Keyword("auto")),
    (PropertyId::TextJustify, InitialValue::Keyword("auto")),
    (
        PropertyId::BlockEllipsis,
        InitialValue::Keyword("no-ellipsis"),
    ),
    (PropertyId::Continue, InitialValue::Keyword("normal")),
    (PropertyId::MaxLines, InitialValue::Keyword("auto")),
    (PropertyId::InitialLetter, InitialValue::Keyword("normal")),
    (PropertyId::TextBoxTrim, InitialValue::Keyword("none")),
    (PropertyId::TextBoxEdge, InitialValue::Keyword("auto")),
    (PropertyId::TextOverflow, InitialValue::Keyword("clip")),
    (PropertyId::OverflowX, InitialValue::Keyword("visible")),
    (PropertyId::OverflowY, InitialValue::Keyword("visible")),
    (PropertyId::OverflowAnchor, InitialValue::Keyword("auto")),
    (PropertyId::OverflowClipMargin, InitialValue::Px(0.0)),
    (PropertyId::TextShadow, InitialValue::Keyword("none")),
    (
        PropertyId::TextDecorationLine,
        InitialValue::Keyword("none"),
    ),
    (
        PropertyId::TextDecorationSkipInk,
        InitialValue::Keyword("auto"),
    ),
    (
        PropertyId::TextDecorationStyle,
        InitialValue::Keyword("solid"),
    ),
    // `text-decoration-color`'s initial value is `currentcolor`.
    (PropertyId::TextDecorationColor, InitialValue::CurrentColor),
    (
        PropertyId::TextDecorationThickness,
        InitialValue::Keyword("auto"),
    ),
    (
        PropertyId::TextUnderlinePosition,
        InitialValue::Keyword("auto"),
    ),
    (
        PropertyId::TextUnderlineOffset,
        InitialValue::Keyword("auto"),
    ),
    // `cursor` initial value is `auto` (CSS UI). Ensuring it is always
    // present lets a dropped/absent `cursor` declaration serialize as `auto`
    // in getComputedStyle (Acid3 test 47).
    (PropertyId::ForcedColorAdjust, InitialValue::Keyword("auto")),
    (PropertyId::WillChange, InitialValue::Keyword("auto")),
    (PropertyId::Cursor, InitialValue::Keyword("auto")),
    (PropertyId::PointerEvents, InitialValue::Keyword("auto")),
    (PropertyId::ScrollBehavior, InitialValue::Keyword("auto")),
    (PropertyId::ScrollbarColor, InitialValue::Keyword("auto")),
    (PropertyId::ScrollbarGutter, InitialValue::Keyword("auto")),
    (PropertyId::ScrollbarWidth, InitialValue::Keyword("auto")),
    (PropertyId::TableLayout, InitialValue::Keyword("auto")),
    (
        PropertyId::OverscrollBehaviorBlock,
        InitialValue::Keyword("auto"),
    ),
    (
        PropertyId::OverscrollBehaviorInline,
        InitialValue::Keyword("auto"),
    ),
    (
        PropertyId::OverscrollBehaviorX,
        InitialValue::Keyword("auto"),
    ),
    (
        PropertyId::OverscrollBehaviorY,
        InitialValue::Keyword("auto"),
    ),
    (PropertyId::ScrollSnapType, InitialValue::Keyword("none")),
    (PropertyId::ScrollSnapAlign, InitialValue::Keyword("none")),
    (PropertyId::Position, InitialValue::Keyword("static")),
    (PropertyId::Contain, InitialValue::Keyword("none")),
    (
        PropertyId::ContentVisibility,
        InitialValue::Keyword("visible"),
    ),
    (
        PropertyId::ContainIntrinsicWidth,
        InitialValue::Keyword("none"),
    ),
    (
        PropertyId::ContainIntrinsicHeight,
        InitialValue::Keyword("none"),
    ),
    (
        PropertyId::ContainIntrinsicInlineSize,
        InitialValue::Keyword("none"),
    ),
    (
        PropertyId::ContainIntrinsicBlockSize,
        InitialValue::Keyword("none"),
    ),
    (PropertyId::ContainerName, InitialValue::Keyword("none")),
    (PropertyId::ContainerType, InitialValue::Keyword("normal")),
    (PropertyId::ColumnCount, InitialValue::Keyword("auto")),
    (PropertyId::ColumnWidth, InitialValue::Keyword("auto")),
    (PropertyId::ColumnGap, InitialValue::Keyword("normal")),
    (PropertyId::ColumnFill, InitialValue::Keyword("balance")),
    (PropertyId::ColumnSpan, InitialValue::Keyword("none")),
    (
        PropertyId::BoxDecorationBreak,
        InitialValue::Keyword("slice"),
    ),
    (PropertyId::ColumnRuleStyle, InitialValue::Keyword("none")),
    (PropertyId::ColumnRuleWidth, InitialValue::Px(3.0)),
    // `column-rule-color`'s initial value is `currentcolor`.
    (PropertyId::ColumnRuleColor, InitialValue::CurrentColor),
    (PropertyId::OutlineStyle, InitialValue::Keyword("none")),
    (PropertyId::OutlineWidth, InitialValue::Px(3.0)),
    (PropertyId::OutlineOffset, InitialValue::Px(0.0)),
    (PropertyId::OutlineColor, InitialValue::Keyword("auto")),
    (PropertyId::BreakBefore, InitialValue::Keyword("auto")),
    (PropertyId::BreakAfter, InitialValue::Keyword("auto")),
    (PropertyId::BreakInside, InitialValue::Keyword("auto")),
    (PropertyId::Page, InitialValue::Keyword("auto")),
    (PropertyId::Orphans, InitialValue::Number(2.0)),
    (PropertyId::Widows, InitialValue::Number(2.0)),
    // CSS Sizing: `aspect-ratio` is `auto`, meaning "use the intrinsic ratio".
    (PropertyId::AspectRatio, InitialValue::Keyword("auto")),
    // CSS Images: `object-fit` is `fill` and `object-position` is `50% 50%`.
    // Keeping them present lets getComputedStyle serialize the initial value
    // even when nothing declares them.
    (PropertyId::ObjectFit, InitialValue::Keyword("fill")),
    (
        PropertyId::ObjectPosition,
        InitialValue::Position(50.0, 50.0),
    ),
    // CSS Masking initial values. `none` is an identity mask in the paint
    // implementation; match-source resolves gradients/images through their
    // alpha channel and keeps SVG/image defaults deterministic.
    (PropertyId::MaskImage, InitialValue::Keyword("none")),
    (PropertyId::MaskMode, InitialValue::Keyword("match-source")),
    (PropertyId::MaskComposite, InitialValue::Keyword("add")),
    (PropertyId::MaskBorderSource, InitialValue::Keyword("none")),
    (PropertyId::MaskBorderSlice, InitialValue::Number(0.0)),
    (PropertyId::MaskBorderWidth, InitialValue::Keyword("auto")),
    (PropertyId::MaskBorderOutset, InitialValue::Number(0.0)),
    (
        PropertyId::MaskBorderRepeat,
        InitialValue::Keyword("stretch"),
    ),
    (PropertyId::MaskBorderMode, InitialValue::Keyword("alpha")),
    (PropertyId::Transform, InitialValue::Keyword("none")),
    (
        PropertyId::TransformOrigin,
        InitialValue::Keyword("50% 50%"),
    ),
    (PropertyId::Perspective, InitialValue::Keyword("none")),
    (
        PropertyId::PerspectiveOrigin,
        InitialValue::Keyword("50% 50%"),
    ),
    (PropertyId::TransformStyle, InitialValue::Keyword("flat")),
    (
        PropertyId::BackfaceVisibility,
        InitialValue::Keyword("visible"),
    ),
    (PropertyId::MixBlendMode, InitialValue::Keyword("normal")),
    (PropertyId::Isolation, InitialValue::Keyword("auto")),
    (PropertyId::TransitionProperty, InitialValue::Keyword("all")),
    (PropertyId::TransitionDuration, InitialValue::Keyword("0s")),
    (
        PropertyId::TransitionTimingFunction,
        InitialValue::Keyword("ease"),
    ),
    (PropertyId::TransitionDelay, InitialValue::Keyword("0s")),
];

fn apply_initial_values(properties: &mut PropertyMap) {
    for (name, initial) in INITIAL_VALUES {
        if matches!(initial, InitialValue::CurrentColor) {
            continue;
        }
        if !properties.contains_key(name) {
            properties.entry(*name).or_insert_with(|| initial.resolve());
        }
    }
    // `currentcolor`-valued initial values are resolved once `color` (set by
    // the author or defaulted above) is known.
    for (name, initial) in INITIAL_VALUES {
        if matches!(initial, InitialValue::CurrentColor) {
            if !properties.contains_key(name) {
                properties.copy_property("color", name.as_str());
            }
        }
    }
    for (padding, margin) in [
        (PropertyId::ScrollPaddingTop, PropertyId::ScrollMarginTop),
        (
            PropertyId::ScrollPaddingRight,
            PropertyId::ScrollMarginRight,
        ),
        (
            PropertyId::ScrollPaddingBottom,
            PropertyId::ScrollMarginBottom,
        ),
        (PropertyId::ScrollPaddingLeft, PropertyId::ScrollMarginLeft),
        (
            PropertyId::ScrollPaddingInlineStart,
            PropertyId::ScrollMarginInlineStart,
        ),
        (
            PropertyId::ScrollPaddingInlineEnd,
            PropertyId::ScrollMarginInlineEnd,
        ),
        (
            PropertyId::ScrollPaddingBlockStart,
            PropertyId::ScrollMarginBlockStart,
        ),
        (
            PropertyId::ScrollPaddingBlockEnd,
            PropertyId::ScrollMarginBlockEnd,
        ),
    ] {
        properties
            .entry(padding)
            .or_insert_with(|| ComputedValue::Keyword("auto".to_string()));
        properties.entry(margin).or_insert(ComputedValue::Px(0.0));
    }
    let intrinsic_width = properties
        .get(&PropertyId::ContainIntrinsicWidth)
        .map(computed_value_css_text)
        .unwrap_or_else(|| "none".to_string());
    let intrinsic_height = properties
        .get(&PropertyId::ContainIntrinsicHeight)
        .map(computed_value_css_text)
        .unwrap_or_else(|| "none".to_string());
    properties.insert(
        "contain-intrinsic-size".to_string(),
        ComputedValue::Keyword(if intrinsic_width == intrinsic_height {
            intrinsic_width
        } else {
            format!("{intrinsic_width} {intrinsic_height}")
        }),
    );
}

fn normalize_background_layer_lists(properties: &mut PropertyMap) {
    let image_count = properties
        .get(&PropertyId::BackgroundImage)
        .map(computed_value_css_text)
        .map(|value| super::split_top_level_commas(&value).len())
        .unwrap_or(1);
    for (name, default) in [
        ("background-position-x", "0%"),
        ("background-position-y", "0%"),
        ("background-size", "auto"),
        ("background-repeat", "repeat"),
        ("background-attachment", "scroll"),
        ("background-origin", "padding-box"),
        ("background-clip", "border-box"),
    ] {
        let raw = properties
            .get(name)
            .map(computed_value_css_text)
            .unwrap_or_else(|| default.to_string());
        let values = super::split_top_level_commas(&raw);
        if image_count == 1 && values.len() == 1 {
            continue;
        }
        let normalized = (0..image_count)
            .map(|index| values[index % values.len()].trim())
            .collect::<Vec<_>>()
            .join(", ");
        properties.insert(name.to_string(), ComputedValue::Keyword(normalized));
    }
}

fn computed_value_css_text(value: &ComputedValue) -> String {
    value.css_text()
}

fn resolve_initial_css_wide_keywords(properties: &mut PropertyMap) {
    let initial_names: Vec<String> = properties
        .iter()
        .filter_map(|(name, value)| {
            matches!(
                value,
                ComputedValue::Keyword(keyword)
                    if matches!(
                        keyword.to_ascii_lowercase().as_str(),
                        "initial" | "unset" | "revert" | "revert-layer" | "revert-rule"
                    )
            )
            .then(|| name.to_string())
        })
        .collect();
    for name in initial_names {
        if is_margin_or_padding_longhand(&name) {
            properties.insert(name, ComputedValue::Px(0.0));
        } else {
            properties.remove(&name);
        }
    }
}

fn is_margin_or_padding_longhand(name: &str) -> bool {
    matches!(
        name,
        "margin-top"
            | "margin-right"
            | "margin-bottom"
            | "margin-left"
            | "margin-inline-start"
            | "margin-inline-end"
            | "margin-block-start"
            | "margin-block-end"
            | "padding-top"
            | "padding-right"
            | "padding-bottom"
            | "padding-left"
            | "padding-inline-start"
            | "padding-inline-end"
            | "padding-block-start"
            | "padding-block-end"
    )
}

/// CSS 2.1 §8.5.3: If border-style is 'none', the computed border-width is 0.
fn zero_border_width_for_none_style(properties: &mut PropertyMap) {
    for side in ["top", "right", "bottom", "left"] {
        let style_key = format!("border-{side}-style");
        let is_none = matches!(
            properties.get(&style_key),
            Some(ComputedValue::Keyword(keyword)) if matches!(keyword.to_ascii_lowercase().as_str(), "none" | "hidden")
        );
        if is_none {
            let width_key = format!("border-{side}-width");
            properties.insert(width_key, ComputedValue::Px(0.0));
        }
    }
}

/// `color: currentColor` is equivalent to `color: inherit` per CSS Color Level 4.
/// Resolve it before general inherit resolution so other properties that reference
/// currentColor can see the resolved color value.
fn resolve_current_color_on_color_property(
    properties: &mut PropertyMap,
    parent_style: Option<&ComputedStyle>,
) {
    let is_current_color = matches!(
        properties.get(&PropertyId::Color),
        Some(ComputedValue::Color(c)) if c.eq_ignore_ascii_case("currentcolor")
    ) || matches!(
        properties.get(&PropertyId::Color),
        Some(ComputedValue::Keyword(k)) if k.eq_ignore_ascii_case("currentcolor")
    );
    if is_current_color {
        if let Some(parent) = parent_style {
            if parent.get("color").is_some() {
                properties.insert_from("color", &parent.properties);
            } else {
                // Root element with color: currentColor → initial value (black)
                properties.insert(PropertyId::Color, ComputedValue::Color("black".to_string()));
            }
        } else {
            properties.insert(PropertyId::Color, ComputedValue::Color("black".to_string()));
        }
    }
}

fn resolve_column_rule_current_color(properties: &mut PropertyMap) {
    if properties.get(&PropertyId::Color).is_none() {
        return;
    }
    let is_current_color = matches!(
        properties.get(&PropertyId::ColumnRuleColor),
        Some(ComputedValue::Color(value) | ComputedValue::Keyword(value))
            if value.eq_ignore_ascii_case("currentcolor")
    );
    if is_current_color {
        properties.copy_property("color", "column-rule-color");
    }
}

fn resolve_accent_current_color(properties: &mut PropertyMap) {
    let is_current_color = matches!(
        properties.get(&PropertyId::AccentColor),
        Some(ComputedValue::Color(value) | ComputedValue::Keyword(value))
            if value.eq_ignore_ascii_case("currentcolor")
    );
    if is_current_color {
        properties.copy_property("color", "accent-color");
    }
}

fn resolve_outline_auto_color(properties: &mut PropertyMap) {
    let color_is_auto = matches!(
        properties.get(&PropertyId::OutlineColor),
        Some(ComputedValue::Keyword(value)) if value.eq_ignore_ascii_case("auto")
    );
    let style_is_auto = matches!(
        properties.get(&PropertyId::OutlineStyle),
        Some(ComputedValue::Keyword(value)) if value.eq_ignore_ascii_case("auto")
    );
    if color_is_auto && !style_is_auto {
        properties.copy_property("color", "outline-color");
    }
}

fn resolve_inherit_and_unset(properties: &mut PropertyMap, parent_style: Option<&ComputedStyle>) {
    let inherited_names: Vec<String> = properties
        .iter()
        .filter_map(|(name, value)| match value {
            ComputedValue::Keyword(keyword)
                if keyword.eq_ignore_ascii_case("inherit")
                    || (keyword.eq_ignore_ascii_case("unset") && is_inherited_property(name)) =>
            {
                Some(name.to_string())
            }
            _ => None,
        })
        .collect();

    for name in inherited_names {
        if let Some(parent_style) = parent_style
            && parent_style.get(&name).is_some()
        {
            properties.insert_from(&name, &parent_style.properties);
            continue;
        }
        properties.remove(&name);
    }
}

fn apply_inheritance(properties: &mut PropertyMap, parent_style: Option<&ComputedStyle>) {
    let Some(parent_style) = parent_style else {
        return;
    };

    for &id in INHERITED_PROPERTY_IDS {
        if !properties.contains_key(&id) {
            properties.insert_from_by_id(id, &parent_style.properties);
        }
    }

    // CSS custom properties inherit by default.
    for (name, value) in parent_style.properties.custom_properties() {
        if name.starts_with("--") && !properties.contains_key(name) {
            properties.insert(name, value.clone());
        }
    }
}

// Inherited CSS properties supported by this engine. Keeping this as shared
// metadata lets both natural inheritance and `all: unset` use the same rule.
const INHERITED_PROPERTY_IDS: &[PropertyId] = &[
    PropertyId::AccentColor,
    PropertyId::CaretColor,
    PropertyId::ColorScheme,
    PropertyId::ForcedColorAdjust,
    PropertyId::BorderCollapse,
    PropertyId::BorderSpacing,
    PropertyId::Color,
    PropertyId::Cursor,
    PropertyId::Direction,
    PropertyId::FontFamily,
    PropertyId::FontFeatureSettings,
    PropertyId::FontKerning,
    PropertyId::FontOpticalSizing,
    PropertyId::FontSize,
    PropertyId::FontSizeAdjust,
    PropertyId::FontPalette,
    PropertyId::FontStyle,
    PropertyId::FontStretch,
    PropertyId::FontSynthesis,
    PropertyId::FontVariantAlternates,
    PropertyId::FontVariantCaps,
    PropertyId::FontVariantEastAsian,
    PropertyId::FontVariantLigatures,
    PropertyId::FontVariantNumeric,
    PropertyId::FontVariantPosition,
    PropertyId::FontVariationSettings,
    PropertyId::FontWeight,
    PropertyId::ImageOrientation,
    PropertyId::ImageRendering,
    PropertyId::LetterSpacing,
    PropertyId::LineBreak,
    PropertyId::LineHeight,
    PropertyId::ListStyleImage,
    PropertyId::ListStylePosition,
    PropertyId::ListStyleType,
    PropertyId::OverflowWrap,
    PropertyId::Orphans,
    PropertyId::PointerEvents,
    PropertyId::Quotes,
    PropertyId::RubyAlign,
    PropertyId::RubyPosition,
    PropertyId::ScrollbarColor,
    PropertyId::ScrollbarWidth,
    PropertyId::TextAlign,
    PropertyId::TextAlignLast,
    PropertyId::TextAutospace,
    PropertyId::TextBoxEdge,
    PropertyId::TextDecorationSkipInk,
    PropertyId::TextEmphasisColor,
    PropertyId::TextEmphasisPosition,
    PropertyId::TextEmphasisStyle,
    PropertyId::TextIndent,
    PropertyId::TextJustify,
    PropertyId::TextCombineUpright,
    PropertyId::TextOrientation,
    PropertyId::TextSpacingTrim,
    PropertyId::TextWrapMode,
    PropertyId::TextWrapStyle,
    PropertyId::TextShadow,
    PropertyId::TextTransform,
    PropertyId::Visibility,
    PropertyId::WhiteSpace,
    PropertyId::WhiteSpaceCollapse,
    PropertyId::WritingMode,
    PropertyId::TextUnderlinePosition,
    PropertyId::TextUnderlineOffset,
    PropertyId::WordBreak,
    PropertyId::WordSpacing,
    PropertyId::Widows,
    PropertyId::HyphenateCharacter,
    PropertyId::Hyphens,
    PropertyId::HangingPunctuation,
    PropertyId::TabSize,
];

fn is_inherited_property(name: &str) -> bool {
    name.starts_with("--") || INHERITED_PROPERTY_IDS.iter().any(|id| id.as_str() == name)
}

fn inherited_font_size(parent_style: Option<&ComputedStyle>, current: &PropertyMap) -> f32 {
    if let Some(ComputedValue::Px(value)) = current.get("font-size") {
        return *value;
    }
    if let Some(parent_style) = parent_style
        && let Some(ComputedValue::Px(value)) = parent_style.get("font-size")
    {
        return *value;
    }
    16.0
}

fn used_line_height(style: Option<&ComputedStyle>, font_size: f32) -> f32 {
    used_line_height_value(style.and_then(|style| style.get("line-height")), font_size)
}

fn used_line_height_value(value: Option<&ComputedValue>, font_size: f32) -> f32 {
    match value {
        Some(ComputedValue::Px(px)) => *px,
        Some(ComputedValue::Number(multiplier)) => multiplier * font_size,
        Some(ComputedValue::Percentage(percent)) => percent * font_size / 100.0,
        _ => font_size * 1.2,
    }
}

fn candidate_font_properties(
    candidates: &[Candidate],
    custom_properties: &BTreeMap<String, Value>,
    parent_style: Option<&ComputedStyle>,
    node: &NodeHandle,
) -> (PropertyMap, Option<usize>) {
    let mut properties = PropertyMap::new();
    let mut scope_root = parent_style.and_then(ComputedStyle::font_family_scope_root);
    for name in [
        "font-family",
        "font-weight",
        "font-style",
        "font-stretch",
        "writing-mode",
        "text-orientation",
    ] {
        if let Some(value) = parent_style.and_then(|style| style.get(name)) {
            properties.insert(name.to_string(), value.clone());
        }
        let Some(candidate) = candidates.iter().rfind(|candidate| candidate.name == name) else {
            continue;
        };
        let Some(value) = resolve_value_with_custom_properties(&candidate.value, custom_properties)
        else {
            continue;
        };
        let value = if sibling_functions::contains(&value) {
            sibling_functions::resolve(&value, node)
        } else {
            value
        };
        if let Value::Keyword(keyword) = &value {
            if matches!(keyword.to_ascii_lowercase().as_str(), "inherit" | "unset") {
                continue;
            }
            if matches!(keyword.to_ascii_lowercase().as_str(), "initial" | "revert") {
                properties.remove(name);
                continue;
            }
        }
        if name == "font-family" {
            scope_root =
                font_reference_scope_root(&value, candidate.layer_context.scope_root, parent_style);
        }
        properties.insert(
            name.to_string(),
            compute_value(&value, name, ResolutionContext::default()),
        );
    }
    (properties, scope_root)
}

fn value_has_font_metric_unit(value: &Value) -> bool {
    match value {
        Value::Length(_, unit) => matches!(
            unit.to_ascii_lowercase().as_str(),
            "ex" | "ch" | "cap" | "ic"
        ),
        Value::Function { arguments, .. }
        | Value::List(arguments)
        | Value::CommaList(arguments) => arguments.iter().any(value_has_font_metric_unit),
        _ => false,
    }
}

pub(crate) fn is_color_keyword(keyword: &str) -> bool {
    if keyword.eq_ignore_ascii_case("currentcolor")
        || crate::css::ForcedColorPalette::for_color_scheme(false)
            .system_color(keyword)
            .is_some()
    {
        return true;
    }
    matches!(
        keyword,
        "black"
            | "white"
            | "red"
            | "green"
            | "blue"
            | "gray"
            | "grey"
            | "silver"
            | "aqua"
            | "teal"
            | "lime"
            | "fuchsia"
            | "olive"
            | "navy"
            | "purple"
            | "maroon"
            | "yellow"
            | "orange"
            | "coral"
            | "salmon"
            | "tomato"
            | "orangered"
            | "darkorange"
            | "gold"
            | "goldenrod"
            | "darkgoldenrod"
            | "peru"
            | "chocolate"
            | "sienna"
            | "saddlebrown"
            | "brown"
            | "firebrick"
            | "darkred"
            | "crimson"
            | "pink"
            | "lightpink"
            | "hotpink"
            | "deeppink"
            | "palevioletred"
            | "mediumvioletred"
            | "lavender"
            | "thistle"
            | "plum"
            | "violet"
            | "orchid"
            | "magenta"
            | "mediumorchid"
            | "darkorchid"
            | "darkviolet"
            | "blueviolet"
            | "indigo"
            | "slateblue"
            | "darkslateblue"
            | "mediumpurple"
            | "rebeccapurple"
            | "lightblue"
            | "powderblue"
            | "lightskyblue"
            | "skyblue"
            | "deepskyblue"
            | "dodgerblue"
            | "cornflowerblue"
            | "steelblue"
            | "royalblue"
            | "mediumblue"
            | "darkblue"
            | "midnightblue"
            | "azure"
            | "aliceblue"
            | "ghostwhite"
            | "mintcream"
            | "honeydew"
            | "lightgreen"
            | "palegreen"
            | "limegreen"
            | "mediumseagreen"
            | "seagreen"
            | "forestgreen"
            | "darkgreen"
            | "yellowgreen"
            | "olivedrab"
            | "darkolivegreen"
            | "mediumaquamarine"
            | "aquamarine"
            | "turquoise"
            | "mediumturquoise"
            | "darkturquoise"
            | "lightseagreen"
            | "cadetblue"
            | "darkcyan"
            | "cyan"
            | "darkslategray"
            | "darkslategrey"
            | "slategray"
            | "slategrey"
            | "lightslategray"
            | "lightslategrey"
            | "darkgray"
            | "darkgrey"
            | "dimgray"
            | "dimgrey"
            | "lightgray"
            | "lightgrey"
            | "gainsboro"
            | "whitesmoke"
            | "snow"
            | "seashell"
            | "floralwhite"
            | "ivory"
            | "linen"
            | "oldlace"
            | "antiquewhite"
            | "bisque"
            | "blanchedalmond"
            | "wheat"
            | "moccasin"
            | "navajowhite"
            | "peachpuff"
            | "mistyrose"
            | "papayawhip"
            | "lightyellow"
            | "lemonchiffon"
            | "khaki"
            | "darkkhaki"
            | "palegoldenrod"
            | "beige"
            | "cornsilk"
            | "chartreuse"
            | "greenyellow"
            | "lawngreen"
            | "springgreen"
            | "mediumspringgreen"
            | "transparent"
    )
}

/// Extracts a numeric channel value from a CSS `Value`.
/// Handles `Value::Number` directly and `Value::Percentage` by clamping to 0–255.
fn extract_channel(v: &Value) -> Option<f32> {
    match v {
        Value::Number(n) => Some(*n),
        Value::Percentage(p) => Some(p * 255.0 / 100.0),
        _ => None,
    }
}

/// Extracts an alpha value (0.0–1.0) from a CSS `Value`.
fn extract_alpha(v: &Value) -> Option<f32> {
    match v {
        Value::Number(n) => Some(n.clamp(0.0, 1.0)),
        Value::Percentage(p) => Some((p / 100.0).clamp(0.0, 1.0)),
        _ => None,
    }
}

/// Flattens function arguments by expanding a single-argument `Value::List`.
///
/// Modern CSS color syntax `rgb(r g b / a)` is parsed as one argument that is
/// a `Value::List`.  This helper normalises both forms — comma-separated and
/// space-separated — into a flat slice.
fn flatten_color_args(arguments: &[Value]) -> Vec<&Value> {
    if arguments.len() == 1
        && let Value::List(items) = &arguments[0]
    {
        return items.iter().collect();
    }
    arguments.iter().collect()
}

/// Converts an `rgb()` or `rgba()` argument list into a hex color string.
///
/// Handles both the legacy comma-separated syntax and the modern
/// space-separated syntax with an optional `/ alpha` component.
fn compute_rgb_function(arguments: &[Value]) -> Option<String> {
    let flat = flatten_color_args(arguments);
    let (rgb_values, alpha) = split_slash(&flat);

    // rgb_values are the channels before "/"
    let channels: Vec<f32> = rgb_values
        .iter()
        .filter_map(|v| extract_channel(v))
        .collect();

    // Use the 4th value as alpha for rgba(r,g,b,a) comma form.
    // Extract via extract_alpha (not extract_channel) so percentages are 0-1.
    let a = alpha.or_else(|| {
        let flat = flatten_color_args(arguments);
        flat.get(3).and_then(|v| extract_alpha(v))
    });

    let (r, g, b) = match channels.as_slice() {
        [r, g, b] | [r, g, b, _] => (
            r.round().clamp(0.0, 255.0) as u8,
            g.round().clamp(0.0, 255.0) as u8,
            b.round().clamp(0.0, 255.0) as u8,
        ),
        _ => return None,
    };

    if let Some(alpha) = a.filter(|alpha| *alpha < 1.0) {
        Some(format!("rgba({r}, {g}, {b}, {})", alpha.clamp(0.0, 1.0)))
    } else {
        format_color_hex(r, g, b, a)
    }
}

/// Converts an `hsl()` or `hsla()` argument list into a hex color string.
fn compute_hsl_function(arguments: &[Value]) -> Option<String> {
    let flat = flatten_color_args(arguments);
    let (hsl_values, alpha) = split_slash(&flat);

    let numbers: Vec<f32> = hsl_values
        .iter()
        .filter_map(|v| match v {
            Value::Number(n) => Some(*n),
            Value::Percentage(p) => Some(*p),
            _ => None,
        })
        .collect();

    // Use 4th value as alpha for hsla(h,s%,l%,a) comma form.
    // Extract via extract_alpha so percentages are 0-1.
    let a = alpha.or_else(|| flat.get(3).and_then(|v| extract_alpha(v)));

    let (h, s, l) = match numbers.as_slice() {
        [h, s, l] | [h, s, l, _] => (*h, *s, *l),
        _ => return None,
    };

    let (r, g, b) = hsl_to_rgb(h, s / 100.0, l / 100.0);
    format_color_hex(r, g, b, a)
}

/// Formats an RGBA color as a hex string.
///
/// Omits the alpha byte when fully opaque to produce the shorter `#rrggbb` form.
fn format_color_hex(r: u8, g: u8, b: u8, a: Option<f32>) -> Option<String> {
    match a {
        Some(a) if a < 1.0 - f32::EPSILON => {
            let a_byte = (a * 255.0).round() as u8;
            Some(format!("#{r:02x}{g:02x}{b:02x}{a_byte:02x}"))
        }
        _ => Some(format!("#{r:02x}{g:02x}{b:02x}")),
    }
}

/// Splits a flat argument list around the `/` keyword into the before and after parts.
///
/// Returns the values before `/`, and the alpha value after `/` (if any).
fn split_slash<'a>(flat: &[&'a Value]) -> (Vec<&'a Value>, Option<f32>) {
    let slash_pos = flat
        .iter()
        .position(|v| matches!(v, Value::Keyword(k) if k == "/"));

    if let Some(pos) = slash_pos {
        let before = flat[..pos].to_vec();
        let alpha = flat.get(pos + 1).and_then(|v| extract_alpha(v));
        (before, alpha)
    } else {
        (flat.to_vec(), None)
    }
}

/// Converts HSL to RGB.  All inputs and outputs are in the 0–255 / 0–360 range.
///
/// - `h`: hue in degrees (0–360)
/// - `s`: saturation as fraction (0.0–1.0)
/// - `l`: lightness as fraction (0.0–1.0)
pub(crate) fn hsl_to_rgb(h: f32, s: f32, l: f32) -> (u8, u8, u8) {
    // CSS allows hue values outside 0-360; wrap to canonical range
    let h = ((h % 360.0) + 360.0) % 360.0;
    let s = s.clamp(0.0, 1.0);
    let l = l.clamp(0.0, 1.0);

    if s == 0.0 {
        let v = (l * 255.0).round() as u8;
        return (v, v, v);
    }

    let q = if l < 0.5 {
        l * (1.0 + s)
    } else {
        l + s - l * s
    };
    let p = 2.0 * l - q;
    let h = h / 360.0;

    let r = hue_to_rgb(p, q, h + 1.0 / 3.0);
    let g = hue_to_rgb(p, q, h);
    let b = hue_to_rgb(p, q, h - 1.0 / 3.0);

    (
        (r * 255.0).round() as u8,
        (g * 255.0).round() as u8,
        (b * 255.0).round() as u8,
    )
}

fn hue_to_rgb(p: f32, q: f32, mut t: f32) -> f32 {
    if t < 0.0 {
        t += 1.0;
    }
    if t > 1.0 {
        t -= 1.0;
    }
    if t < 1.0 / 6.0 {
        return p + (q - p) * 6.0 * t;
    }
    if t < 1.0 / 2.0 {
        return q;
    }
    if t < 2.0 / 3.0 {
        return p + (q - p) * (2.0 / 3.0 - t) * 6.0;
    }
    p
}

pub(super) fn render_value(value: &Value) -> String {
    match value {
        Value::TreeCounting { index, .. } => if *index {
            "sibling-index()"
        } else {
            "sibling-count()"
        }
        .to_string(),
        Value::Keyword(value) => value.clone(),
        Value::Length(number, unit) => format!("{number}{unit}"),
        Value::Color(value) => value.clone(),
        Value::Function { name, arguments } => format!(
            "{name}({})",
            arguments.iter().map(render_value).collect::<Vec<_>>().join(
                if name.eq_ignore_ascii_case("url") {
                    ","
                } else {
                    ", "
                }
            )
        ),
        Value::List(values) => values
            .iter()
            .map(render_value)
            .collect::<Vec<_>>()
            .join(" "),
        Value::CommaList(values) => values
            .iter()
            .map(render_value)
            .collect::<Vec<_>>()
            .join(", "),
        Value::String(value) => value.clone(),
        Value::Number(value) => value.to_string(),
        Value::Percentage(value) => format!("{value}%"),
    }
}

fn render_grid_track_value(value: &Value, ctx: ResolutionContext) -> String {
    match value {
        Value::Length(number, unit) if unit.eq_ignore_ascii_case("fr") => {
            format!("{number}fr")
        }
        Value::Length(number, unit) => resolve_length_to_px(*number, unit, ctx)
            .map(|px| format!("{px}px"))
            .unwrap_or_else(|| format!("{number}{unit}")),
        Value::Function { name, arguments } if is_length_percentage_math_function(name) => {
            if let Some(value) = evaluate_length_percentage_math(value, ctx) {
                return value.css_text();
            }
            if name.eq_ignore_ascii_case("calc")
                && let Some(quantity) = evaluate_calc(arguments, ctx)
            {
                return match quantity.unit {
                    CalcUnit::Px => format!("{}px", quantity.value),
                    CalcUnit::Percentage => format!("{}%", quantity.value),
                    CalcUnit::Unitless => quantity.value.to_string(),
                };
            }
            render_value(value)
        }
        Value::Function { name, arguments } if name.eq_ignore_ascii_case("minmax") => {
            let rendered = arguments
                .iter()
                .map(|argument| match argument {
                    Value::Number(number) if *number == 0.0 => "0px".to_string(),
                    _ => render_grid_track_value(argument, ctx),
                })
                .collect::<Vec<_>>()
                .join(", ");
            format!("{name}({rendered})")
        }
        Value::Function { name, arguments } => format!(
            "{name}({})",
            arguments
                .iter()
                .map(|argument| render_grid_track_value(argument, ctx))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Value::List(values) => values
            .iter()
            .map(|value| render_grid_track_value(value, ctx))
            .collect::<Vec<_>>()
            .join(" "),
        _ => render_value(value),
    }
}

fn render_font_family_value(values: &[Value]) -> String {
    values
        .iter()
        .map(render_value)
        .collect::<Vec<_>>()
        .join(", ")
}

/// Captures changed animation values and preserves original alias expressions.
/// The returned owned values are inputs to a second cascade in the same frame.
fn animated_custom_property_values(
    specified: &BTreeMap<String, Value>,
    custom: &BTreeMap<String, Value>,
    properties: &PropertyMap,
    registrations: &BTreeMap<String, RegisteredCustomProperty>,
    untyped: &BTreeMap<String, Value>,
) -> Option<BTreeMap<String, Value>> {
    let mut changed: Vec<_> = registrations
        .iter()
        .filter_map(|(name, registration)| {
            if matches!(registration.syntax, RegisteredPropertySyntax::Universal) {
                return None;
            }
            let value = properties.get(name)?;
            (custom.get(name).map(render_value).as_deref() != Some(value.css_text().as_str())).then(
                || {
                    (
                        name.clone(),
                        parse_registered_initial_value(&value.css_text())
                            .unwrap_or_else(|| computed_value_to_value(value)),
                    )
                },
            )
        })
        .collect();
    changed.extend(
        untyped
            .iter()
            .filter(|(name, value)| custom.get(*name) != Some(*value))
            .map(|(name, value)| (name.clone(), value.clone())),
    );
    if changed.is_empty() {
        return None;
    }
    let mut values = custom.clone();
    // Re-resolve aliases specified on this element against its animated values.
    // Inherited aliases already have their parent's computed substitutions.
    values.extend(
        specified
            .iter()
            .map(|(name, value)| (name.clone(), value.clone())),
    );
    values.extend(changed);
    Some(values)
}

fn inherited_custom_properties(parent_style: Option<&ComputedStyle>) -> BTreeMap<String, Value> {
    parent_style
        .map(|style| style.custom_properties.clone())
        .unwrap_or_default()
}

/// Resolves context-independent registered numbers before the font prepass.
/// Relative-font calculations remain in the subsequent element-context pass.
fn resolve_font_numeric_custom_properties(
    current: BTreeMap<String, Value>,
    specified: &BTreeMap<String, Value>,
    inherited: &BTreeMap<String, Value>,
    registrations: &BTreeMap<String, RegisteredCustomProperty>,
    ctx: ResolutionContext,
    node: &NodeHandle,
) -> BTreeMap<String, Value> {
    let numeric = registrations.iter().filter(|(name, registration)| {
        let numeric = matches!(&registration.syntax, RegisteredPropertySyntax::Alternatives(parts)
            if parts.len() == 1 && parts[0].multiplier == RegisteredSyntaxMultiplier::Single
                && matches!(parts[0].kind, RegisteredSyntaxKind::Number | RegisteredSyntaxKind::Integer));
        numeric && current.get(*name).is_none_or(|value| {
            let value = sibling_functions::resolve(value, node);
            typed_math::infer(&value, None) != Some(typed_math::NumericType::NUMBER)
                || individual_transform::fixed_number_expression(&value).is_some()
        })
    }).map(|(name, registration)| (name.clone(), registration.clone())).collect::<BTreeMap<_, _>>();
    if numeric.is_empty() {
        return current;
    }
    compute_registered_custom_properties(specified, inherited, &numeric, ctx, node).0
}

fn compute_registered_custom_properties(
    specified_on_element: &BTreeMap<String, Value>,
    inherited: &BTreeMap<String, Value>,
    registrations: &BTreeMap<String, RegisteredCustomProperty>,
    ctx: ResolutionContext,
    node: &NodeHandle,
) -> (BTreeMap<String, Value>, BTreeMap<String, ComputedValue>) {
    let mut values = inherited.clone();

    for registration in registrations.values() {
        let fallback = if registration.inherits {
            inherited
                .get(&registration.name)
                .cloned()
                .or_else(|| registration.initial_value.clone())
        } else {
            registration.initial_value.clone()
        };
        if let Some(value) = fallback {
            values.insert(registration.name.clone(), value);
        } else {
            values.remove(&registration.name);
        }
    }

    for (name, specified) in specified_on_element {
        let registration = registrations.get(name);
        let keyword = match specified {
            Value::Keyword(keyword) => Some(keyword.to_ascii_lowercase()),
            _ => None,
        };
        let fallback = || {
            registration.and_then(|registration| {
                if registration.inherits {
                    inherited
                        .get(name)
                        .cloned()
                        .or_else(|| registration.initial_value.clone())
                } else {
                    registration.initial_value.clone()
                }
            })
        };
        match keyword.as_deref() {
            Some("inherit") => {
                if let Some(value) = inherited.get(name).cloned() {
                    values.insert(name.clone(), value);
                } else if let Some(value) =
                    registration.and_then(|value| value.initial_value.clone())
                {
                    values.insert(name.clone(), value);
                } else {
                    values.remove(name);
                }
            }
            Some("initial") => {
                if let Some(value) = registration.and_then(|value| value.initial_value.clone()) {
                    values.insert(name.clone(), value);
                } else {
                    values.remove(name);
                }
            }
            Some("unset") if registration.is_some() => {
                if let Some(value) = fallback() {
                    values.insert(name.clone(), value);
                } else {
                    values.remove(name);
                }
            }
            Some("unset") => {
                if let Some(value) = inherited.get(name).cloned() {
                    values.insert(name.clone(), value);
                } else {
                    values.remove(name);
                }
            }
            _ => {
                values.insert(name.clone(), specified.clone());
            }
        }
    }

    let mut resolved = resolve_custom_property_values(&values);
    for registration in registrations.values() {
        if !matches!(registration.syntax, RegisteredPropertySyntax::Universal)
            && let Some(value) = resolved.get_mut(&registration.name)
            && sibling_functions::contains(value)
        {
            *value = sibling_functions::resolve(value, node);
        }
    }
    // A registered value that does not match its syntax computes as `unset`.
    // Apply those fallbacks before resolving dependent var() references again.
    for registration in registrations.values() {
        let valid = resolved
            .get(&registration.name)
            .is_some_and(|value| registered_value_matches_syntax(value, &registration.syntax));
        if valid {
            continue;
        }
        let fallback = if registration.inherits {
            inherited
                .get(&registration.name)
                .cloned()
                .or_else(|| registration.initial_value.clone())
        } else {
            registration.initial_value.clone()
        };
        if let Some(value) = fallback {
            resolved.insert(registration.name.clone(), value);
        } else {
            resolved.remove(&registration.name);
        }
    }
    resolved = resolve_custom_property_values(&resolved);

    let mut computed = BTreeMap::new();
    for (name, value) in &resolved {
        let value = if let Some(registration) = registrations.get(name) {
            compute_registered_value(value, &registration.syntax, ctx)
        } else {
            // Unregistered custom properties retain their token sequence. In
            // particular, color-looking keywords such as `green` must not be
            // normalized until a typed registration is active.
            ComputedValue::Keyword(render_value(value))
        };
        computed.insert(name.clone(), value);
    }
    for (name, value) in &computed {
        if let Some(registration) = registrations.get(name) {
            // Universal registrations retain the substituted token stream,
            // including the declaration scope of tree-counting functions.
            if matches!(registration.syntax, RegisteredPropertySyntax::Universal) {
                continue;
            }
            let tokens = parse_registered_initial_value(&value.css_text())
                .unwrap_or_else(|| computed_value_to_value(value));
            resolved.insert(name.clone(), tokens);
        }
    }
    let resolved = finish_registered_aliases(values, &resolved, registrations, &mut computed);
    (resolved, computed)
}

/// Substitutes computed registered values into original aliases before inheritance.
fn finish_registered_aliases(
    mut original: BTreeMap<String, Value>,
    resolved: &BTreeMap<String, Value>,
    registrations: &BTreeMap<String, RegisteredCustomProperty>,
    computed: &mut BTreeMap<String, ComputedValue>,
) -> BTreeMap<String, Value> {
    // Keep invalid/cyclic aliases invalid rather than reviving their original tokens.
    original.retain(|name, _| resolved.contains_key(name));
    for name in registrations.keys() {
        if let Some(value) = resolved.get(name) {
            original.insert(name.clone(), value.clone());
        }
    }
    let resolved = resolve_custom_property_values(&original);
    for (name, value) in &resolved {
        if !registrations.contains_key(name) {
            computed.insert(name.clone(), ComputedValue::Keyword(render_value(value)));
        }
    }
    resolved
}

/// Computes a dimension with a fixed canonical unit using the shared math evaluator.
fn compute_registered_dimension(
    value: &Value,
    kind: &RegisteredSyntaxKind,
    ctx: ResolutionContext,
) -> Option<Value> {
    use typed_math::NumericType;
    let (expected, unit) = match kind {
        RegisteredSyntaxKind::Time => (NumericType::TIME, "s"),
        RegisteredSyntaxKind::Angle => (NumericType::ANGLE, "deg"),
        RegisteredSyntaxKind::Resolution => (NumericType::RESOLUTION, "dppx"),
        _ => return None,
    };
    if typed_math::infer(value, None)? != expected {
        return None;
    }
    let Value::Length(mut number, computed_unit) =
        individual_transform::computed_numeric_expression(value, ctx)?
    else {
        return None;
    };
    if expected == NumericType::TIME && computed_unit == "ms" {
        number /= 1000.0;
    }
    if expected == NumericType::RESOLUTION {
        number = number.max(0.0);
    }
    Some(Value::Length(number, unit.into()))
}

fn normalize_registered_numeric(
    value: ComputedValue,
    syntax: &RegisteredPropertySyntax,
) -> ComputedValue {
    if let (ComputedValue::Number(number), RegisteredPropertySyntax::Alternatives(parts)) =
        (&value, syntax)
    {
        if parts.len() == 1
            && parts[0].multiplier == RegisteredSyntaxMultiplier::Single
            && matches!(parts[0].kind, RegisteredSyntaxKind::Integer)
        {
            return ComputedValue::Number(round_css_integer(*number));
        }
    }
    value
}

fn compute_registered_value(
    value: &Value,
    syntax: &RegisteredPropertySyntax,
    ctx: ResolutionContext,
) -> ComputedValue {
    if let RegisteredPropertySyntax::Alternatives(parts) = syntax {
        for part in parts {
            if part.multiplier == RegisteredSyntaxMultiplier::Single {
                if let Some(value) = compute_registered_dimension(value, &part.kind, ctx) {
                    return ComputedValue::Keyword(render_value(&value));
                }
            }
        }
    }
    let scalar = match syntax {
        RegisteredPropertySyntax::Alternatives(parts) => parts.iter().find(|part| {
            part.multiplier == RegisteredSyntaxMultiplier::Single
                && registered_value_matches_kind(value, &part.kind)
        }),
        RegisteredPropertySyntax::Universal => None,
    };
    match scalar.map(|part| &part.kind) {
        Some(
            RegisteredSyntaxKind::Length
            | RegisteredSyntaxKind::LengthPercentage
            | RegisteredSyntaxKind::Number
            | RegisteredSyntaxKind::Integer
            | RegisteredSyntaxKind::Percentage
            | RegisteredSyntaxKind::Color,
        ) => normalize_registered_numeric(compute_value(value, "--registered", ctx), syntax),
        _ => ComputedValue::Keyword(render_value(value)),
    }
}

fn resolve_custom_property_values(specified: &BTreeMap<String, Value>) -> BTreeMap<String, Value> {
    specified
        .iter()
        .filter_map(|(name, value)| {
            resolve_value_with_custom_properties(value, specified)
                .map(|resolved| (name.clone(), resolved))
        })
        .collect()
}

/// Converts a `ComputedValue` back into a `Value` for re-processing (e.g., var() resolution).
fn computed_value_to_value(cv: &ComputedValue) -> Value {
    match cv {
        ComputedValue::Px(v) => Value::Length(*v, "px".to_string()),
        ComputedValue::Number(v) => Value::Number(*v),
        ComputedValue::Percentage(v) => Value::Percentage(*v),
        ComputedValue::Color(c) => Value::Keyword(c.clone()),
        ComputedValue::Keyword(k) => Value::Keyword(k.clone()),
        ComputedValue::String(s) => Value::Keyword(s.clone()),
        ComputedValue::LengthPercentage(value) => Value::Keyword(value.css_text()),
        value @ ComputedValue::TextIndent { .. } => Value::Keyword(value.css_text()),
        value @ ComputedValue::Position { .. } => Value::Keyword(value.css_text()),
        value @ ComputedValue::NineSlice(_) => Value::Keyword(value.css_text()),
    }
}

fn resolve_value_with_custom_properties(
    value: &Value,
    custom_properties: &BTreeMap<String, Value>,
) -> Option<Value> {
    let mut stack = Vec::new();
    resolve_value_with_custom_properties_inner(value, custom_properties, &mut stack, 0)
}

fn resolve_value_with_custom_properties_inner(
    value: &Value,
    custom_properties: &BTreeMap<String, Value>,
    stack: &mut Vec<String>,
    depth: usize,
) -> Option<Value> {
    if depth > 32 {
        return None;
    }

    match value {
        Value::Function { name, arguments } if name.eq_ignore_ascii_case("var") => {
            resolve_var_function(arguments, custom_properties, stack, depth + 1)
        }
        Value::Function { name, arguments } => {
            let mut resolved_arguments = Vec::with_capacity(arguments.len());
            for argument in arguments {
                resolved_arguments.push(resolve_value_with_custom_properties_inner(
                    argument,
                    custom_properties,
                    stack,
                    depth + 1,
                )?);
            }
            Some(Value::Function {
                name: name.clone(),
                arguments: resolved_arguments,
            })
        }
        Value::List(values) => {
            let mut resolved_values = Vec::with_capacity(values.len());
            for item in values {
                let resolved = resolve_value_with_custom_properties_inner(
                    item,
                    custom_properties,
                    stack,
                    depth + 1,
                )?;
                if let Value::List(values) = resolved {
                    resolved_values.extend(values);
                } else {
                    resolved_values.push(resolved);
                }
            }
            Some(Value::List(resolved_values))
        }
        Value::CommaList(values) => {
            let mut resolved_values = Vec::with_capacity(values.len());
            for item in values {
                let resolved = resolve_value_with_custom_properties_inner(
                    item,
                    custom_properties,
                    stack,
                    depth + 1,
                )?;
                if let Value::CommaList(values) = resolved {
                    resolved_values.extend(values);
                } else {
                    resolved_values.push(resolved);
                }
            }
            Some(Value::CommaList(resolved_values))
        }
        _ => Some(value.clone()),
    }
}

fn resolve_var_function(
    arguments: &[Value],
    custom_properties: &BTreeMap<String, Value>,
    stack: &mut Vec<String>,
    depth: usize,
) -> Option<Value> {
    let reference_name = custom_property_reference_name(arguments.first()?)?;
    if stack.iter().any(|name| name == reference_name) {
        return arguments.get(1).and_then(|fallback| {
            resolve_value_with_custom_properties_inner(fallback, custom_properties, stack, depth)
        });
    }

    if let Some(referenced) = custom_properties.get(reference_name) {
        stack.push(reference_name.to_string());
        let resolved =
            resolve_value_with_custom_properties_inner(referenced, custom_properties, stack, depth);
        let _ = stack.pop();
        if resolved.is_some() {
            return resolved;
        }
    }

    arguments.get(1).and_then(|fallback| {
        resolve_value_with_custom_properties_inner(fallback, custom_properties, stack, depth)
    })
}

fn custom_property_reference_name(value: &Value) -> Option<&str> {
    match value {
        Value::Keyword(name) if name.starts_with("--") => Some(name.as_str()),
        _ => None,
    }
}

#[cfg(test)]
#[path = "style_tests.rs"]
mod style_tests;
