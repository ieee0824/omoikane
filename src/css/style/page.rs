//! Page-context cascade for printed documents.

use std::collections::BTreeMap;

use super::{
    CascadeLayerOrder, ComputedValue, LayerPath, Origin, ResolutionContext, StyleResolver,
    compute_value, layer_block_path, layer_group_rule_is_active,
};
use crate::css::{Declaration, Rule, Value};

/// The side of a page in a two-sided document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageSide {
    /// Left-hand page of a spread.
    Left,
    /// Right-hand page of a spread.
    Right,
}

/// The facts used to match a CSS `@page` selector.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageSelectorContext {
    /// Zero-based page number in the document.
    pub index: usize,
    /// Page type selected by the `page` property, if any.
    pub name: Option<String>,
    /// Page side after applying the document's page progression.
    pub side: PageSide,
    /// Whether this page was inserted empty by a forced side break.
    pub blank: bool,
}

impl PageSelectorContext {
    /// Creates a page context with left-to-right page progression.
    pub fn new(index: usize, name: Option<String>) -> Self {
        Self {
            index,
            name,
            side: if index.is_multiple_of(2) {
                PageSide::Right
            } else {
                PageSide::Left
            },
            blank: false,
        }
    }
}

/// Winning declarations in a page context, before used-value geometry resolution.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ResolvedPageStyle {
    properties: BTreeMap<String, Value>,
}

/// Used paper dimensions and page margins in CSS pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PageBoxGeometry {
    /// Paper width.
    pub width: f32,
    /// Paper height.
    pub height: f32,
    /// Top margin.
    pub margin_top: f32,
    /// Right margin.
    pub margin_right: f32,
    /// Bottom margin.
    pub margin_bottom: f32,
    /// Left margin.
    pub margin_left: f32,
}

impl ResolvedPageStyle {
    /// Returns the winning specified value for a page property.
    pub fn get(&self, name: &str) -> Option<&Value> {
        self.properties.get(name)
    }

    /// Returns the winning properties in stable name order.
    pub fn properties(&self) -> &BTreeMap<String, Value> {
        &self.properties
    }

    /// Resolves `size` and physical margins against the supplied print defaults.
    pub fn geometry(&self, default_size: (f32, f32), default_margin: f32) -> PageBoxGeometry {
        let (width, height) = self
            .get("size")
            .and_then(|value| page_size(value, default_size))
            .unwrap_or(default_size);
        let ctx = ResolutionContext {
            viewport_width: width,
            viewport_height: height,
            ..ResolutionContext::default()
        };
        let margin = |side: &str| {
            self.get(side)
                .and_then(|value| match compute_value(value, side, ctx) {
                    ComputedValue::Px(value) => Some(value),
                    ComputedValue::Percentage(value) => Some(width * value / 100.0),
                    ComputedValue::Number(value) if value == 0.0 => Some(0.0),
                    _ => None,
                })
                .filter(|value| value.is_finite())
                .unwrap_or(default_margin)
        };
        PageBoxGeometry {
            width,
            height,
            margin_top: margin("margin-top"),
            margin_right: margin("margin-right"),
            margin_bottom: margin("margin-bottom"),
            margin_left: margin("margin-left"),
        }
    }
}

