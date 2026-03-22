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
    assert!(is_supported_property("transform"));
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
    cell.set_attribute("width", "50%");
    cell.set_attribute("height", "24px");
    cell.set_attribute("face", "Hiragino Sans, sans-serif");
    body.set_attribute("text", "#112233");
    body.set_attribute("background", "legacy/wallpaper.png");
    body.set_attribute("width", "640");
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
    assert_eq!(body_style.get("width"), Some(&ComputedValue::Px(640.0)));
    assert_eq!(
        cell_style.get("background-color"),
        Some(&ComputedValue::Color("#336699".to_string()))
    );
    assert_eq!(
        cell_style.get("text-align"),
        Some(&ComputedValue::Keyword("center".to_string()))
    );
    assert_eq!(
        cell_style.get("width"),
        Some(&ComputedValue::Percentage(50.0))
    );
    assert_eq!(cell_style.get("height"), Some(&ComputedValue::Px(24.0)));
    assert_eq!(
        cell_style.get("font-family"),
        Some(&ComputedValue::Keyword(
            "Hiragino Sans, sans-serif".to_string()
        ))
    );
}

#[test]
fn ignores_invalid_legacy_dimension_hints() {
    let document = NodeHandle::document();
    let html = NodeHandle::element("html");
    let body = NodeHandle::element("body");
    let cell = NodeHandle::element("td");
    cell.set_attribute("width", "abc");
    cell.set_attribute("height", "");
    document.append_child(html.clone());
    html.append_child(body.clone());
    body.append_child(cell.clone());

    let mut resolver = StyleResolver::new();
    let cell_style = resolver.computed_style(&cell);

    assert!(!cell_style.properties().contains_key("width"));
    assert!(!cell_style.properties().contains_key("height"));
}

#[test]
fn keeps_comma_separated_font_family_value() {
    let (_document, _body, title, _html) = sample_tree();
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("h1 { font-family: Arial, sans-serif; }").unwrap(),
    );

    let style = resolver.computed_style(&title);
    assert_eq!(
        style.get("font-family"),
        Some(&ComputedValue::Keyword("Arial, sans-serif".to_string()))
    );
}

#[test]
fn keeps_transform_list_values_in_computed_style() {
    let (_document, _body, title, _html) = sample_tree();
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("h1 { transform: translateX(10px) translateY(6px); }").unwrap(),
    );

    let style = resolver.computed_style(&title);
    let value = match style.get("transform") {
        Some(ComputedValue::Keyword(value)) => value.to_ascii_lowercase(),
        other => panic!("unexpected transform value: {other:?}"),
    };
    assert!(value.contains("translatex(10px)"));
    assert!(value.contains("translatey(6px)"));
}

#[test]
fn expands_two_value_gap_shorthand_into_row_and_column_gap() {
    let (_document, _body, title, _html) = sample_tree();
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("h1 { gap: 10px 20px; }").unwrap(),
    );

    let style = resolver.computed_style(&title);
    assert_eq!(style.get("row-gap"), Some(&ComputedValue::Px(10.0)));
    assert_eq!(style.get("column-gap"), Some(&ComputedValue::Px(20.0)));
    assert_eq!(style.get("gap"), None);
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
        (
            "transform".to_string(),
            "translateX(10px)".to_string(),
            2_i64
        )
    );

    drop(stmt);
    drop(conn);
    close_sqlite_connection_for_path(&db_path_str);
    let _ = fs::remove_file(db_path);
}

#[test]
fn sqlite_top_n_query_orders_by_occurrences() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let db_path = std::env::temp_dir().join(format!("omoikane-unsupported-css-topn-{unique}.db"));
    let db_path_str = db_path.to_string_lossy().to_string();

    persist_unsupported_css_to_sqlite(&db_path_str, "transform", "translateX(10px)");
    persist_unsupported_css_to_sqlite(&db_path_str, "transform", "translateX(10px)");
    persist_unsupported_css_to_sqlite(&db_path_str, "filter", "blur(4px)");
    persist_unsupported_css_to_sqlite(&db_path_str, "backdrop-filter", "blur(4px)");
    persist_unsupported_css_to_sqlite(&db_path_str, "backdrop-filter", "blur(4px)");
    persist_unsupported_css_to_sqlite(&db_path_str, "backdrop-filter", "blur(4px)");

    let conn = Connection::open(&db_path_str).unwrap();
    let rows = query_unsupported_css_top_n(&conn, 2).unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].0, "backdrop-filter");
    assert_eq!(rows[0].2, 3);
    assert_eq!(rows[1].0, "transform");
    assert_eq!(rows[1].2, 2);

    drop(conn);
    close_sqlite_connection_for_path(&db_path_str);
    let _ = fs::remove_file(db_path);
}

