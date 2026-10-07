//! Canonical registry of CSS property names this engine recognizes, and the
//! [`PropertyMap`] that stores computed values keyed by them.
//!
//! Longhand and shorthand/legacy-alias names are declared exactly once, by
//! [`define_properties!`] below, so [`SUPPORTED_PROPERTIES`],
//! [`is_shorthand_or_legacy_alias`], and [`PropertyId`] cannot drift apart
//! the way two independently hand-maintained lists could (see
//! <https://github.com/ieee0824/omoikane/issues/1087>, item 3). Adding a
//! property means adding one line here; every derived list and function
//! picks it up automatically.

use std::collections::btree_map::{self, BTreeMap};
use std::fmt;
use std::iter::Peekable;

use super::ComputedValue;

macro_rules! define_properties {
    (
        longhands: [$($lh_str:literal => $lh_variant:ident),+ $(,)?],
        shorthand_or_alias: [$($sh_str:literal),+ $(,)?],
    ) => {
        /// Identity of one implemented CSS longhand property.
        ///
        /// Only implemented longhands have a variant. Shorthands, legacy
        /// aliases, custom properties (`--foo`), and unimplemented names
        /// never produce a `PropertyId`; [`PropertyMap`] stores those under
        /// their string name instead.
        ///
        /// Variants are declared in ascending byte order of their CSS
        /// names, so the derived `Ord` matches string order. [`PropertyMap`]
        /// relies on this to iterate in the same order as a string-keyed
        /// map (checked by `variants_are_declared_in_name_order`).
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub(crate) enum PropertyId {
            $(#[doc = $lh_str] $lh_variant),+
        }

        impl PropertyId {
            /// Every variant, in declaration (and therefore name) order.
            pub(crate) const ALL: &'static [Self] = &[$(Self::$lh_variant),+];

            /// Every variant's name, index-aligned with [`Self::ALL`].
            const NAMES: &'static [&'static str] = &[$($lh_str),+];

            /// Parses a canonical (already-lowercased) property name.
            ///
            /// Returns `None` for shorthands, legacy aliases, custom
            /// properties, and any name this engine does not implement.
            pub(crate) fn parse(name: &str) -> Option<Self> {
                // `NAMES` is sorted (see `variants_are_declared_in_name_order`),
                // so this costs the same comparisons as a string-keyed map.
                Self::NAMES
                    .binary_search(&name)
                    .ok()
                    .map(|index| Self::ALL[index])
            }

            /// Returns the canonical CSS property name.
            pub(crate) const fn as_str(self) -> &'static str {
                Self::NAMES[self as usize]
            }
        }

        /// Every property name this engine recognizes: implemented longhands
        /// plus the shorthands and legacy aliases that expand into them.
        pub(crate) const SUPPORTED_PROPERTIES: &[&str] = &[
            $($lh_str,)+
            $($sh_str,)+
        ];

        /// Returns whether `name` is a shorthand or legacy alias rather than
        /// an implemented longhand.
        pub(crate) fn is_shorthand_or_legacy_alias(name: &str) -> bool {
            matches!(name, $($sh_str)|+)
        }
    };
}