fn page_size(value: &Value, default_size: (f32, f32)) -> Option<(f32, f32)> {
    let values = match value {
        Value::List(values) => values.as_slice(),
        value => std::slice::from_ref(value),
    };
    let length = |value: &Value| match compute_value(value, "width", ResolutionContext::default()) {
        ComputedValue::Px(px) if px.is_finite() && px > 0.0 => Some(px),
        _ => None,
    };
    if let [width, height] = values
        && let (Some(width), Some(height)) = (length(width), length(height))
    {
        return Some((width, height));
    }
    if let [value] = values
        && let Some(length) = length(value)
    {
        return Some((length, length));
    }
    let mut size = default_size;
    let mut paper = false;
    let mut orientation = None;
    for value in values {
        let Value::Keyword(keyword) = value else {
            return None;
        };
        match keyword.to_ascii_lowercase().as_str() {
            "auto" if values.len() == 1 => return Some(default_size),
            "a5" => (size, paper) = ((148.0 * 96.0 / 25.4, 210.0 * 96.0 / 25.4), true),
            "a4" => (size, paper) = ((210.0 * 96.0 / 25.4, 297.0 * 96.0 / 25.4), true),
            "a3" => (size, paper) = ((297.0 * 96.0 / 25.4, 420.0 * 96.0 / 25.4), true),
            "b5" => (size, paper) = ((176.0 * 96.0 / 25.4, 250.0 * 96.0 / 25.4), true),
            "b4" => (size, paper) = ((250.0 * 96.0 / 25.4, 353.0 * 96.0 / 25.4), true),
            "letter" => (size, paper) = ((8.5 * 96.0, 11.0 * 96.0), true),
            "legal" => (size, paper) = ((8.5 * 96.0, 14.0 * 96.0), true),
            "ledger" => (size, paper) = ((11.0 * 96.0, 17.0 * 96.0), true),
            "portrait" | "landscape" if orientation.is_none() => orientation = Some(keyword),
            _ => return None,
        }
    }
    if !paper && orientation.is_none() {
        return None;
    }
    match orientation
        .map(|value| value.to_ascii_lowercase())
        .as_deref()
    {
        Some("landscape") if size.0 < size.1 => Some((size.1, size.0)),
        Some("portrait") if size.0 > size.1 => Some((size.1, size.0)),
        _ => Some(size),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct PageSpecificity {
    name: u8,
    first_or_blank: u16,
    side: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PagePseudo {
    First,
    Blank,
    Left,
    Right,
}

#[derive(Debug)]
struct PageSelector<'a> {
    name: Option<&'a str>,
    pseudos: Vec<PagePseudo>,
}

impl PageSelector<'_> {
    fn specificity(&self) -> PageSpecificity {
        let mut specificity = PageSpecificity {
            name: u8::from(self.name.is_some()),
            first_or_blank: 0,
            side: 0,
        };
        for pseudo in &self.pseudos {
            match pseudo {
                PagePseudo::First | PagePseudo::Blank => {
                    specificity.first_or_blank = specificity.first_or_blank.saturating_add(1);
                }
                PagePseudo::Left | PagePseudo::Right => {
                    specificity.side = specificity.side.saturating_add(1);
                }
            }
        }
        specificity
    }

    fn matches(&self, context: &PageSelectorContext) -> bool {
        self.name.is_none_or(|name| {
            !name.eq_ignore_ascii_case("auto") && context.name.as_deref() == Some(name)
        }) && self.pseudos.iter().all(|pseudo| match pseudo {
            PagePseudo::First => context.index == 0,
            PagePseudo::Blank => context.blank,
            PagePseudo::Left => context.side == PageSide::Left,
            PagePseudo::Right => context.side == PageSide::Right,
        })
    }
}

fn parse_page_selector_list(prelude: &str) -> Option<Vec<PageSelector<'_>>> {
    if prelude.is_empty() {
        return Some(vec![PageSelector {
            name: None,
            pseudos: Vec::new(),
        }]);
    }
    prelude
        .split(',')
        .map(|part| {
            let part = part.trim();
            if part.is_empty() || part.chars().any(char::is_whitespace) {
                return None;
            }
            let (name, mut rest) = match part.find(':') {
                Some(0) => (None, part),
                Some(position) => (Some(&part[..position]), &part[position..]),
                None => (Some(part), ""),
            };
            if name.is_some_and(|name| {
                !name
                    .chars()
                    .all(|ch| ch.is_alphanumeric() || matches!(ch, '-' | '_'))
            }) {
                return None;
            }
            let mut pseudos = Vec::new();
            while !rest.is_empty() {
                rest = rest.strip_prefix(':')?;
                let end = rest.find(':').unwrap_or(rest.len());
                let pseudo = match &rest[..end].to_ascii_lowercase()[..] {
                    "first" => PagePseudo::First,
                    "blank" => PagePseudo::Blank,
                    "left" => PagePseudo::Left,
                    "right" => PagePseudo::Right,
                    _ => return None,
                };
                pseudos.push(pseudo);
                rest = &rest[end..];
            }
            Some(PageSelector { name, pseudos })
        })
        .collect()
}