#[test]
fn sanitizes_url_like_values_in_unsupported_css_logging() {
    let value = "url(\"https://example.com/a?x=1\") blur(4px) data:image/png;base64,AAAABBBB";
    let sanitized = sanitize_unsupported_css_log_value(value);
    assert!(!sanitized.contains("example.com"));
    assert!(!sanitized.contains("data:image"));
    assert!(!sanitized.contains("AAAABBBB"));
    assert!(sanitized.contains("[redacted-url]"));
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
    // h1 UA default: 2em = 40px (parent body 20px * 2)
    assert_eq!(title_style.get("font-size"), Some(&ComputedValue::Px(40.0)));
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
    // h1 UA default: 2em = 32px (parent 16px * 2)
    assert_eq!(style.get("font-size"), Some(&ComputedValue::Px(32.0)));
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

#[test]
fn resolves_var_from_inherited_root_custom_properties() {
    let (_document, body, title, _html) = sample_tree();
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(
            ":root { --theme: rgb(255, 255, 255); --primary: #123456; } \
             body { background-color: var(--theme); color: var(--primary); }",
        )
        .unwrap(),
    );

    let body_style = resolver.computed_style(&body);
    let title_style = resolver.computed_style(&title);

    assert_eq!(
        body_style.get("background-color"),
        Some(&ComputedValue::Color("#ffffff".to_string()))
    );
    assert_eq!(
        body_style.get("color"),
        Some(&ComputedValue::Color("#123456".to_string()))
    );
    assert_eq!(
        title_style.get("color"),
        Some(&ComputedValue::Color("#123456".to_string()))
    );
}

#[test]
fn resolves_var_with_fallback_for_missing_custom_property() {
    let (_document, _body, title, _html) = sample_tree();
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("h1 { color: var(--missing-color, blue); }").unwrap(),
    );

    let style = resolver.computed_style(&title);
    assert_eq!(
        style.get("color"),
        Some(&ComputedValue::Color("blue".to_string()))
    );
}

#[test]
fn drops_declaration_when_var_cannot_be_resolved() {
    let (_document, _body, title, _html) = sample_tree();
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("h1 { color: var(--missing-color); }").unwrap(),
    );

    let style = resolver.computed_style(&title);
    assert_eq!(
        style.get("color"),
        Some(&ComputedValue::Color("black".to_string()))
    );
}

#[test]
fn resolves_calc_with_var_lengths() {
    let (_document, body, _title, _html) = sample_tree();
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(
            ":root { --main-width: 720px; --gap: 24px; } \
             body { max-width: calc(var(--main-width) + var(--gap) * 2); }",
        )
        .unwrap(),
    );

    let style = resolver.computed_style(&body);
    assert_eq!(style.get("max-width"), Some(&ComputedValue::Px(768.0)));
}

#[test]
fn resolves_calc_with_var_lengths_without_operator_whitespace() {
    let (_document, body, _title, _html) = sample_tree();
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(
            ":root { --main-width: 720px; --gap: 24px; } \
             body { max-width: calc(var(--main-width)+var(--gap)*2); }",
        )
        .unwrap(),
    );

    let style = resolver.computed_style(&body);
    assert_eq!(style.get("max-width"), Some(&ComputedValue::Px(768.0)));
}

#[test]
fn computes_rgba_function_to_hex_with_alpha() {
    let (_document, _body, title, _html) = sample_tree();
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("h1 { color: rgba(255, 0, 0, 0.5); }").unwrap(),
    );

    let style = resolver.computed_style(&title);
    // rgba(255, 0, 0, 0.5) → r=255 g=0 b=0 a=128(0x80)
    assert_eq!(
        style.get("color"),
        Some(&ComputedValue::Color("#ff000080".to_string()))
    );
}

#[test]
fn computes_rgba_fully_opaque_to_hex() {
    let (_document, _body, title, _html) = sample_tree();
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("h1 { color: rgba(0, 128, 255, 1); }").unwrap(),
    );

    let style = resolver.computed_style(&title);
    assert_eq!(
        style.get("color"),
        Some(&ComputedValue::Color("#0080ff".to_string()))
    );
}

