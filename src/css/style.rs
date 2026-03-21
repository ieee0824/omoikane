//! CSS cascade and computed style resolution.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use crate::dom::{Node, NodeHandle, NodeType};
use rusqlite::{Connection, params};

use super::{
    PseudoElement, Rule, Specificity, Stylesheet, Value, matches_selector_with_pseudo, specificity,
};

/// CSS origin.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
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
}

/// Resolved computed style for a node.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ComputedStyle {
    properties: BTreeMap<String, ComputedValue>,
}

impl ComputedStyle {
    /// Returns a computed property.
    pub fn get(&self, name: &str) -> Option<&ComputedValue> {
        self.properties.get(name)
    }

    /// Returns all computed properties.
    pub fn properties(&self) -> &BTreeMap<String, ComputedValue> {
        &self.properties
    }
}

/// A stylesheet together with its cascade origin.
#[derive(Debug, Clone)]
pub struct StylesheetInput {
    pub origin: Origin,
    pub stylesheet: Stylesheet,
}

/// Computes styles and caches results per node.
#[derive(Debug, Default)]
pub struct StyleResolver {
    stylesheets: Vec<StylesheetInput>,
    cache: HashMap<usize, ComputedStyle>,
    pseudo_cache: HashMap<(usize, PseudoElement), ComputedStyle>,
}

static UNSUPPORTED_CSS_LOGGED: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
static UNSUPPORTED_CSS_CONFIG: OnceLock<UnsupportedCssConfig> = OnceLock::new();
static SQLITE_LOG_ERRORS: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
const MAX_UNSUPPORTED_LOG_KEYS: usize = 4096;
const MAX_UNSUPPORTED_LOG_VALUE_LEN: usize = 256;
const MAX_SQLITE_LOG_ERRORS: usize = 1024;

thread_local! {
    static SQLITE_CONNECTIONS: RefCell<HashMap<String, Connection>> = RefCell::new(HashMap::new());
}

#[derive(Debug, Clone)]
struct UnsupportedCssConfig {
    logging_enabled: bool,
    sqlite_path: Option<String>,
}

impl StyleResolver {
    /// Creates a new style resolver.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a stylesheet with its origin.
    pub fn add_stylesheet(&mut self, origin: Origin, stylesheet: Stylesheet) {
        self.stylesheets
            .push(StylesheetInput { origin, stylesheet });
        self.cache.clear();
        self.pseudo_cache.clear();
    }

    /// Resolves computed style for `node`, using the cache when possible.
    pub fn computed_style(&mut self, node: &NodeHandle) -> ComputedStyle {
        let key = node.identity();
        if let Some(style) = self.cache.get(&key) {
            return style.clone();
        }

        let inherited = node
            .parent_node()
            .map(|parent| self.computed_style(&parent));
        let style = self.compute_style(node, inherited.as_ref());
        self.cache.insert(key, style.clone());
        style
    }

    /// Resolves computed style for a pseudo-element attached to `node`.
    pub fn computed_pseudo_style(
        &mut self,
        node: &NodeHandle,
        pseudo: PseudoElement,
    ) -> Option<ComputedStyle> {
        let key = (node.identity(), pseudo);
        if let Some(style) = self.pseudo_cache.get(&key) {
            return Some(style.clone());
        }

        let parent_style = self.computed_style(node);
        let style = self.compute_style_with_pseudo(node, Some(&parent_style), Some(pseudo));
        if style.properties.is_empty() {
            return None;
        }

        self.pseudo_cache.insert(key, style.clone());
        Some(style)
    }

    fn compute_style(
        &self,
        node: &NodeHandle,
        parent_style: Option<&ComputedStyle>,
    ) -> ComputedStyle {
        self.compute_style_with_pseudo(node, parent_style, None)
    }