#[derive(Debug)]
struct PageCandidate {
    declaration: Declaration,
    origin: Origin,
    layer_order: Vec<usize>,
    specificity: PageSpecificity,
    source_order: usize,
}

impl PageCandidate {
    fn priority(&self) -> (u8, u8) {
        let origin = match (self.declaration.important, self.origin) {
            (true, Origin::UserAgent) => 5,
            (true, Origin::User) => 4,
            (true, Origin::Author) => 3,
            (false, Origin::Author) => 2,
            (false, Origin::User) => 1,
            (false, Origin::UserAgent) => 0,
        };
        (u8::from(self.declaration.important), origin)
    }

    fn outranks(&self, other: &Self) -> bool {
        use std::cmp::Ordering;
        let layer = self.layer_order.cmp(&other.layer_order);
        self.priority()
            .cmp(&other.priority())
            .then(if self.declaration.important {
                layer.reverse()
            } else {
                layer
            })
            .then(self.specificity.cmp(&other.specificity))
            .then(self.source_order.cmp(&other.source_order))
            == Ordering::Greater
    }
}

#[allow(clippy::too_many_arguments)]
fn collect_page_candidates(
    rules: &[Rule],
    origin: Origin,
    stylesheet_id: usize,
    layer_order: &CascadeLayerOrder,
    active_layer: Option<&LayerPath>,
    context: &PageSelectorContext,
    viewport_width: f32,
    viewport_height: f32,
    color_scheme_dark: bool,
    media_type: crate::css::MediaType,
    source_order: &mut usize,
    candidates: &mut Vec<PageCandidate>,
) {
    for rule in rules {
        let Rule::At(at_rule) = rule else {
            continue;
        };
        if at_rule.name.eq_ignore_ascii_case("page") {
            let specificity = parse_page_selector_list(&at_rule.prelude).and_then(|selectors| {
                selectors
                    .iter()
                    .filter(|selector| selector.matches(context))
                    .map(PageSelector::specificity)
                    .max()
            });
            for declaration in &at_rule.declarations {
                if let Some(specificity) = specificity {
                    candidates.push(PageCandidate {
                        declaration: declaration.clone(),
                        origin,
                        layer_order: layer_order.rank(active_layer),
                        specificity,
                        source_order: *source_order,
                    });
                }
                *source_order += 1;
            }
            continue;
        }
        let Some(block) = at_rule.block.as_deref() else {
            continue;
        };
        if !matches!(
            at_rule.name.to_ascii_lowercase().as_str(),
            "layer" | "media" | "supports"
        ) || !layer_group_rule_is_active(
            at_rule,
            viewport_width,
            viewport_height,
            color_scheme_dark,
            media_type,
        ) {
            continue;
        }
        let path = if at_rule.name.eq_ignore_ascii_case("layer") {
            let Some(path) = layer_block_path(
                at_rule,
                stylesheet_id,
                active_layer.map(Vec::as_slice).unwrap_or(&[]),
            ) else {
                continue;
            };
            Some(path)
        } else {
            None
        };
        collect_page_candidates(
            block,
            origin,
            stylesheet_id,
            layer_order,
            path.as_ref().or(active_layer),
            context,
            viewport_width,
            viewport_height,
            color_scheme_dark,
            media_type,
            source_order,
            candidates,
        );
    }
}