#[test]
fn computes_hsl_function_to_hex() {
    let (_document, _body, title, _html) = sample_tree();
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("h1 { color: hsl(0, 100%, 50%); }").unwrap(),
    );

    let style = resolver.computed_style(&title);
    // hsl(0, 100%, 50%) → pure red
    assert_eq!(
        style.get("color"),
        Some(&ComputedValue::Color("#ff0000".to_string()))
    );
}

#[test]
fn computes_hsl_green_to_hex() {
    let (_document, _body, title, _html) = sample_tree();
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("h1 { color: hsl(120, 100%, 50%); }").unwrap(),
    );

    let style = resolver.computed_style(&title);
    // hsl(120, 100%, 50%) → pure green
    assert_eq!(
        style.get("color"),
        Some(&ComputedValue::Color("#00ff00".to_string()))
    );
}

#[test]
fn computes_hsla_function_to_hex_with_alpha() {
    let (_document, _body, title, _html) = sample_tree();
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("h1 { color: hsla(240, 100%, 50%, 0.5); }").unwrap(),
    );

    let style = resolver.computed_style(&title);
    // hsla(240, 100%, 50%, 0.5) → semi-transparent blue a=128(0x80)
    assert_eq!(
        style.get("color"),
        Some(&ComputedValue::Color("#0000ff80".to_string()))
    );
}

#[test]
fn computes_rgb_modern_syntax_with_alpha() {
    let (_document, _body, title, _html) = sample_tree();
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("h1 { color: rgb(255 0 0 / 0.5); }").unwrap(),
    );

    let style = resolver.computed_style(&title);
    // rgb(255 0 0 / 0.5) → semi-transparent red a=128(0x80)
    assert_eq!(
        style.get("color"),
        Some(&ComputedValue::Color("#ff000080".to_string()))
    );
}

#[test]
fn computes_rgb_modern_syntax_no_alpha() {
    let (_document, _body, title, _html) = sample_tree();
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("h1 { color: rgb(0 128 255); }").unwrap(),
    );

    let style = resolver.computed_style(&title);
    assert_eq!(
        style.get("color"),
        Some(&ComputedValue::Color("#0080ff".to_string()))
    );
}

#[test]
fn computes_named_color_coral() {
    let (_document, _body, title, _html) = sample_tree();
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("h1 { color: coral; }").unwrap(),
    );

    let style = resolver.computed_style(&title);
    assert_eq!(
        style.get("color"),
        Some(&ComputedValue::Color("coral".to_string()))
    );
}

#[test]
fn computes_named_color_crimson() {
    let (_document, _body, title, _html) = sample_tree();
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("h1 { color: crimson; }").unwrap(),
    );

    let style = resolver.computed_style(&title);
    assert_eq!(
        style.get("color"),
        Some(&ComputedValue::Color("crimson".to_string()))
    );
}

#[test]
fn computes_rgba_percentage_alpha() {
    let (_document, _body, title, _html) = sample_tree();
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("h1 { color: rgba(255, 0, 0, 50%); }").unwrap(),
    );
    let style = resolver.computed_style(&title);
    // 50% alpha = 0.5 → hex alpha 80
    assert_eq!(
        style.get("color"),
        Some(&ComputedValue::Color("#ff000080".to_string()))
    );
}

#[test]
fn computes_hsl_wraps_hue_above_360() {
    let (_document, _body, title, _html) = sample_tree();
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("h1 { color: hsl(720, 100%, 50%); }").unwrap(),
    );
    let style = resolver.computed_style(&title);
    // 720 mod 360 = 0 → red
    assert_eq!(
        style.get("color"),
        Some(&ComputedValue::Color("#ff0000".to_string()))
    );
}

#[test]
fn computes_hsl_wraps_negative_hue() {
    let (_document, _body, title, _html) = sample_tree();
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("h1 { color: hsl(-120, 100%, 50%); }").unwrap(),
    );
    let style = resolver.computed_style(&title);
    // -120 mod 360 = 240 → blue
    assert_eq!(
        style.get("color"),
        Some(&ComputedValue::Color("#0000ff".to_string()))
    );
}

// --- shorthand 展開テスト ---

