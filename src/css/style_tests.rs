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
