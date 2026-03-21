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
        let mut custom_properties = inherited_custom_properties(parent_style);
        for candidate in &candidates {
            if candidate.name.starts_with("--") {
                custom_properties.insert(candidate.name.clone(), candidate.value.clone());
            }
        }

        // Process font-size first so that em units in other properties
        // resolve against the element's own computed font-size.
        if let Some(fs_candidate) = candidates.iter().filter(|c| c.name == "font-size").last() {
            if let Some(resolved_value) =
                resolve_value_with_custom_properties(&fs_candidate.value, &custom_properties)
            {
                let parent_fs = parent_style
                    .and_then(|ps| ps.get("font-size"))
                    .and_then(|v| match v {
                        ComputedValue::Px(px) => Some(*px),
                        _ => None,
                    })
                    .unwrap_or(16.0);
                let computed = compute_value(&resolved_value, "font-size", parent_fs);
                properties.insert("font-size".to_string(), computed);
            }
        }

        for candidate in candidates {
            if candidate.name == "font-size" {
                continue; // already processed above
            }
            log_unsupported_css_if_enabled(&candidate.name, &candidate.value);
            let Some(resolved_value) =
                resolve_value_with_custom_properties(&candidate.value, &custom_properties)
            else {
                continue;
            };
            let font_size = inherited_font_size(parent_style, &properties);
            if candidate.name == "gap" {
                if let Some((row_gap, column_gap)) =
                    compute_gap_shorthand(&resolved_value, font_size)
                {
                    insert_computed_property(&mut properties, "row-gap", row_gap);
                    insert_computed_property(&mut properties, "column-gap", column_gap);
                }
                continue;
            }
            let computed = compute_value(&resolved_value, &candidate.name, font_size);
            insert_computed_property(&mut properties, &candidate.name, computed);
        }

        apply_ua_defaults(node, &mut properties, pseudo, parent_style);
        apply_presentational_hints(node, &mut properties, pseudo);
        resolve_explicit_inherit(&mut properties, parent_style);
        apply_inheritance(&mut properties, parent_style);
        apply_initial_values(&mut properties);
        zero_border_width_for_none_style(&mut properties);

        ComputedStyle { properties }
    }
}