impl StyleResolver {
    /// Resolves the declarations applying to one printed page.
    pub fn resolved_page_style(&self, context: &PageSelectorContext) -> ResolvedPageStyle {
        let mut candidates = Vec::new();
        let mut source_order = 0;
        for (position, (input, scope)) in self
            .stylesheets
            .iter()
            .zip(&self.stylesheet_scopes)
            .enumerate()
        {
            if scope.root.is_some() {
                continue;
            }
            let Some(layer_order) = self.layer_orders.get(&super::LayerContextKey {
                origin: input.origin,
                scope_root: None,
            }) else {
                continue;
            };
            collect_page_candidates(
                &input.stylesheet.rules,
                input.origin,
                self.stylesheet_ids[position],
                layer_order,
                None,
                context,
                self.viewport_width,
                self.viewport_height,
                self.color_scheme_dark,
                self.media_type,
                &mut source_order,
                &mut candidates,
            );
        }
        let mut winners: BTreeMap<String, PageCandidate> = BTreeMap::new();
        for candidate in candidates {
            let name = candidate.declaration.name.clone();
            if winners
                .get(&name)
                .is_none_or(|previous| candidate.outranks(previous))
            {
                winners.insert(name, candidate);
            }
        }
        ResolvedPageStyle {
            properties: winners
                .into_iter()
                .map(|(name, candidate)| (name, candidate.declaration.value))
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::css::{MediaType, parse_stylesheet};

    fn style(css: &str, context: &PageSelectorContext) -> ResolvedPageStyle {
        let mut resolver = StyleResolver::default();
        resolver.set_viewport(800.0, 1000.0);
        resolver.set_media_type(MediaType::Print);
        resolver.add_stylesheet(Origin::Author, parse_stylesheet(css).unwrap());
        resolver.resolved_page_style(context)
    }

    fn px(style: &ResolvedPageStyle, name: &str) -> f32 {
        match style.get(name) {
            Some(Value::Length(value, unit)) if unit == "cm" => value * 96.0 / 2.54,
            Some(Value::Length(value, unit)) if unit == "px" => *value,
            Some(Value::Number(value)) if *value == 0.0 => 0.0,
            other => panic!("unexpected {name} value: {other:?}"),
        }
    }

    #[test]
    fn page_selector_specificity_and_source_order() {
        let css = "@page { margin-left: 1cm } @page :left { margin-left: 2cm } \
                   @page a { margin-left: 3cm } @page a:first { margin-left: 4cm } \
                   @page a:first { margin-right: 5cm }";
        let first = style(css, &PageSelectorContext::new(0, Some("a".into())));
        assert!((px(&first, "margin-left") - 4.0 * 96.0 / 2.54).abs() < 0.01);
        assert!((px(&first, "margin-right") - 5.0 * 96.0 / 2.54).abs() < 0.01);
        let second = style(css, &PageSelectorContext::new(1, Some("a".into())));
        assert!((px(&second, "margin-left") - 3.0 * 96.0 / 2.54).abs() < 0.01);
        let anonymous = style(css, &PageSelectorContext::new(1, None));
        assert!((px(&anonymous, "margin-left") - 2.0 * 96.0 / 2.54).abs() < 0.01);
    }

    #[test]
    fn layers_resolve_page_margins_like_wpt_layers_001_to_004() {
        for (order, first_margin, named_margin) in
            [("one, two", 0.0, 0.0), ("two, one", 96.0 / 2.54, 0.0)]
        {
            let css = format!(
                "@layer {order}; \
                 @page b {{ margin: 0 }} \
                 @layer one {{ @page {{ margin: 1cm }} }} \
                 @layer two {{ @page {{ margin: 0 }} }}"
            );
            let first = style(&css, &PageSelectorContext::new(0, None));
            let named = style(&css, &PageSelectorContext::new(1, Some("b".into())));
            assert!((px(&first, "margin-top") - first_margin).abs() < 0.01);
            assert!((px(&named, "margin-top") - named_margin).abs() < 0.01);
        }
        for (order, named_first) in [("one, two", 2.0), ("two, one", 0.0)] {
            let css = format!(
                "@layer {order}; \
                 @page b {{ margin: 3cm }} \
                 @layer one {{ @page b {{ margin: 1cm }} @page :first {{ margin: 0 }} }} \
                 @layer two {{ @page b {{ margin: 0 }} @page a:first {{ margin: 2cm }} }}"
            );
            let first = style(&css, &PageSelectorContext::new(0, Some("a".into())));
            let named = style(&css, &PageSelectorContext::new(1, Some("b".into())));
            assert!((px(&first, "margin-top") - named_first * 96.0 / 2.54).abs() < 0.01);
            assert!((px(&named, "margin-top") - 3.0 * 96.0 / 2.54).abs() < 0.01);
        }
    }

    #[test]
    fn print_conditions_anonymous_nested_layers_and_important_order() {
        let css = "@media screen { @page { margin: 99cm } } \
                   @media print { @supports (display: block) { \
                     @layer outer { @layer { @page { margin-top: 1cm !important } } \
                                    @layer inner { @page { margin-top: 2cm !important } } } \
                   } } \
                   @page { margin-bottom: 3cm; @top-center { content: 'header' } }";
        let sheet = parse_stylesheet(css).unwrap();
        let mut resolver = StyleResolver::default();
        resolver.set_media_type(MediaType::Print);
        resolver.add_stylesheet(Origin::Author, sheet.clone());
        let page = resolver.resolved_page_style(&PageSelectorContext::new(0, None));
        assert!((px(&page, "margin-top") - 96.0 / 2.54).abs() < 0.01);
        assert!((px(&page, "margin-bottom") - 3.0 * 96.0 / 2.54).abs() < 0.01);
        resolver.set_media_type(MediaType::Screen);
        let screen = resolver.resolved_page_style(&PageSelectorContext::new(0, None));
        assert!((px(&screen, "margin-top") - 99.0 * 96.0 / 2.54).abs() < 0.01);
    }

    #[test]
    fn page_geometry_uses_selected_size_and_margins() {
        let page = style(
            "@page { size: 10cm 20cm; margin: 1cm 2cm 3cm 4cm }",
            &PageSelectorContext::new(0, None),
        );
        let geometry = page.geometry((800.0, 1000.0), 12.0);
        assert!((geometry.width - 10.0 * 96.0 / 2.54).abs() < 0.01);
        assert!((geometry.height - 20.0 * 96.0 / 2.54).abs() < 0.01);
        assert!((geometry.margin_top - 96.0 / 2.54).abs() < 0.01);
        assert!((geometry.margin_right - 2.0 * 96.0 / 2.54).abs() < 0.01);
        assert!((geometry.margin_bottom - 3.0 * 96.0 / 2.54).abs() < 0.01);
        assert!((geometry.margin_left - 4.0 * 96.0 / 2.54).abs() < 0.01);
    }

    #[test]
    fn page_origin_important_and_source_order_resolve_independently() {
        let mut resolver = StyleResolver::default();
        resolver.set_media_type(MediaType::Print);
        for (origin, css) in [
            (Origin::UserAgent, "@page { margin-top: 1cm !important }"),
            (Origin::User, "@page { margin-top: 2cm !important }"),
            (Origin::Author, "@page { margin-top: 3cm !important }"),
        ] {
            resolver.add_stylesheet(origin, parse_stylesheet(css).unwrap());
        }
        let page = resolver.resolved_page_style(&PageSelectorContext::new(0, None));
        assert!((px(&page, "margin-top") - 96.0 / 2.54).abs() < 0.01);

        let page = style(
            "@page { margin-left: 1cm } @page { margin-left: 2cm }",
            &PageSelectorContext::new(0, None),
        );
        assert!((px(&page, "margin-left") - 2.0 * 96.0 / 2.54).abs() < 0.01);
    }

    #[test]
    fn margin_at_rules_do_not_swallow_following_page_declarations() {
        let parsed = parse_stylesheet(
            "@page a { margin-top: 1cm; @top-center { content: 'title' } margin-left: 2cm }",
        )
        .unwrap();
        let Rule::At(page) = &parsed.rules[0] else {
            panic!("expected @page rule");
        };
        assert_eq!(page.declarations.len(), 2);
        assert_eq!(page.block.as_ref().unwrap().len(), 1);
        let page = style(
            "@page a { margin-top: 1cm; @top-center { content: 'title' } margin-left: 2cm }",
            &PageSelectorContext::new(0, Some("a".into())),
        );
        assert!((px(&page, "margin-left") - 2.0 * 96.0 / 2.54).abs() < 0.01);
    }
}