#[test]
fn expands_margin_1_value() {
    let stylesheet = parse_stylesheet("div { margin: 10px; }").unwrap();
    let Rule::Style(rule) = &stylesheet.rules[0] else {
        panic!("expected style rule");
    };
    for side in ["margin-top", "margin-right", "margin-bottom", "margin-left"] {
        assert!(
            rule.declarations
                .iter()
                .any(|d| d.name == side && matches!(&d.value, Value::Length(v, u) if *v == 10.0 && u == "px")),
            "{side} not found with 10px"
        );
    }
}

#[test]
fn expands_margin_2_values() {
    let stylesheet = parse_stylesheet("div { margin: 10px 20px; }").unwrap();
    let Rule::Style(rule) = &stylesheet.rules[0] else {
        panic!("expected style rule");
    };
    // top/bottom = 10px, right/left = 20px
    for side in ["margin-top", "margin-bottom"] {
        assert!(
            rule.declarations
                .iter()
                .any(|d| d.name == side && matches!(&d.value, Value::Length(v, u) if *v == 10.0 && u == "px")),
            "{side} not found with 10px"
        );
    }
    for side in ["margin-right", "margin-left"] {
        assert!(
            rule.declarations
                .iter()
                .any(|d| d.name == side && matches!(&d.value, Value::Length(v, u) if *v == 20.0 && u == "px")),
            "{side} not found with 20px"
        );
    }
}

#[test]
fn expands_margin_3_values() {
    let stylesheet = parse_stylesheet("div { margin: 10px 20px 30px; }").unwrap();
    let Rule::Style(rule) = &stylesheet.rules[0] else {
        panic!("expected style rule");
    };
    // top=10px, right/left=20px, bottom=30px
    assert!(rule.declarations.iter().any(
        |d| d.name == "margin-top" && matches!(&d.value, Value::Length(v, u) if *v == 10.0 && u == "px")
    ));
    assert!(rule.declarations.iter().any(
        |d| d.name == "margin-right" && matches!(&d.value, Value::Length(v, u) if *v == 20.0 && u == "px")
    ));
    assert!(rule.declarations.iter().any(
        |d| d.name == "margin-bottom" && matches!(&d.value, Value::Length(v, u) if *v == 30.0 && u == "px")
    ));
    assert!(rule.declarations.iter().any(
        |d| d.name == "margin-left" && matches!(&d.value, Value::Length(v, u) if *v == 20.0 && u == "px")
    ));
}

#[test]
fn expands_margin_4_values() {
    let stylesheet = parse_stylesheet("div { margin: 1px 2px 3px 4px; }").unwrap();
    let Rule::Style(rule) = &stylesheet.rules[0] else {
        panic!("expected style rule");
    };
    let expected = [
        ("margin-top", 1.0f32),
        ("margin-right", 2.0),
        ("margin-bottom", 3.0),
        ("margin-left", 4.0),
    ];
    for (side, px) in &expected {
        assert!(
            rule.declarations
                .iter()
                .any(|d| d.name == *side && matches!(&d.value, Value::Length(v, u) if *v == *px && u == "px")),
            "{side} not found with {px}px"
        );
    }
}

#[test]
fn expands_padding_4_values() {
    let stylesheet = parse_stylesheet("div { padding: 1px 2px 3px 4px; }").unwrap();
    let Rule::Style(rule) = &stylesheet.rules[0] else {
        panic!("expected style rule");
    };
    let expected = [
        ("padding-top", 1.0f32),
        ("padding-right", 2.0),
        ("padding-bottom", 3.0),
        ("padding-left", 4.0),
    ];
    for (side, px) in &expected {
        assert!(
            rule.declarations
                .iter()
                .any(|d| d.name == *side && matches!(&d.value, Value::Length(v, u) if *v == *px && u == "px")),
            "{side} not found with {px}px"
        );
    }
}

#[test]
fn expands_border_width_4_values() {
    let stylesheet = parse_stylesheet("div { border-width: 1px 2px 3px 4px; }").unwrap();
    let Rule::Style(rule) = &stylesheet.rules[0] else {
        panic!("expected style rule");
    };
    let expected = [
        ("border-top-width", 1.0f32),
        ("border-right-width", 2.0),
        ("border-bottom-width", 3.0),
        ("border-left-width", 4.0),
    ];
    for (side, px) in &expected {
        assert!(
            rule.declarations
                .iter()
                .any(|d| d.name == *side && matches!(&d.value, Value::Length(v, u) if *v == *px && u == "px")),
            "{side} not found with {px}px"
        );
    }
}