    fn compute_style_with_pseudo(
        &self,
        node: &NodeHandle,
        parent_style: Option<&ComputedStyle>,
        pseudo: Option<PseudoElement>,
    ) -> ComputedStyle {
        let mut candidates = Vec::new();
        let mut source_order = 0usize;

        for input in &self.stylesheets {
            collect_rule_candidates(
                node,
                &input.stylesheet.rules,
                input.origin,
                pseudo,
                &mut source_order,
                &mut candidates,
            );
        }

        candidates.sort_by(|left, right| {
            cascade_rank(left)
                .cmp(&cascade_rank(right))
                .then(left.specificity.cmp(&right.specificity))
                .then(left.source_order.cmp(&right.source_order))
        });

        let mut properties: BTreeMap<String, ComputedValue> = BTreeMap::new();

        // Process font-size first so that em units in other properties
        // resolve against the element's own computed font-size.
        if let Some(fs_candidate) = candidates.iter().filter(|c| c.name == "font-size").last() {
            let parent_fs = parent_style
                .and_then(|ps| ps.get("font-size"))
                .and_then(|v| match v {
                    ComputedValue::Px(px) => Some(*px),
                    _ => None,
                })
                .unwrap_or(16.0);
            let computed = compute_value(&fs_candidate.value, "font-size", parent_fs);
            properties.insert("font-size".to_string(), computed);
        }

        for candidate in candidates {
            if candidate.name == "font-size" {
                continue; // already processed above
            }
            log_unsupported_css_if_enabled(&candidate.name, &candidate.value);
            let font_size = inherited_font_size(parent_style, &properties);
            let computed = compute_value(&candidate.value, &candidate.name, font_size);
            // CSS 2.1: non-zero unitless numbers are invalid for length properties;
            // skip them so they don't override valid length values in the cascade.
            if matches!(computed, ComputedValue::Number(n) if n != 0.0)
                && is_length_property(&candidate.name)
            {
                continue;
            }
            properties.insert(candidate.name, computed);
        }

        apply_presentational_hints(node, &mut properties, pseudo);
        resolve_explicit_inherit(&mut properties, parent_style);
        apply_inheritance(&mut properties, parent_style);
        apply_initial_values(&mut properties);
        zero_border_width_for_none_style(&mut properties);

        ComputedStyle { properties }
    }
}

#[derive(Debug, Clone)]
struct Candidate {
    name: String,
    value: Value,
    important: bool,
    origin: Origin,
    specificity: Specificity,
    source_order: usize,
}

fn collect_rule_candidates(
    node: &NodeHandle,
    rules: &[Rule],
    origin: Origin,
    pseudo: Option<PseudoElement>,
    source_order: &mut usize,
    out: &mut Vec<Candidate>,
) {
    if node.node_type() != NodeType::Element {
        return;
    }

    for rule in rules {
        match rule {
            Rule::Style(style_rule) => {
                let matching_specificity = style_rule
                    .selectors
                    .iter()
                    .filter(|selector| matches_selector_with_pseudo(node, selector, pseudo))
                    .map(specificity)
                    .max();

                if let Some(specificity) = matching_specificity {
                    for declaration in &style_rule.declarations {
                        out.push(Candidate {
                            name: declaration.name.clone(),
                            value: declaration.value.clone(),
                            important: declaration.important,
                            origin,
                            specificity,
                            source_order: *source_order,
                        });
                        *source_order += 1;
                    }
                } else {
                    *source_order += style_rule.declarations.len();
                }
            }
            Rule::At(at_rule) => {
                if let Some(block) = &at_rule.block {
                    collect_rule_candidates(node, block, origin, pseudo, source_order, out);
                } else {
                    *source_order += at_rule.declarations.len();
                }
            }
        }
    }
}

fn is_length_property(name: &str) -> bool {
    matches!(
        name,
        "width"
            | "height"
            | "min-width"
            | "min-height"
            | "max-width"
            | "max-height"
            | "margin-top"
            | "margin-right"
            | "margin-bottom"
            | "margin-left"
            | "padding-top"
            | "padding-right"
            | "padding-bottom"
            | "padding-left"
            | "border-top-width"
            | "border-right-width"
            | "border-bottom-width"
            | "border-left-width"
            | "top"
            | "right"
            | "bottom"
            | "left"
            | "border-spacing"
    )
}