define_properties! {
    longhands: [
        "-webkit-clip-path" => WebkitClipPath,
        "-webkit-mask-composite" => WebkitMaskComposite,
        "-webkit-mask-image" => WebkitMaskImage,
        "-webkit-mask-mode" => WebkitMaskMode,
        "-webkit-mask-position-x" => WebkitMaskPositionX,
        "-webkit-mask-position-y" => WebkitMaskPositionY,
        "-webkit-mask-repeat" => WebkitMaskRepeat,
        "-webkit-mask-size" => WebkitMaskSize,
        "align-content" => AlignContent,
        "align-items" => AlignItems,
        "align-self" => AlignSelf,
        "animation-delay" => AnimationDelay,
        "animation-direction" => AnimationDirection,
        "animation-duration" => AnimationDuration,
        "animation-fill-mode" => AnimationFillMode,
        "animation-iteration-count" => AnimationIterationCount,
        "animation-name" => AnimationName,
        "animation-play-state" => AnimationPlayState,
        "animation-timing-function" => AnimationTimingFunction,
        "aspect-ratio" => AspectRatio,
        "backdrop-filter" => BackdropFilter,
        "backface-visibility" => BackfaceVisibility,
        "background-attachment" => BackgroundAttachment,
        "background-clip" => BackgroundClip,
        "background-color" => BackgroundColor,
        "background-image" => BackgroundImage,
        "background-origin" => BackgroundOrigin,
        "background-position-x" => BackgroundPositionX,
        "background-position-y" => BackgroundPositionY,
        "background-repeat" => BackgroundRepeat,
        "background-size" => BackgroundSize,
        "block-size" => BlockSize,
        "border-block-end-color" => BorderBlockEndColor,
        "border-block-end-style" => BorderBlockEndStyle,
        "border-block-end-width" => BorderBlockEndWidth,
        "border-block-start-color" => BorderBlockStartColor,
        "border-block-start-style" => BorderBlockStartStyle,
        "border-block-start-width" => BorderBlockStartWidth,
        "border-bottom-color" => BorderBottomColor,
        "border-bottom-left-radius" => BorderBottomLeftRadius,
        "border-bottom-right-radius" => BorderBottomRightRadius,
        "border-bottom-style" => BorderBottomStyle,
        "border-bottom-width" => BorderBottomWidth,
        "border-collapse" => BorderCollapse,
        "border-end-end-radius" => BorderEndEndRadius,
        "border-end-start-radius" => BorderEndStartRadius,
        "border-inline-end-color" => BorderInlineEndColor,
        "border-inline-end-style" => BorderInlineEndStyle,
        "border-inline-end-width" => BorderInlineEndWidth,
        "border-inline-start-color" => BorderInlineStartColor,
        "border-inline-start-style" => BorderInlineStartStyle,
        "border-inline-start-width" => BorderInlineStartWidth,
        "border-left-color" => BorderLeftColor,
        "border-left-style" => BorderLeftStyle,
        "border-left-width" => BorderLeftWidth,
        "border-right-color" => BorderRightColor,
        "border-right-style" => BorderRightStyle,
        "border-right-width" => BorderRightWidth,
        "border-spacing" => BorderSpacing,
        "border-start-end-radius" => BorderStartEndRadius,
        "border-start-start-radius" => BorderStartStartRadius,
        "border-top-color" => BorderTopColor,
        "border-top-left-radius" => BorderTopLeftRadius,
        "border-top-right-radius" => BorderTopRightRadius,
        "border-top-style" => BorderTopStyle,
        "border-top-width" => BorderTopWidth,
        "bottom" => Bottom,
        "box-decoration-break" => BoxDecorationBreak,
        "box-shadow" => BoxShadow,
        "box-sizing" => BoxSizing,
        "break-after" => BreakAfter,
        "break-before" => BreakBefore,
        "break-inside" => BreakInside,
        "clear" => Clear,
        "clip-path" => ClipPath,
        "color" => Color,
        "column-count" => ColumnCount,
        "column-fill" => ColumnFill,
        "column-gap" => ColumnGap,
        "column-rule-color" => ColumnRuleColor,
        "column-rule-style" => ColumnRuleStyle,
        "column-rule-width" => ColumnRuleWidth,
        "column-span" => ColumnSpan,
        "column-width" => ColumnWidth,
        "contain" => Contain,
        "contain-intrinsic-block-size" => ContainIntrinsicBlockSize,
        "contain-intrinsic-height" => ContainIntrinsicHeight,
        "contain-intrinsic-inline-size" => ContainIntrinsicInlineSize,
        "contain-intrinsic-width" => ContainIntrinsicWidth,
        "container-name" => ContainerName,
        "container-type" => ContainerType,
        "content" => Content,
        "content-visibility" => ContentVisibility,
        "counter-increment" => CounterIncrement,
        "counter-reset" => CounterReset,
        "cursor" => Cursor,
        "direction" => Direction,
        "display" => Display,
        "filter" => Filter,
        "flex-basis" => FlexBasis,
        "flex-direction" => FlexDirection,
        "flex-grow" => FlexGrow,
        "flex-shrink" => FlexShrink,
        "flex-wrap" => FlexWrap,
        "float" => Float,
        "font-family" => FontFamily,
        "font-size" => FontSize,
        "font-stretch" => FontStretch,
        "font-style" => FontStyle,
        "font-weight" => FontWeight,
        "grid-auto-columns" => GridAutoColumns,
        "grid-auto-flow" => GridAutoFlow,
        "grid-auto-rows" => GridAutoRows,
        "grid-column-end" => GridColumnEnd,
        "grid-column-start" => GridColumnStart,
        "grid-row-end" => GridRowEnd,
        "grid-row-start" => GridRowStart,
        "grid-template-areas" => GridTemplateAreas,
        "grid-template-columns" => GridTemplateColumns,
        "grid-template-rows" => GridTemplateRows,
        "height" => Height,
        "inline-size" => InlineSize,
        "inset-block-end" => InsetBlockEnd,
        "inset-block-start" => InsetBlockStart,
        "inset-inline-end" => InsetInlineEnd,
        "inset-inline-start" => InsetInlineStart,
        "isolation" => Isolation,
        "justify-content" => JustifyContent,
        "justify-items" => JustifyItems,
        "justify-self" => JustifySelf,
        "left" => Left,
        "letter-spacing" => LetterSpacing,
        "line-height" => LineHeight,
        "list-style-image" => ListStyleImage,
        "list-style-position" => ListStylePosition,
        "list-style-type" => ListStyleType,
        "margin-block-end" => MarginBlockEnd,
        "margin-block-start" => MarginBlockStart,
        "margin-bottom" => MarginBottom,
        "margin-inline-end" => MarginInlineEnd,
        "margin-inline-start" => MarginInlineStart,
        "margin-left" => MarginLeft,
        "margin-right" => MarginRight,
        "margin-top" => MarginTop,
        "mask-composite" => MaskComposite,
        "mask-image" => MaskImage,
        "mask-mode" => MaskMode,
        "mask-position-x" => MaskPositionX,
        "mask-position-y" => MaskPositionY,
        "mask-repeat" => MaskRepeat,
        "mask-size" => MaskSize,
        "max-block-size" => MaxBlockSize,
        "max-height" => MaxHeight,
        "max-inline-size" => MaxInlineSize,
        "max-width" => MaxWidth,
        "min-block-size" => MinBlockSize,
        "min-height" => MinHeight,
        "min-inline-size" => MinInlineSize,
        "min-width" => MinWidth,
        "mix-blend-mode" => MixBlendMode,
        "object-fit" => ObjectFit,
        "object-position" => ObjectPosition,
        "opacity" => Opacity,
        "order" => Order,
        "orphans" => Orphans,
        "outline-color" => OutlineColor,
        "outline-offset" => OutlineOffset,
        "outline-style" => OutlineStyle,
        "outline-width" => OutlineWidth,
        "overflow-wrap" => OverflowWrap,
        "overflow-x" => OverflowX,
        "overflow-y" => OverflowY,
        "overscroll-behavior-block" => OverscrollBehaviorBlock,
        "overscroll-behavior-inline" => OverscrollBehaviorInline,
        "overscroll-behavior-x" => OverscrollBehaviorX,
        "overscroll-behavior-y" => OverscrollBehaviorY,
        "padding-block-end" => PaddingBlockEnd,
        "padding-block-start" => PaddingBlockStart,
        "padding-bottom" => PaddingBottom,
        "padding-inline-end" => PaddingInlineEnd,
        "padding-inline-start" => PaddingInlineStart,
        "padding-left" => PaddingLeft,
        "padding-right" => PaddingRight,
        "padding-top" => PaddingTop,
        "page" => Page,
        "perspective" => Perspective,
        "perspective-origin" => PerspectiveOrigin,
        "pointer-events" => PointerEvents,
        "position" => Position,
        "right" => Right,
        "row-gap" => RowGap,
        "scroll-behavior" => ScrollBehavior,
        "scroll-margin-block-end" => ScrollMarginBlockEnd,
        "scroll-margin-block-start" => ScrollMarginBlockStart,
        "scroll-margin-bottom" => ScrollMarginBottom,
        "scroll-margin-inline-end" => ScrollMarginInlineEnd,
        "scroll-margin-inline-start" => ScrollMarginInlineStart,
        "scroll-margin-left" => ScrollMarginLeft,
        "scroll-margin-right" => ScrollMarginRight,
        "scroll-margin-top" => ScrollMarginTop,
        "scroll-padding-block-end" => ScrollPaddingBlockEnd,
        "scroll-padding-block-start" => ScrollPaddingBlockStart,
        "scroll-padding-bottom" => ScrollPaddingBottom,
        "scroll-padding-inline-end" => ScrollPaddingInlineEnd,
        "scroll-padding-inline-start" => ScrollPaddingInlineStart,
        "scroll-padding-left" => ScrollPaddingLeft,
        "scroll-padding-right" => ScrollPaddingRight,
        "scroll-padding-top" => ScrollPaddingTop,
        "scroll-snap-align" => ScrollSnapAlign,
        "scroll-snap-type" => ScrollSnapType,
        "shape-margin" => ShapeMargin,
        "shape-outside" => ShapeOutside,
        "text-align" => TextAlign,
        "text-decoration-color" => TextDecorationColor,
        "text-decoration-line" => TextDecorationLine,
        "text-decoration-style" => TextDecorationStyle,
        "text-decoration-thickness" => TextDecorationThickness,
        "text-indent" => TextIndent,
        "text-overflow" => TextOverflow,
        "text-shadow" => TextShadow,
        "text-transform" => TextTransform,
        "text-underline-offset" => TextUnderlineOffset,
        "text-underline-position" => TextUnderlinePosition,
        "top" => Top,
        "transform" => Transform,
        "transform-origin" => TransformOrigin,
        "transform-style" => TransformStyle,
        "transition-delay" => TransitionDelay,
        "transition-duration" => TransitionDuration,
        "transition-property" => TransitionProperty,
        "transition-timing-function" => TransitionTimingFunction,
        "unicode-bidi" => UnicodeBidi,
        "vertical-align" => VerticalAlign,
        "visibility" => Visibility,
        "white-space" => WhiteSpace,
        "widows" => Widows,
        "width" => Width,
        "word-break" => WordBreak,
        "word-spacing" => WordSpacing,
        "writing-mode" => WritingMode,
        "z-index" => ZIndex,
    ],
    shorthand_or_alias: [
        "-webkit-mask",
        "-webkit-mask-position",
        "animation",
        "border-block",
        "border-block-color",
        "border-block-end",
        "border-block-start",
        "border-block-style",
        "border-block-width",
        "border-color",
        "border-inline",
        "border-inline-color",
        "border-inline-end",
        "border-inline-start",
        "border-inline-style",
        "border-inline-width",
        "border-style",
        "border-width",
        "column-rule",
        "columns",
        "contain-intrinsic-size",
        "gap",
        "grid-area",
        "grid-column",
        "grid-column-gap",
        "grid-gap",
        "grid-row",
        "grid-row-gap",
        "grid-template",
        "inset",
        "inset-block",
        "inset-inline",
        "mask",
        "mask-position",
        "overflow",
        "overscroll-behavior",
        "place-content",
        "place-items",
        "place-self",
        "scroll-margin",
        "scroll-margin-block",
        "scroll-margin-inline",
        "scroll-padding",
        "scroll-padding-block",
        "scroll-padding-inline",
        "transition",
        "word-wrap",
    ],
}