#[test]
fn expands_border_color_2_values() {
    let stylesheet = parse_stylesheet("div { border-color: red blue; }").unwrap();
    let Rule::Style(rule) = &stylesheet.rules[0] else {
        panic!("expected style rule");
    };
    for side in ["border-top-color", "border-bottom-color"] {
        assert!(
            rule.declarations
                .iter()
                .any(|d| d.name == side && matches!(&d.value, Value::Keyword(v) if v == "red")),
            "{side} not found with red"
        );
    }
    for side in ["border-right-color", "border-left-color"] {
        assert!(
            rule.declarations
                .iter()
                .any(|d| d.name == side && matches!(&d.value, Value::Keyword(v) if v == "blue")),
            "{side} not found with blue"
        );
    }
}

#[test]
fn expands_overflow_1_value() {
    let stylesheet = parse_stylesheet("div { overflow: hidden; }").unwrap();
    let Rule::Style(rule) = &stylesheet.rules[0] else {
        panic!("expected style rule");
    };
    for prop in ["overflow-x", "overflow-y"] {
        assert!(
            rule.declarations
                .iter()
                .any(|d| d.name == prop && matches!(&d.value, Value::Keyword(v) if v == "hidden")),
            "{prop} not found with hidden"
        );
    }
}

#[test]
fn expands_overflow_2_values() {
    let stylesheet = parse_stylesheet("div { overflow: auto scroll; }").unwrap();
    let Rule::Style(rule) = &stylesheet.rules[0] else {
        panic!("expected style rule");
    };
    assert!(
        rule.declarations
            .iter()
            .any(|d| d.name == "overflow-x" && matches!(&d.value, Value::Keyword(v) if v == "auto")),
        "overflow-x not found with auto"
    );
    assert!(
        rule.declarations
            .iter()
            .any(|d| d.name == "overflow-y" && matches!(&d.value, Value::Keyword(v) if v == "scroll")),
        "overflow-y not found with scroll"
    );
}

#[test]
fn expands_flex_shorthand_grow_shrink_basis() {
    let stylesheet = parse_stylesheet("div { flex: 2 1 100px; }").unwrap();
    let Rule::Style(rule) = &stylesheet.rules[0] else {
        panic!("expected style rule");
    };
    assert!(
        rule.declarations
            .iter()
            .any(|d| d.name == "flex-grow" && matches!(&d.value, Value::Number(v) if *v == 2.0)),
        "flex-grow not found with 2"
    );
    assert!(
        rule.declarations
            .iter()
            .any(|d| d.name == "flex-shrink" && matches!(&d.value, Value::Number(v) if *v == 1.0)),
        "flex-shrink not found with 1"
    );
    assert!(
        rule.declarations
            .iter()
            .any(|d| d.name == "flex-basis" && matches!(&d.value, Value::Length(v, u) if *v == 100.0 && u == "px")),
        "flex-basis not found with 100px"
    );
}

#[test]
fn expands_flex_shorthand_1_value_number() {
    // flex: 2 → flex-grow: 2, flex-shrink: 1, flex-basis: 0
    let stylesheet = parse_stylesheet("div { flex: 2; }").unwrap();
    let Rule::Style(rule) = &stylesheet.rules[0] else {
        panic!("expected style rule");
    };
    assert!(
        rule.declarations
            .iter()
            .any(|d| d.name == "flex-grow" && matches!(&d.value, Value::Number(v) if *v == 2.0)),
        "flex-grow not found with 2"
    );
    assert!(
        rule.declarations
            .iter()
            .any(|d| d.name == "flex-shrink" && matches!(&d.value, Value::Number(v) if *v == 1.0)),
        "flex-shrink not found with 1"
    );
    assert!(
        rule.declarations
            .iter()
            .any(|d| d.name == "flex-basis" && matches!(&d.value, Value::Number(v) if *v == 0.0)),
        "flex-basis not found with 0"
    );
}

#[test]
fn expands_flex_shorthand_none() {
    // flex: none → flex-grow: 0, flex-shrink: 0, flex-basis: auto
    let stylesheet = parse_stylesheet("div { flex: none; }").unwrap();
    let Rule::Style(rule) = &stylesheet.rules[0] else {
        panic!("expected style rule");
    };
    assert!(
        rule.declarations
            .iter()
            .any(|d| d.name == "flex-grow" && matches!(&d.value, Value::Number(v) if *v == 0.0)),
        "flex-grow not found with 0"
    );
    assert!(
        rule.declarations
            .iter()
            .any(|d| d.name == "flex-shrink" && matches!(&d.value, Value::Number(v) if *v == 0.0)),
        "flex-shrink not found with 0"
    );
    assert!(
        rule.declarations
            .iter()
            .any(|d| d.name == "flex-basis" && matches!(&d.value, Value::Keyword(v) if v == "auto")),
        "flex-basis not found with auto"
    );
}