fn compute_gap_shorthand(
    value: &Value,
    parent_font_size: f32,
) -> Option<(ComputedValue, ComputedValue)> {
    match value {
        Value::List(values) => match values.as_slice() {
            [single] => {
                let computed = compute_value(single, "row-gap", parent_font_size);
                if should_skip_computed_property("row-gap", &computed) {
                    None
                } else {
                    Some((computed.clone(), computed))
                }
            }
            [row, column] => {
                let row_gap = compute_value(row, "row-gap", parent_font_size);
                let column_gap = compute_value(column, "column-gap", parent_font_size);
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
            let computed = compute_value(value, "row-gap", parent_font_size);
            if should_skip_computed_property("row-gap", &computed) {
                None
            } else {
                Some((computed.clone(), computed))
            }
        }
    }
}

fn insert_computed_property(
    properties: &mut BTreeMap<String, ComputedValue>,
    name: &str,
    computed: ComputedValue,
) {
    if should_skip_computed_property(name, &computed) {
        return;
    }
    properties.insert(name.to_string(), computed);
}

fn should_skip_computed_property(name: &str, computed: &ComputedValue) -> bool {
    // CSS 2.1: non-zero unitless numbers are invalid for length properties;
    // skip them so they don't override valid length values in the cascade.
    matches!(computed, ComputedValue::Number(n) if *n != 0.0) && is_length_property(name)
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

    let rendered_value = sanitize_unsupported_css_log_value(&render_value(value));
    if let Some(path) = config.sqlite_path.as_deref() {
        persist_unsupported_css_to_sqlite(path, property, &rendered_value);
        if let Some(top_n) = config.top_n {
            emit_unsupported_css_top_n_summary_if_updated(path, top_n);
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

fn emit_unsupported_css_top_n_summary_if_updated(path: &str, top_n: usize) {
    let rows = SQLITE_CONNECTIONS.with(|connections| {
        let mut connections = connections.borrow_mut();
        let Some(conn) = connections.get_mut(path) else {
            return Ok(Vec::new());
        };
        query_unsupported_css_top_n(conn, top_n)
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
    for (property, value, occurrences) in &rows {
        property.hash(&mut hasher);
        value.hash(&mut hasher);
        occurrences.hash(&mut hasher);
    }
    let digest = hasher.finish();
    let key = format!("{path}#{top_n}");
    let map = UNSUPPORTED_CSS_TOP_N_LAST_DIGEST.get_or_init(|| Mutex::new(HashMap::new()));
    let mut map = map
        .lock()
        .expect("unsupported css top-n digest lock poisoned");
    if map.get(&key).copied() == Some(digest) {
        return;
    }
    map.insert(key, digest);

    eprintln!("[omoikane][unsupported-css][top-n] top {top_n} candidates (site/url anonymized)");
    for (index, (property, value, occurrences)) in rows.iter().enumerate() {
        let value = truncate_log_value(value, MAX_UNSUPPORTED_LOG_VALUE_LEN);
        eprintln!(
            "[omoikane][unsupported-css][top-n] {}. {}={} (count={})",
            index + 1,
            property,
            value,
            occurrences
        );
    }
}

fn query_unsupported_css_top_n(
    conn: &Connection,
    top_n: usize,
) -> Result<Vec<(String, String, i64)>, rusqlite::Error> {
    let limit = i64::try_from(top_n).unwrap_or(i64::MAX);
    let mut stmt = conn.prepare(
        "SELECT property, value, occurrences
         FROM unsupported_css_log
         ORDER BY occurrences DESC, property ASC, value ASC
         LIMIT ?1",
    )?;
    stmt.query_map(params![limit], |row| {
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
            | "gap"
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
            | "column-gap"
            | "overflow"
            | "overflow-x"
            | "overflow-y"
            | "padding-bottom"
            | "padding-left"
            | "padding-right"
            | "padding-top"
            | "position"
            | "right"
            | "row-gap"
            | "transform"
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
        Value::Function { name, arguments }
            if name.eq_ignore_ascii_case("rgb") || name.eq_ignore_ascii_case("rgba") =>
        {
            if let Some(hex) = compute_rgb_function(arguments) {
                ComputedValue::Color(hex)
            } else {
                ComputedValue::Keyword(name.clone())
            }
        }
        Value::Function { name, arguments }
            if name.eq_ignore_ascii_case("hsl") || name.eq_ignore_ascii_case("hsla") =>
        {
            if let Some(hex) = compute_hsl_function(arguments) {
                ComputedValue::Color(hex)
            } else {
                ComputedValue::Keyword(name.clone())
            }
        }
        Value::Function { name, arguments } if name.eq_ignore_ascii_case("calc") => {
            if let Some(quantity) = evaluate_calc(arguments, parent_font_size) {
                return match quantity.unit {
                    CalcUnit::Px => ComputedValue::Px(quantity.value),
                    CalcUnit::Percentage => {
                        if property_name == "font-size" {
                            ComputedValue::Px(parent_font_size * (quantity.value / 100.0))
                        } else {
                            ComputedValue::Percentage(quantity.value)
                        }
                    }
                    CalcUnit::Unitless => ComputedValue::Number(quantity.value),
                };
            }
            ComputedValue::Keyword(render_value(value))
        }
        Value::Function { .. } => ComputedValue::Keyword(render_value(value)),
        Value::List(values) => {
            if property_name.eq_ignore_ascii_case("transform")
                || property_name.eq_ignore_ascii_case("overflow")
            {
                return ComputedValue::Keyword(render_value(value));
            }
            if property_name.eq_ignore_ascii_case("font-family") {
                return ComputedValue::Keyword(render_font_family_value(values));
            }
            if let Some(first) = values.first() {
                compute_value(first, property_name, parent_font_size)
            } else {
                ComputedValue::Keyword(String::new())
            }
        }
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

fn evaluate_calc(arguments: &[Value], parent_font_size: f32) -> Option<CalcQuantity> {
    let expression = arguments.first()?;
    let mut tokens = Vec::new();
    collect_calc_tokens(expression, parent_font_size, &mut tokens)?;
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
    parent_font_size: f32,
    out: &mut Vec<CalcToken>,
) -> Option<()> {
    match value {
        Value::List(values) => {
            for item in values {
                collect_calc_tokens(item, parent_font_size, out)?;
            }
            Some(())
        }
        Value::Keyword(op) if matches!(op.as_str(), "+" | "-" | "*" | "/") => {
            out.push(CalcToken::Operator(op.chars().next()?));
            Some(())
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
                _ => return None,
            };
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
        _ => None,
    }
}

fn parse_calc_add_sub(tokens: &[CalcToken], index: &mut usize) -> Option<CalcQuantity> {
    let mut left = parse_calc_mul_div(tokens, index)?;
    loop {
        let op = match tokens.get(*index) {
            Some(CalcToken::Operator(op @ ('+' | '-'))) => *op,
            _ => break,
        };
        *index += 1;
        let right = parse_calc_mul_div(tokens, index)?;
        left = apply_calc_operator(left, op, right)?;
    }
    Some(left)
}

fn parse_calc_mul_div(tokens: &[CalcToken], index: &mut usize) -> Option<CalcQuantity> {
    let mut left = parse_calc_factor(tokens, index)?;
    loop {
        let op = match tokens.get(*index) {
            Some(CalcToken::Operator(op @ ('*' | '/'))) => *op,
            _ => break,
        };
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
    if right.value == 0.0 || right.unit != CalcUnit::Unitless {
        return None;
    }
    Some(CalcQuantity {
        value: left.value / right.value,
        unit: left.unit,
    })
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
            properties.insert(
                "background-color".to_string(),
                ComputedValue::Color(background),
            );
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

    if let Some(align) = attributes
        .get("align")
        .map(|value| value.trim().to_ascii_lowercase())
        .filter(|value| matches!(value.as_str(), "left" | "right" | "center" | "justify"))
    {
        if !properties.contains_key("text-align") {
            properties.insert(
                "text-align".to_string(),
                ComputedValue::Keyword(align.clone()),
            );
        }
        // For block/table elements, align="center" means auto margins (structural centering)
        if align == "center" {
            let is_table_or_block = node
                .tag_name()
                .as_deref()
                .is_some_and(|tag| {
                    matches!(
                        tag.to_ascii_lowercase().as_str(),
                        "table" | "div" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "p"
                    )
                });
            if is_table_or_block {
                if !properties.contains_key("margin-left") {
                    properties.insert(
                        "margin-left".to_string(),
                        ComputedValue::Keyword("auto".to_string()),
                    );
                }
                if !properties.contains_key("margin-right") {
                    properties.insert(
                        "margin-right".to_string(),
                        ComputedValue::Keyword("auto".to_string()),
                    );
                }
            }
        }
    }

    if !properties.contains_key("width") {
        if let Some(width) = attributes
            .get("width")
            .and_then(|value| parse_legacy_dimension_hint(value))
        {
            properties.insert("width".to_string(), width);
        }
    }

    if !properties.contains_key("height") {
        if let Some(height) = attributes
            .get("height")
            .and_then(|value| parse_legacy_dimension_hint(value))
        {
            properties.insert("height".to_string(), height);
        }
    }

    if !properties.contains_key("color") {
        if let Some(color) = attributes
            .get("color")
            .and_then(|value| parse_legacy_color_hint(value))
        {
            properties.insert("color".to_string(), ComputedValue::Color(color));
        }
    }

    if !properties.contains_key("font-family") {
        if let Some(face) = attributes
            .get("face")
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
        {
            properties.insert("font-family".to_string(), ComputedValue::Keyword(face));
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
    properties: &mut BTreeMap<String, ComputedValue>,
    pseudo: Option<PseudoElement>,
    parent_style: Option<&ComputedStyle>,
) {
    if pseudo.is_some() || node.node_type() != NodeType::Element {
        return;
    }
    let tag = match node.tag_name() {
        Some(tag) => tag.to_ascii_lowercase(),
        None => return,
    };
    let parent_font_size = inherited_font_size(parent_style, properties);

    // UA stylesheet defaults per CSS 2.1 Appendix D / HTML spec
    struct UaDefaults {
        font_size_em: f32,
        font_weight_bold: bool,
        margin_em: f32,
    }

    let defaults = match tag.as_str() {
        "h1" => Some(UaDefaults { font_size_em: 2.0, font_weight_bold: true, margin_em: 0.67 }),
        "h2" => Some(UaDefaults { font_size_em: 1.5, font_weight_bold: true, margin_em: 0.83 }),
        "h3" => Some(UaDefaults { font_size_em: 1.17, font_weight_bold: true, margin_em: 1.0 }),
        "h4" => Some(UaDefaults { font_size_em: 1.0, font_weight_bold: true, margin_em: 1.33 }),
        "h5" => Some(UaDefaults { font_size_em: 0.83, font_weight_bold: true, margin_em: 1.67 }),
        "h6" => Some(UaDefaults { font_size_em: 0.67, font_weight_bold: true, margin_em: 2.33 }),
        _ => None,
    };

    if let Some(defaults) = defaults {
        // Determine the element's final font size: use existing CSS value if present,
        // otherwise apply the UA default multiplier to the inherited size.
        let element_font_size =
            if let Some(ComputedValue::Px(existing_px)) = properties.get("font-size") {
                *existing_px
            } else {
                let computed = defaults.font_size_em * parent_font_size;
                properties
                    .entry("font-size".to_string())
                    .or_insert(ComputedValue::Px(computed));
                computed
            };
        let margin_px = defaults.margin_em * element_font_size;
        if defaults.font_weight_bold {
            properties
                .entry("font-weight".to_string())
                .or_insert(ComputedValue::Keyword("bold".to_string()));
        }
        properties
            .entry("margin-top".to_string())
            .or_insert(ComputedValue::Px(margin_px));
        properties
            .entry("margin-bottom".to_string())
            .or_insert(ComputedValue::Px(margin_px));
        return;
    }

    match tag.as_str() {
        "p" => {
            let em = parent_font_size;
            properties.entry("margin-top".to_string()).or_insert(ComputedValue::Px(em));
            properties.entry("margin-bottom".to_string()).or_insert(ComputedValue::Px(em));
        }
        "b" | "strong" => {
            properties.entry("font-weight".to_string()).or_insert(ComputedValue::Keyword("bold".to_string()));
        }
        "i" | "em" => {
            properties.entry("font-style".to_string()).or_insert(ComputedValue::Keyword("italic".to_string()));
        }
        "hr" => {
            properties.entry("border-top-style".to_string()).or_insert(ComputedValue::Keyword("inset".to_string()));
            properties.entry("border-top-width".to_string()).or_insert(ComputedValue::Px(1.0));
            let half_em = parent_font_size * 0.5;
            properties.entry("margin-top".to_string()).or_insert(ComputedValue::Px(half_em));
            properties.entry("margin-bottom".to_string()).or_insert(ComputedValue::Px(half_em));
        }
        _ => {}
    }
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

    for inherited_name in [
        "color",
        "font-family",
        "font-size",
        "line-height",
        "white-space",
    ] {
        if !properties.contains_key(inherited_name) {
            if let Some(value) = parent_style.get(inherited_name) {
                properties.insert(inherited_name.to_string(), value.clone());
            }
        }
    }

    // CSS custom properties inherit by default.
    for (name, value) in parent_style.properties() {
        if name.starts_with("--") && !properties.contains_key(name) {
            properties.insert(name.clone(), value.clone());
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
    if arguments.len() == 1 {
        if let Value::List(items) = &arguments[0] {
            return items.iter().collect();
        }
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
        [r, g, b, ..] => (*r as u8, *g as u8, *b as u8),
        _ => return None,
    };

    format_color_hex(r, g, b, a)
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
    let a = alpha.or_else(|| {
        flat.get(3).and_then(|v| extract_alpha(v))
    });

    let (h, s, l) = match numbers.as_slice() {
        [h, s, l, ..] => (*h, *s, *l),
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
fn hsl_to_rgb(h: f32, s: f32, l: f32) -> (u8, u8, u8) {
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

fn render_font_family_value(values: &[Value]) -> String {
    values
        .iter()
        .map(render_value)
        .collect::<Vec<_>>()
        .join(", ")
}

fn inherited_custom_properties(parent_style: Option<&ComputedStyle>) -> BTreeMap<String, Value> {
    let mut custom_properties = BTreeMap::new();
    let Some(parent_style) = parent_style else {
        return custom_properties;
    };
    for (name, value) in parent_style.properties() {
        if !name.starts_with("--") {
            continue;
        }
        custom_properties.insert(name.clone(), computed_to_value(value));
    }
    custom_properties
}

fn computed_to_value(value: &ComputedValue) -> Value {
    match value {
        ComputedValue::Keyword(value) => Value::Keyword(value.clone()),
        ComputedValue::Px(value) => Value::Length(*value, "px".to_string()),
        ComputedValue::Percentage(value) => Value::Percentage(*value),
        ComputedValue::Color(value) => Value::Color(value.clone()),
        ComputedValue::String(value) => Value::String(value.clone()),
        ComputedValue::Number(value) => Value::Number(*value),
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
                resolved_values.push(resolve_value_with_custom_properties_inner(
                    item,
                    custom_properties,
                    stack,
                    depth + 1,
                )?);
            }
            Some(Value::List(resolved_values))
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