/// A borrowed property name accepted by [`PropertyMap`] lookups: a
/// [`PropertyId`], or any string name (resolved to a [`PropertyId`] when it
/// names an implemented longhand).
pub(crate) trait PropertyKeyRef {
    /// Returns the implemented longhand this name identifies, or the name
    /// itself when it identifies anything else.
    fn resolve(&self) -> Result<PropertyId, &str>;
}

impl PropertyKeyRef for PropertyId {
    fn resolve(&self) -> Result<PropertyId, &str> {
        Ok(*self)
    }
}

impl PropertyKeyRef for str {
    fn resolve(&self) -> Result<PropertyId, &str> {
        PropertyId::parse(self).ok_or(self)
    }
}

impl PropertyKeyRef for String {
    fn resolve(&self) -> Result<PropertyId, &str> {
        self.as_str().resolve()
    }
}

/// An owned property name accepted by [`PropertyMap::insert`] and
/// [`PropertyMap::entry`].
pub(crate) trait IntoPropertyKey {
    /// Returns the implemented longhand this name identifies, or the name
    /// itself when it identifies anything else.
    fn into_key(self) -> Result<PropertyId, Box<str>>;
}

impl IntoPropertyKey for PropertyId {
    fn into_key(self) -> Result<PropertyId, Box<str>> {
        Ok(self)
    }
}