#[test]
fn expands_flex_shorthand_auto() {
    // flex: auto → flex-grow: 1, flex-shrink: 1, flex-basis: auto
    let stylesheet = parse_stylesheet("div { flex: auto; }").unwrap();
    let Rule::Style(rule) = &stylesheet.rules[0] else {
        panic!("expected style rule");
    };
    assert!(
        rule.declarations
            .iter()
            .any(|d| d.name == "flex-grow" && matches!(&d.value, Value::Number(v) if *v == 1.0)),
        "flex-grow not found with 1"
    );
    assert!(
        rule.declarations
            .iter()
            .any(|d| d.name == "flex-shrink" && matches!(&d.value, Value::Number(v) if *v == 1.0)),
        "flex-shrink not found with 1"
    );
    assert!(
        rule.declarations
            .iter()
            .any(|d| d.name == "flex-basis" && matches!(&d.value, Value::Keyword(v) if v == "auto")),
        "flex-basis not found with auto"
    );
}

#[test]
fn expands_flex_shorthand_basis_only() {
    // flex: 100px → flex-grow: 1, flex-shrink: 1, flex-basis: 100px
    let stylesheet = parse_stylesheet("div { flex: 100px; }").unwrap();
    let Rule::Style(rule) = &stylesheet.rules[0] else {
        panic!("expected style rule");
    };
    assert!(
        rule.declarations
            .iter()
            .any(|d| d.name == "flex-grow" && matches!(&d.value, Value::Number(v) if *v == 1.0)),
        "flex-grow not found with 1"
    );
    assert!(
        rule.declarations
            .iter()
            .any(|d| d.name == "flex-shrink" && matches!(&d.value, Value::Number(v) if *v == 1.0)),
        "flex-shrink not found with 1"
    );
    assert!(
        rule.declarations
            .iter()
            .any(|d| d.name == "flex-basis" && matches!(&d.value, Value::Length(v, u) if *v == 100.0 && u == "px")),
        "flex-basis not found with 100px"
    );
}

#[test]
fn expands_flex_shorthand_grow_basis() {
    // flex: 2 100px → flex-grow: 2, flex-shrink: 1, flex-basis: 100px
    let stylesheet = parse_stylesheet("div { flex: 2 100px; }").unwrap();
    let Rule::Style(rule) = &stylesheet.rules[0] else {
        panic!("expected style rule");
    };
    assert!(
        rule.declarations
            .iter()
            .any(|d| d.name == "flex-grow" && matches!(&d.value, Value::Number(v) if *v == 2.0)),
        "flex-grow not found with 2"
    );
    assert!(
        rule.declarations
            .iter()
            .any(|d| d.name == "flex-shrink" && matches!(&d.value, Value::Number(v) if *v == 1.0)),
        "flex-shrink not found with 1"
    );
    assert!(
        rule.declarations
            .iter()
            .any(|d| d.name == "flex-basis" && matches!(&d.value, Value::Length(v, u) if *v == 100.0 && u == "px")),
        "flex-basis not found with 100px"
    );
}

// ===== text-decoration shorthand tests =====

#[test]
fn expands_text_decoration_shorthand_underline() {
    let stylesheet = parse_stylesheet("a { text-decoration: underline; }").unwrap();
    let rule = match &stylesheet.rules[0] {
        crate::css::Rule::Style(r) => r,
        _ => panic!("expected style rule"),
    };
    assert!(
        rule.declarations
            .iter()
            .any(|d| d.name == "text-decoration-line"
                && matches!(&d.value, Value::Keyword(v) if v == "underline")),
        "text-decoration-line: underline not found"
    );
}

#[test]
fn expands_text_decoration_shorthand_line_through_with_color() {
    let stylesheet =
        parse_stylesheet("del { text-decoration: line-through red; }").unwrap();
    let rule = match &stylesheet.rules[0] {
        crate::css::Rule::Style(r) => r,
        _ => panic!("expected style rule"),
    };
    assert!(
        rule.declarations
            .iter()
            .any(|d| d.name == "text-decoration-line"
                && matches!(&d.value, Value::Keyword(v) if v == "line-through")),
        "text-decoration-line: line-through not found"
    );
    assert!(
        rule.declarations
            .iter()
            .any(|d| d.name == "text-decoration-color"),
        "text-decoration-color not found"
    );
}