fn cascade_rank(candidate: &Candidate) -> (u8, u8) {
    let importance = if candidate.important { 1 } else { 0 };
    let origin = match (candidate.important, candidate.origin) {
        (true, Origin::User) => 5,
        (true, Origin::Author) => 4,
        (true, Origin::UserAgent) => 3,
        (false, Origin::Author) => 2,
        (false, Origin::User) => 1,
        (false, Origin::UserAgent) => 0,
    };
    (importance, origin)
}

fn log_unsupported_css_if_enabled(property: &str, value: &Value) {
    if should_ignore_unsupported_css_logging(property) || is_supported_property(property) {
        return;
    }

    let config = unsupported_css_config();
    if !config.logging_enabled && config.sqlite_path.is_none() {
        return;
    }

    let rendered_value = render_value(value);
    if let Some(path) = config.sqlite_path.as_deref() {
        persist_unsupported_css_to_sqlite(path, property, &rendered_value);
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
            eprintln!("[omoikane][unsupported-css] {property}={value}");
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
            first_seen_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            last_seen_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            occurrences INTEGER NOT NULL DEFAULT 0,
            PRIMARY KEY (property, value)
        );
        CREATE INDEX IF NOT EXISTS idx_unsupported_css_log_occurrences
        ON unsupported_css_log (occurrences DESC);",
    )?;
    Ok(())
}

fn persist_unsupported_css_to_sqlite(path: &str, property: &str, value: &str) {
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
            "INSERT INTO unsupported_css_log (property, value, occurrences)
             VALUES (?1, ?2, 1)
             ON CONFLICT(property, value) DO UPDATE SET
               occurrences = unsupported_css_log.occurrences + 1,
               last_seen_at = CURRENT_TIMESTAMP",
            params![property, value],
        )?;
        Ok(())
    });

    if let Err(error) = result {
        log_sqlite_error(&error);
    }
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

fn unsupported_css_dedup_key(property: &str, value: &str) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    value.hash(&mut hasher);
    format!("{property}#{}#{}", value.len(), hasher.finish())
}

fn truncate_log_value(value: &str, max_len: usize) -> String {
    if value.chars().count() <= max_len {
        return value.to_string();
    }
    let mut out = value.chars().take(max_len).collect::<String>();
    out.push_str("...");
    out
}

fn is_supported_property(name: &str) -> bool {
    matches!(
        name,
        "align-items"
            | "align-self"
            | "background-attachment"
            | "background-color"
            | "background-image"
            | "background-position-x"
            | "background-position-y"
            | "background-repeat"
            | "border-bottom-color"
            | "border-bottom-style"
            | "border-bottom-width"
            | "border-collapse"
            | "border-left-color"
            | "border-left-style"
            | "border-left-width"
            | "border-right-color"
            | "border-right-style"
            | "border-right-width"
            | "border-spacing"
            | "border-style"
            | "border-top-color"
            | "border-top-style"
            | "border-top-width"
            | "bottom"
            | "clear"
            | "color"
            | "content"
            | "display"
            | "flex-direction"
            | "flex-grow"
            | "flex-shrink"
            | "flex-wrap"
            | "float"
            | "font-family"
            | "font-size"
            | "font-style"
            | "font-weight"
            | "height"
            | "justify-content"
            | "left"
            | "line-height"
            | "margin-bottom"
            | "margin-left"
            | "margin-right"
            | "margin-top"
            | "max-height"
            | "max-width"
            | "min-height"
            | "min-width"
            | "overflow"
            | "padding-bottom"
            | "padding-left"
            | "padding-right"
            | "padding-top"
            | "position"
            | "right"
            | "text-align"
            | "top"
            | "vertical-align"
            | "visibility"
            | "white-space"
            | "width"
            | "z-index"
    )
}