impl IntoPropertyKey for &str {
    fn into_key(self) -> Result<PropertyId, Box<str>> {
        PropertyId::parse(self).ok_or_else(|| self.into())
    }
}

impl IntoPropertyKey for &String {
    fn into_key(self) -> Result<PropertyId, Box<str>> {
        self.as_str().into_key()
    }
}

impl IntoPropertyKey for String {
    fn into_key(self) -> Result<PropertyId, Box<str>> {
        PropertyId::parse(&self).ok_or_else(|| self.into_boxed_str())
    }
}

/// Computed property values keyed by CSS property name.
///
/// Behaves like a `BTreeMap<String, ComputedValue>`: keys are matched
/// exactly, and iteration yields names in ascending byte order. Implemented
/// longhands (the common case) are stored under their [`PropertyId`], so
/// lookups by `PropertyId` compare integers rather than strings. Every other
/// name the cascade stores — registered custom properties (`--foo`),
/// shorthands serialized for CSSOM (`transition`, `contain-intrinsic-size`),
/// and unimplemented author properties retained for diagnostics — lives in
/// a string-keyed fallback, so no stored name is ever dropped or rejected.
///
/// A given name always resolves to the same side, so the representation is
/// canonical and the derived equality matches string-keyed map equality.
#[derive(Clone, PartialEq, Default)]
pub(crate) struct PropertyMap {
    known: BTreeMap<PropertyId, ComputedValue>,
    other: BTreeMap<Box<str>, ComputedValue>,
}