#[test]
fn expands_text_decoration_shorthand_solid_style() {
    let stylesheet = parse_stylesheet("u { text-decoration: underline solid; }").unwrap();
    let rule = match &stylesheet.rules[0] {
        crate::css::Rule::Style(r) => r,
        _ => panic!("expected style rule"),
    };
    assert!(
        rule.declarations
            .iter()
            .any(|d| d.name == "text-decoration-style"
                && matches!(&d.value, Value::Keyword(v) if v == "solid")),
        "text-decoration-style: solid not found"
    );
}

#[test]
fn expands_text_decoration_shorthand_none() {
    let stylesheet = parse_stylesheet("a { text-decoration: none; }").unwrap();
    let rule = match &stylesheet.rules[0] {
        crate::css::Rule::Style(r) => r,
        _ => panic!("expected style rule"),
    };
    assert!(
        rule.declarations
            .iter()
            .any(|d| d.name == "text-decoration-line"
                && matches!(&d.value, Value::Keyword(v) if v == "none")),
        "text-decoration-line: none not found"
    );
}

// ===== text-transform compute tests =====

#[test]
fn computes_text_transform_uppercase() {
    let (_document, _body, title, _html) = sample_tree();
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("h1 { text-transform: uppercase; }").unwrap(),
    );
    let style = resolver.computed_style(&title);
    assert_eq!(
        style.get("text-transform"),
        Some(&ComputedValue::Keyword("uppercase".to_string()))
    );
}

// ===== letter-spacing inheritance tests =====

#[test]
fn letter_spacing_inherits_from_parent() {
    let document = NodeHandle::document();
    let html = NodeHandle::element("html");
    let body = NodeHandle::element("body");
    let span = NodeHandle::element("span");
    document.append_child(html.clone());
    html.append_child(body.clone());
    body.append_child(span.clone());

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("body { letter-spacing: 2px; }").unwrap(),
    );
    let style = resolver.computed_style(&span);
    assert_eq!(
        style.get("letter-spacing"),
        Some(&ComputedValue::Px(2.0)),
        "letter-spacing should inherit from parent"
    );
}

#[test]
fn word_spacing_inherits_from_parent() {
    let document = NodeHandle::document();
    let html = NodeHandle::element("html");
    let body = NodeHandle::element("body");
    let p = NodeHandle::element("p");
    document.append_child(html.clone());
    html.append_child(body.clone());
    body.append_child(p.clone());

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("body { word-spacing: 4px; }").unwrap(),
    );
    let style = resolver.computed_style(&p);
    assert_eq!(
        style.get("word-spacing"),
        Some(&ComputedValue::Px(4.0)),
        "word-spacing should inherit from parent"
    );
}

// --- rem / viewport unit tests ---

#[test]
fn resolves_rem_using_root_font_size() {
    // rem は root element の font-size (デフォルト 16px) を基準にする
    let document = NodeHandle::document();
    let html = NodeHandle::element("html");
    let body = NodeHandle::element("body");
    let div = NodeHandle::element("div");
    document.append_child(html.clone());
    html.append_child(body.clone());
    body.append_child(div.clone());

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("div { margin-top: 2rem; }").unwrap(),
    );
    // root font-size = 20px → 2rem = 40px
    resolver.set_root_font_size(20.0);
    let style = resolver.computed_style(&div);
    assert_eq!(
        style.get("margin-top"),
        Some(&ComputedValue::Px(40.0)),
        "2rem with root font-size 20px should be 40px"
    );
}

#[test]
fn resolves_rem_default_root_font_size() {
    // root font-size が未設定の場合はデフォルト 16px を使う
    let document = NodeHandle::document();
    let html = NodeHandle::element("html");
    let body = NodeHandle::element("body");
    let div = NodeHandle::element("div");
    document.append_child(html.clone());
    html.append_child(body.clone());
    body.append_child(div.clone());

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("div { padding-left: 1.5rem; }").unwrap(),
    );
    // デフォルト root font-size 16px → 1.5rem = 24px
    let style = resolver.computed_style(&div);
    assert_eq!(
        style.get("padding-left"),
        Some(&ComputedValue::Px(24.0)),
        "1.5rem with default root font-size 16px should be 24px"
    );
}