fn compute_value(value: &Value, property_name: &str, parent_font_size: f32) -> ComputedValue {
    match value {
        Value::Keyword(keyword) => {
            if is_color_keyword(keyword)
                || property_name.ends_with("color")
                || property_name == "color"
            {
                ComputedValue::Color(keyword.clone())
            } else {
                ComputedValue::Keyword(keyword.clone())
            }
        }
        Value::Length(number, unit) => {
            let px = match unit.as_str() {
                "px" => *number,
                "em" => *number * parent_font_size,
                "mm" => *number * (96.0 / 25.4),
                "cm" => *number * (96.0 / 2.54),
                "in" => *number * 96.0,
                "pt" => *number * (96.0 / 72.0),
                "pc" => *number * (96.0 / 6.0),
                _ => *number,
            };
            ComputedValue::Px(px)
        }
        Value::Percentage(percent) => {
            if property_name == "font-size" {
                let px = parent_font_size * (*percent / 100.0);
                ComputedValue::Px(px)
            } else {
                ComputedValue::Percentage(*percent)
            }
        }
        Value::Color(color) => ComputedValue::Color(color.clone()),
        Value::String(value) => ComputedValue::String(value.clone()),
        Value::Number(value) => ComputedValue::Number(*value),
        Value::Function { name, arguments } if name.eq_ignore_ascii_case("rgb") => {
            let channels: Vec<u8> = arguments
                .iter()
                .map(|argument| match argument {
                    Value::Number(number) => *number as u8,
                    _ => 0,
                })
                .collect();
            if channels.len() == 3 {
                ComputedValue::Color(format!(
                    "#{:02x}{:02x}{:02x}",
                    channels[0], channels[1], channels[2]
                ))
            } else {
                ComputedValue::Keyword(name.clone())
            }
        }
        Value::Function { .. } => ComputedValue::Keyword(render_value(value)),
        Value::List(values) => {
            if let Some(first) = values.first() {
                compute_value(first, property_name, parent_font_size)
            } else {
                ComputedValue::Keyword(String::new())
            }
        }
    }
}