impl PropertyMap {
    /// Creates an empty map.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Returns the value stored for `key`.
    pub(crate) fn get<K: PropertyKeyRef + ?Sized>(&self, key: &K) -> Option<&ComputedValue> {
        match key.resolve() {
            Ok(id) => self.known.get(&id),
            Err(name) => self.other.get(name),
        }
    }

    /// Returns whether a value is stored for `key`.
    pub(crate) fn contains_key<K: PropertyKeyRef + ?Sized>(&self, key: &K) -> bool {
        match key.resolve() {
            Ok(id) => self.known.contains_key(&id),
            Err(name) => self.other.contains_key(name),
        }
    }

    /// Removes and returns the value stored for `key`.
    pub(crate) fn remove<K: PropertyKeyRef + ?Sized>(&mut self, key: &K) -> Option<ComputedValue> {
        match key.resolve() {
            Ok(id) => self.known.remove(&id),
            Err(name) => self.other.remove(name),
        }
    }

    /// Stores `value` for `key`, returning the previous value.
    pub(crate) fn insert(
        &mut self,
        key: impl IntoPropertyKey,
        value: ComputedValue,
    ) -> Option<ComputedValue> {
        match key.into_key() {
            Ok(id) => self.known.insert(id, value),
            Err(name) => self.other.insert(name, value),
        }
    }

    /// Returns the entry for `key` for in-place insertion.
    pub(crate) fn entry(&mut self, key: impl IntoPropertyKey) -> PropertyEntry<'_> {
        match key.into_key() {
            Ok(id) => PropertyEntry::Known(self.known.entry(id)),
            Err(name) => PropertyEntry::Other(self.other.entry(name)),
        }
    }

    /// Returns whether no properties are stored.
    pub(crate) fn is_empty(&self) -> bool {
        self.known.is_empty() && self.other.is_empty()
    }

    /// Iterates `(name, value)` pairs in ascending name order.
    pub(crate) fn iter(&self) -> Iter<'_> {
        Iter {
            known: self.known.iter().peekable(),
            other: self.other.iter().peekable(),
        }
    }

    /// Iterates stored names in ascending order.
    pub(crate) fn keys(&self) -> impl Iterator<Item = &str> {
        self.iter().map(|(name, _)| name)
    }
}

impl fmt::Debug for PropertyMap {
    // Match the `BTreeMap<String, _>` form this map replaced, so existing
    // debug output and snapshots keep their shape.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_map().entries(self.iter()).finish()
    }
}

impl<'a> IntoIterator for &'a PropertyMap {
    type Item = (&'a str, &'a ComputedValue);
    type IntoIter = Iter<'a>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

/// Name-ordered iterator over a [`PropertyMap`].
///
/// Merges the two sorted halves; they never share a name.
pub(crate) struct Iter<'a> {
    known: Peekable<btree_map::Iter<'a, PropertyId, ComputedValue>>,
    other: Peekable<btree_map::Iter<'a, Box<str>, ComputedValue>>,
}