#[test]
fn resolves_vw_unit() {
    let document = NodeHandle::document();
    let html = NodeHandle::element("html");
    let body = NodeHandle::element("body");
    let div = NodeHandle::element("div");
    document.append_child(html.clone());
    html.append_child(body.clone());
    body.append_child(div.clone());

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("div { width: 50vw; }").unwrap(),
    );
    // viewport 幅 1000px → 50vw = 500px
    resolver.set_viewport(1000.0, 800.0);
    let style = resolver.computed_style(&div);
    assert_eq!(
        style.get("width"),
        Some(&ComputedValue::Px(500.0)),
        "50vw with viewport width 1000px should be 500px"
    );
}

#[test]
fn resolves_vh_unit() {
    let document = NodeHandle::document();
    let html = NodeHandle::element("html");
    let body = NodeHandle::element("body");
    let div = NodeHandle::element("div");
    document.append_child(html.clone());
    html.append_child(body.clone());
    body.append_child(div.clone());

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("div { height: 100vh; }").unwrap(),
    );
    // viewport 高さ 600px → 100vh = 600px
    resolver.set_viewport(1200.0, 600.0);
    let style = resolver.computed_style(&div);
    assert_eq!(
        style.get("height"),
        Some(&ComputedValue::Px(600.0)),
        "100vh with viewport height 600px should be 600px"
    );
}

#[test]
fn resolves_vmin_unit() {
    let document = NodeHandle::document();
    let html = NodeHandle::element("html");
    let body = NodeHandle::element("body");
    let div = NodeHandle::element("div");
    document.append_child(html.clone());
    html.append_child(body.clone());
    body.append_child(div.clone());

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("div { width: 10vmin; }").unwrap(),
    );
    // viewport 1000x600 → vmin = 600px の 1% → 10vmin = 60px
    resolver.set_viewport(1000.0, 600.0);
    let style = resolver.computed_style(&div);
    assert_eq!(
        style.get("width"),
        Some(&ComputedValue::Px(60.0)),
        "10vmin with viewport 1000x600 should be 60px"
    );
}

#[test]
fn resolves_vmax_unit() {
    let document = NodeHandle::document();
    let html = NodeHandle::element("html");
    let body = NodeHandle::element("body");
    let div = NodeHandle::element("div");
    document.append_child(html.clone());
    html.append_child(body.clone());
    body.append_child(div.clone());

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("div { width: 10vmax; }").unwrap(),
    );
    // viewport 1000x600 → vmax = 1000px の 1% → 10vmax = 100px
    resolver.set_viewport(1000.0, 600.0);
    let style = resolver.computed_style(&div);
    assert_eq!(
        style.get("width"),
        Some(&ComputedValue::Px(100.0)),
        "10vmax with viewport 1000x600 should be 100px"
    );
}

#[test]
fn resolves_rem_in_font_size() {
    // font-size に rem を使った場合
    let document = NodeHandle::document();
    let html = NodeHandle::element("html");
    let body = NodeHandle::element("body");
    let div = NodeHandle::element("div");
    document.append_child(html.clone());
    html.append_child(body.clone());
    body.append_child(div.clone());

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("div { font-size: 1.5rem; }").unwrap(),
    );
    // root font-size = 16px → 1.5rem = 24px
    resolver.set_root_font_size(16.0);
    let style = resolver.computed_style(&div);
    assert_eq!(
        style.get("font-size"),
        Some(&ComputedValue::Px(24.0)),
        "1.5rem font-size with root font-size 16px should be 24px"
    );
}

#[test]
fn rem_resolves_from_css_defined_root_font_size() {
    // html の font-size が CSS で指定されていれば、rem はその値を使う
    let document = NodeHandle::document();
    let html = NodeHandle::element("html");
    let body = NodeHandle::element("body");
    let div = NodeHandle::element("div");
    document.append_child(html.clone());
    html.append_child(body.clone());
    body.append_child(div.clone());

    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        // html の font-size を 20px に設定
        parse_stylesheet("html { font-size: 20px; } div { margin-top: 2rem; }").unwrap(),
    );
    // set_root_font_size() を呼ばなくても CSS の html font-size から自動計算される
    let _ = resolver.computed_style(&html); // html のスタイルを先に解決
    let style = resolver.computed_style(&div);
    assert_eq!(
        style.get("margin-top"),
        Some(&ComputedValue::Px(40.0)),
        "2rem should resolve from CSS-defined root font-size of 20px"
    );
}