fn apply_presentational_hints(
    node: &NodeHandle,
    properties: &mut BTreeMap<String, ComputedValue>,
    pseudo: Option<PseudoElement>,
) {
    if pseudo.is_some() || node.node_type() != NodeType::Element {
        return;
    }

    let attributes = node.attributes().unwrap_or_default();

    if !properties.contains_key("background-color") {
        if let Some(background) = attributes
            .get("bgcolor")
            .and_then(|value| parse_legacy_color_hint(value))
        {
            properties.insert("background-color".to_string(), ComputedValue::Color(background));
        }
    }

    if !properties.contains_key("background-image") {
        if let Some(background) = attributes
            .get("background")
            .map(|value| value.trim())
            .filter(|value| !value.is_empty())
        {
            let escaped = background.replace('\\', "\\\\").replace('"', "\\\"");
            properties.insert(
                "background-image".to_string(),
                ComputedValue::Keyword(format!("url(\"{escaped}\")")),
            );
        }
    }

    if !properties.contains_key("color")
        && node
            .tag_name()
            .as_deref()
            .is_some_and(|name| name.eq_ignore_ascii_case("body"))
    {
        if let Some(color) = attributes
            .get("text")
            .and_then(|value| parse_legacy_color_hint(value))
        {
            properties.insert("color".to_string(), ComputedValue::Color(color));
        }
    }

    if !properties.contains_key("text-align") {
        if let Some(align) = attributes
            .get("align")
            .map(|value| value.trim().to_ascii_lowercase())
            .filter(|value| matches!(value.as_str(), "left" | "right" | "center" | "justify"))
        {
            properties.insert("text-align".to_string(), ComputedValue::Keyword(align));
        }
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

fn is_hex_color(value: &str) -> bool {
    (value.len() == 3 || value.len() == 6) && value.chars().all(|ch| ch.is_ascii_hexdigit())
}

fn apply_initial_values(properties: &mut BTreeMap<String, ComputedValue>) {
    properties
        .entry("color".to_string())
        .or_insert_with(|| ComputedValue::Color("black".to_string()));
    properties
        .entry("font-size".to_string())
        .or_insert_with(|| ComputedValue::Px(16.0));
}

/// CSS 2.1 §8.5.3: If border-style is 'none', the computed border-width is 0.
fn zero_border_width_for_none_style(properties: &mut BTreeMap<String, ComputedValue>) {
    for side in ["top", "right", "bottom", "left"] {
        let style_key = format!("border-{side}-style");
        let is_none = matches!(
            properties.get(&style_key),
            Some(ComputedValue::Keyword(keyword)) if keyword.eq_ignore_ascii_case("none")
        );
        if is_none {
            let width_key = format!("border-{side}-width");
            properties.insert(width_key, ComputedValue::Px(0.0));
        }
    }
}

fn resolve_explicit_inherit(
    properties: &mut BTreeMap<String, ComputedValue>,
    parent_style: Option<&ComputedStyle>,
) {
    let inherited_names: Vec<String> = properties
        .iter()
        .filter_map(|(name, value)| match value {
            ComputedValue::Keyword(keyword) if keyword.eq_ignore_ascii_case("inherit") => {
                Some(name.clone())
            }
            _ => None,
        })
        .collect();

    for name in inherited_names {
        if let Some(parent_style) = parent_style {
            if let Some(parent_value) = parent_style.get(&name) {
                properties.insert(name, parent_value.clone());
                continue;
            }
        }
        properties.remove(&name);
    }
}

fn apply_inheritance(
    properties: &mut BTreeMap<String, ComputedValue>,
    parent_style: Option<&ComputedStyle>,
) {
    let Some(parent_style) = parent_style else {
        return;
    };

    for inherited_name in ["color", "font-size", "line-height", "white-space"] {
        if !properties.contains_key(inherited_name) {
            if let Some(value) = parent_style.get(inherited_name) {
                properties.insert(inherited_name.to_string(), value.clone());
            }
        }
    }
}

fn inherited_font_size(
    parent_style: Option<&ComputedStyle>,
    current: &BTreeMap<String, ComputedValue>,
) -> f32 {
    if let Some(ComputedValue::Px(value)) = current.get("font-size") {
        return *value;
    }
    if let Some(parent_style) = parent_style {
        if let Some(ComputedValue::Px(value)) = parent_style.get("font-size") {
            return *value;
        }
    }
    16.0
}

fn is_color_keyword(keyword: &str) -> bool {
    matches!(
        keyword,
        "black" | "white" | "red" | "green" | "blue" | "gray" | "grey"
    )
}

fn render_value(value: &Value) -> String {
    match value {
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
        Value::String(value) => value.clone(),
        Value::Number(value) => value.to_string(),
        Value::Percentage(value) => format!("{value}%"),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    use crate::css::{PseudoElement, parse_stylesheet};
    use crate::dom::NodeHandle;

    use super::*;

    fn sample_tree() -> (NodeHandle, NodeHandle, NodeHandle, NodeHandle) {
        let document = NodeHandle::document();
        let html = NodeHandle::element("html");
        let body = NodeHandle::element("body");
        let title = NodeHandle::element("h1");

        title.set_attribute("id", "hero");
        title.set_attribute("class", "primary");

        document.append_child(html.clone());
        html.append_child(body.clone());
        body.append_child(title.clone());

        (document, body, title, html)
    }

    #[test]
    fn applies_origin_importance_specificity_and_source_order() {
        let (_document, _body, title, _html) = sample_tree();
        let mut resolver = StyleResolver::new();

        resolver.add_stylesheet(
            Origin::UserAgent,
            parse_stylesheet("h1 { color: black; }").unwrap(),
        );
        resolver.add_stylesheet(
            Origin::User,
            parse_stylesheet("h1 { color: green; }").unwrap(),
        );
        resolver.add_stylesheet(
            Origin::Author,
            parse_stylesheet("h1 { color: blue; } #hero { color: red !important; }").unwrap(),
        );

        let style = resolver.computed_style(&title);
        assert_eq!(
            style.get("color"),
            Some(&ComputedValue::Color("red".to_string()))
        );
    }

    #[test]
    fn important_user_rule_beats_important_author_rule() {
        let (_document, _body, title, _html) = sample_tree();
        let mut resolver = StyleResolver::new();

        resolver.add_stylesheet(
            Origin::Author,
            parse_stylesheet("#hero { color: red !important; }").unwrap(),
        );
        resolver.add_stylesheet(
            Origin::User,
            parse_stylesheet("h1 { color: green !important; }").unwrap(),
        );

        let style = resolver.computed_style(&title);
        assert_eq!(
            style.get("color"),
            Some(&ComputedValue::Color("green".to_string()))
        );
    }

    #[test]
    fn identifies_supported_property_names() {
        assert!(is_supported_property("background-color"));
        assert!(is_supported_property("position"));
        assert!(!is_supported_property("transform"));
        assert!(!is_supported_property("filter"));
    }

    #[test]
    fn applies_legacy_html_presentational_hints() {
        let document = NodeHandle::document();
        let html = NodeHandle::element("html");
        let body = NodeHandle::element("body");
        let cell = NodeHandle::element("td");
        cell.set_attribute("bgcolor", "336699");
        cell.set_attribute("align", "center");
        body.set_attribute("text", "#112233");
        body.set_attribute("background", "legacy/wallpaper.png");
        document.append_child(html.clone());
        html.append_child(body.clone());
        body.append_child(cell.clone());

        let mut resolver = StyleResolver::new();
        let body_style = resolver.computed_style(&body);
        let cell_style = resolver.computed_style(&cell);

        assert_eq!(
            body_style.get("color"),
            Some(&ComputedValue::Color("#112233".to_string()))
        );
        assert_eq!(
            body_style.get("background-image"),
            Some(&ComputedValue::Keyword(
                "url(\"legacy/wallpaper.png\")".to_string()
            ))
        );
        assert_eq!(
            cell_style.get("background-color"),
            Some(&ComputedValue::Color("#336699".to_string()))
        );
        assert_eq!(
            cell_style.get("text-align"),
            Some(&ComputedValue::Keyword("center".to_string()))
        );
    }

    #[test]
    fn sqlite_logging_creates_schema_and_accumulates_occurrences() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let db_path = std::env::temp_dir().join(format!("omoikane-unsupported-css-{unique}.db"));
        let db_path_str = db_path.to_string_lossy().to_string();

        persist_unsupported_css_to_sqlite(&db_path_str, "transform", "translateX(10px)");
        persist_unsupported_css_to_sqlite(&db_path_str, "transform", "translateX(10px)");
        persist_unsupported_css_to_sqlite(&db_path_str, "filter", "blur(4px)");

        let conn = Connection::open(&db_path_str).unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT property, value, occurrences
                 FROM unsupported_css_log
                 ORDER BY property, value",
            )
            .unwrap();
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();

        assert_eq!(rows.len(), 2);
        assert_eq!(
            rows[0],
            ("filter".to_string(), "blur(4px)".to_string(), 1_i64)
        );
        assert_eq!(
            rows[1],
            ("transform".to_string(), "translateX(10px)".to_string(), 2_i64)
        );

        drop(stmt);
        drop(conn);
        close_sqlite_connection_for_path(&db_path_str);
        let _ = fs::remove_file(db_path);
    }

    #[test]
    fn ignores_custom_properties_for_unsupported_logging() {
        assert!(should_ignore_unsupported_css_logging("--brand-color"));
        assert!(!should_ignore_unsupported_css_logging("transform"));
    }

    #[test]
    fn inherits_color_and_font_size() {
        let (document, body, title, html) = sample_tree();
        let mut resolver = StyleResolver::new();

        resolver.add_stylesheet(
            Origin::Author,
            parse_stylesheet("body { color: blue; font-size: 20px; }").unwrap(),
        );

        let _ = document;
        let _ = html;
        let body_style = resolver.computed_style(&body);
        let title_style = resolver.computed_style(&title);

        assert_eq!(
            body_style.get("color"),
            Some(&ComputedValue::Color("blue".to_string()))
        );
        assert_eq!(
            title_style.get("color"),
            Some(&ComputedValue::Color("blue".to_string()))
        );
        assert_eq!(title_style.get("font-size"), Some(&ComputedValue::Px(20.0)));
    }

    #[test]
    fn resolves_em_and_percentage_font_sizes() {
        let (document, _body, title, html) = sample_tree();
        let mut resolver = StyleResolver::new();

        resolver.add_stylesheet(
            Origin::Author,
            parse_stylesheet("body { font-size: 20px; } h1 { margin-top: 2em; font-size: 150%; }")
                .unwrap(),
        );

        let _ = document;
        let _ = html;
        let style = resolver.computed_style(&title);

        assert_eq!(style.get("font-size"), Some(&ComputedValue::Px(30.0)));
        // CSS 2.1 §4.3.2: em unit uses the element's own computed font-size
        assert_eq!(style.get("margin-top"), Some(&ComputedValue::Px(60.0)));
    }

    #[test]
    fn caches_computed_styles() {
        let (_document, _body, title, _html) = sample_tree();
        let mut resolver = StyleResolver::new();
        resolver.add_stylesheet(
            Origin::Author,
            parse_stylesheet("h1 { color: blue; }").unwrap(),
        );

        let first = resolver.computed_style(&title);
        let second = resolver.computed_style(&title);

        assert_eq!(first, second);
        assert!(resolver.cache.len() >= 1);
    }

    #[test]
    fn applies_initial_values_when_no_rule_matches() {
        let (_document, _body, title, _html) = sample_tree();
        let mut resolver = StyleResolver::new();

        let style = resolver.computed_style(&title);
        assert_eq!(
            style.get("color"),
            Some(&ComputedValue::Color("black".to_string()))
        );
        assert_eq!(style.get("font-size"), Some(&ComputedValue::Px(16.0)));
    }

    #[test]
    fn keeps_pseudo_element_rules_out_of_normal_computed_style() {
        let (_document, _body, title, _html) = sample_tree();
        let mut resolver = StyleResolver::new();
        resolver.add_stylesheet(
            Origin::Author,
            parse_stylesheet("h1::before { content: \"prefix\"; color: red; }").unwrap(),
        );

        let style = resolver.computed_style(&title);
        assert_eq!(style.get("content"), None);
        assert_eq!(
            style.get("color"),
            Some(&ComputedValue::Color("black".to_string()))
        );
    }

    #[test]
    fn resolves_computed_style_for_pseudo_elements() {
        let (_document, _body, title, _html) = sample_tree();
        let mut resolver = StyleResolver::new();
        resolver.add_stylesheet(
            Origin::Author,
            parse_stylesheet("h1 { color: blue; } h1::before { content: \"prefix\"; }").unwrap(),
        );

        let style = resolver
            .computed_pseudo_style(&title, PseudoElement::Before)
            .unwrap();
        assert_eq!(
            style.get("content"),
            Some(&ComputedValue::String("prefix".to_string()))
        );
        assert_eq!(
            style.get("color"),
            Some(&ComputedValue::Color("blue".to_string()))
        );
    }

    #[test]
    fn resolves_explicit_inherit_keyword_from_parent() {
        let (_document, body, title, _html) = sample_tree();
        let mut resolver = StyleResolver::new();
        resolver.add_stylesheet(
            Origin::Author,
            parse_stylesheet("body { float: right; } h1 { float: inherit; }").unwrap(),
        );

        let body_style = resolver.computed_style(&body);
        let title_style = resolver.computed_style(&title);

        assert_eq!(
            body_style.get("float"),
            Some(&ComputedValue::Keyword("right".to_string()))
        );
        assert_eq!(
            title_style.get("float"),
            Some(&ComputedValue::Keyword("right".to_string()))
        );
    }

    #[test]
    fn border_style_none_zeroes_side_width_even_when_width_only_comes_from_shorthand() {
        let (_document, _body, title, _html) = sample_tree();
        let mut resolver = StyleResolver::new();
        resolver.add_stylesheet(
            Origin::Author,
            parse_stylesheet("h1 { border: solid 12px transparent; border-style: none solid; }")
                .unwrap(),
        );

        let style = resolver.computed_style(&title);

        assert_eq!(
            style.get("border-top-style"),
            Some(&ComputedValue::Keyword("none".to_string()))
        );
        assert_eq!(
            style.get("border-bottom-style"),
            Some(&ComputedValue::Keyword("none".to_string()))
        );
        assert_eq!(style.get("border-top-width"), Some(&ComputedValue::Px(0.0)));
        assert_eq!(
            style.get("border-bottom-width"),
            Some(&ComputedValue::Px(0.0))
        );
        assert_eq!(
            style.get("border-right-style"),
            Some(&ComputedValue::Keyword("solid".to_string()))
        );
        assert_eq!(
            style.get("border-left-style"),
            Some(&ComputedValue::Keyword("solid".to_string()))
        );
    }
}