impl<'a> Iterator for Iter<'a> {
    type Item = (&'a str, &'a ComputedValue);

    fn next(&mut self) -> Option<Self::Item> {
        let known_first = match (self.known.peek(), self.other.peek()) {
            (Some((id, _)), Some((name, _))) => id.as_str() < &***name,
            (Some(_), None) => true,
            (None, Some(_)) => false,
            (None, None) => return None,
        };
        if known_first {
            self.known.next().map(|(id, value)| (id.as_str(), value))
        } else {
            self.other.next().map(|(name, value)| (&**name, value))
        }
    }
}

/// A view into a single [`PropertyMap`] entry.
pub(crate) enum PropertyEntry<'a> {
    Known(btree_map::Entry<'a, PropertyId, ComputedValue>),
    Other(btree_map::Entry<'a, Box<str>, ComputedValue>),
}

impl<'a> PropertyEntry<'a> {
    /// Inserts `default` if the entry is vacant, returning the stored value.
    pub(crate) fn or_insert(self, default: ComputedValue) -> &'a mut ComputedValue {
        match self {
            Self::Known(entry) => entry.or_insert(default),
            Self::Other(entry) => entry.or_insert(default),
        }
    }

    /// Inserts the result of `default` if the entry is vacant, returning the
    /// stored value.
    pub(crate) fn or_insert_with(
        self,
        default: impl FnOnce() -> ComputedValue,
    ) -> &'a mut ComputedValue {
        match self {
            Self::Known(entry) => entry.or_insert_with(default),
            Self::Other(entry) => entry.or_insert_with(default),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variants_are_declared_in_name_order() {
        // PropertyMap's merged iteration assumes derived Ord == name order.
        for pair in PropertyId::ALL.windows(2) {
            assert!(
                pair[0].as_str() < pair[1].as_str(),
                "`{}` must be declared before `{}`",
                pair[1].as_str(),
                pair[0].as_str()
            );
        }
    }

    #[test]
    fn every_longhand_round_trips_through_its_name() {
        for id in PropertyId::ALL {
            assert_eq!(PropertyId::parse(id.as_str()), Some(*id));
        }
    }

    #[test]
    fn registry_partitions_supported_names_into_longhands_and_shorthands() {
        // Double check for issue #1087: every supported name is exactly one
        // of an implemented longhand or a shorthand/alias, never both or
        // neither, and the counts add up with no duplicates.
        let mut seen = std::collections::BTreeSet::new();
        let mut longhands = 0;
        for name in SUPPORTED_PROPERTIES {
            assert!(seen.insert(*name), "`{name}` is registered twice");
            let is_longhand = PropertyId::parse(name).is_some();
            assert_ne!(
                is_longhand,
                is_shorthand_or_legacy_alias(name),
                "`{name}` must be exactly one of longhand or shorthand/alias"
            );
            longhands += usize::from(is_longhand);
        }
        assert_eq!(longhands, PropertyId::ALL.len());
    }

    #[test]
    fn map_keeps_non_longhand_names_and_iterates_like_a_string_keyed_map() {
        let names = [
            "--custom",
            "-webkit-mask-image",
            "color",
            "contain-intrinsic-size",
            "display",
            "not-a-real-property",
            "transition",
            "z-index",
        ];
        let mut map = PropertyMap::new();
        let mut reference = BTreeMap::new();
        for (index, name) in names.iter().rev().enumerate() {
            let value = ComputedValue::Number(index as f32);
            map.insert(*name, value.clone());
            reference.insert(name.to_string(), value);
        }

        let iterated: Vec<_> = map
            .iter()
            .map(|(n, v)| (n.to_string(), v.clone()))
            .collect();
        let expected: Vec<_> = reference.into_iter().collect();
        assert_eq!(iterated, expected);
        assert_eq!(map.iter().count(), names.len());

        for name in names {
            assert!(map.contains_key(name), "`{name}` should be retained");
        }
        assert_eq!(
            map.get(&PropertyId::Color),
            map.get("color"),
            "a PropertyId and its name address the same entry"
        );
        assert!(
            !map.contains_key("COLOR"),
            "names match exactly, like a string key"
        );
    }
}
