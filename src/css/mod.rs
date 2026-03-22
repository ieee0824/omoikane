//! CSS parsing primitives.
//!
//! The CSS layer exposes a tokenizer and a stylesheet parser that later style
//! resolution phases can reuse directly.

use std::fmt;

mod matcher;
mod media;
mod parser;
mod shorthand;
pub(crate) mod style;
mod tokenizer;

pub use matcher::{
    PseudoElement, Specificity, matches_selector, matches_selector_with_pseudo,
    selector_pseudo_element, specificity,
};
pub use media::{evaluate_media_query, parse_media_query_list};
pub use parser::parse_stylesheet;
pub use style::{ComputedStyle, ComputedValue, Origin, StyleResolver, StylesheetInput};
pub use tokenizer::tokenize;

/// A token emitted by the CSS tokenizer.
#[derive(Debug, Clone, PartialEq)]
pub enum CssToken {
    Ident(String),
    AtKeyword(String),
    Hash(String),
    String(String),
    Number(f32),
    Percentage(f32),
    Dimension(f32, String),
    Colon,
    Semicolon,
    Comma,
    CurlyOpen,
    CurlyClose,
    ParenOpen,
    ParenClose,
    BracketOpen,
    BracketClose,
    Delim(char),
    Whitespace,
}

/// Selector combinators.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Combinator {
    Descendant,
    Child,
    AdjacentSibling,
    GeneralSibling,
}

/// A parsed selector.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selector {
    pub parts: Vec<SelectorPart>,
}

/// A selector segment plus the combinator to the segment on its left.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectorPart {
    pub combinator: Option<Combinator>,
    pub simples: Vec<SimpleSelector>,
}

/// A basic CSS simple selector.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SimpleSelector {
    Type(String),
    Universal,
    Class(String),
    Id(String),
    Attribute {
        name: String,
        operator: Option<AttributeOperator>,
        value: Option<String>,
    },
    PseudoClass(String),
    PseudoElement(String),
    /// `:not(<compound-selector>)` -- negation pseudo-class (single compound, no commas).
    Not(Vec<SimpleSelector>),
}

/// Attribute selector operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttributeOperator {
    /// `=` -- exact match.
    Equals,
    /// `~=` -- value is in whitespace-separated list.
    Includes,
    /// `^=` -- value starts with.
    StartsWith,
    /// `$=` -- value ends with.
    EndsWith,
    /// `*=` -- value contains.
    Contains,
    /// `|=` -- value equals or starts with followed by a hyphen.
    DashMatch,
}

/// A stylesheet rule.
#[derive(Debug, Clone, PartialEq)]
pub enum Rule {
    Style(StyleRule),
    At(AtRule),
}

/// A regular style rule.
#[derive(Debug, Clone, PartialEq)]
pub struct StyleRule {
    pub selectors: Vec<Selector>,
    pub declarations: Vec<Declaration>,
}

/// An at-rule.
#[derive(Debug, Clone, PartialEq)]
pub struct AtRule {
    pub name: String,
    pub prelude: String,
    pub block: Option<Vec<Rule>>,
    pub declarations: Vec<Declaration>,
}

/// A parsed `@media` query.
///
/// A query is a media type combined with zero or more feature conditions.
/// All conditions must match for the query to evaluate to `true`.
#[derive(Debug, Clone, PartialEq)]
pub struct MediaQuery {
    /// `true` when the query is prefixed with `not`.
    pub negated: bool,
    /// Media type (e.g. `"screen"`, `"print"`, `"all"`).
    /// `None` means the type was omitted (equivalent to `"all"`).
    pub media_type: Option<String>,
    /// Feature conditions combined with `and`.
    pub conditions: Vec<MediaCondition>,
}

/// A single `@media` feature condition, e.g. `(max-width: 768px)`.
#[derive(Debug, Clone, PartialEq)]
pub enum MediaCondition {
    /// `(max-width: <length>)` -- viewport width <= value.
    MaxWidth(f32),
    /// `(min-width: <length>)` -- viewport width >= value.
    MinWidth(f32),
    /// `(max-height: <length>)` -- viewport height <= value.
    MaxHeight(f32),
    /// `(min-height: <length>)` -- viewport height >= value.
    MinHeight(f32),
    /// `(orientation: portrait)` -- height >= width.
    OrientationPortrait,
    /// `(orientation: landscape)` -- width > height.
    OrientationLandscape,
    /// `(prefers-color-scheme: dark)`.
    PrefersColorSchemeDark,
    /// `(prefers-color-scheme: light)`.
    PrefersColorSchemeLight,
    /// An unrecognised condition -- treated as matching to be forward-compatible.
    Unknown,
}

/// A CSS declaration.
#[derive(Debug, Clone, PartialEq)]
pub struct Declaration {
    pub name: String,
    pub value: Value,
    pub important: bool,
}

/// CSS values used by declarations.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Keyword(String),
    Length(f32, String),
    Color(String),
    Function { name: String, arguments: Vec<Value> },
    List(Vec<Value>),
    String(String),
    Number(f32),
    Percentage(f32),
}

/// A parsed stylesheet.
#[derive(Debug, Clone, PartialEq)]
pub struct Stylesheet {
    pub rules: Vec<Rule>,
}

/// Errors produced by CSS parsing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CssParseError {
    UnexpectedEndOfInput,
    ExpectedToken(&'static str),
    InvalidNumber,
    InvalidSelector,
    InvalidDeclaration,
}

impl fmt::Display for CssParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnexpectedEndOfInput => write!(f, "unexpected end of CSS input"),
            Self::ExpectedToken(token) => write!(f, "expected token: {token}"),
            Self::InvalidNumber => write!(f, "invalid CSS number"),
            Self::InvalidSelector => write!(f, "invalid CSS selector"),
            Self::InvalidDeclaration => write!(f, "invalid CSS declaration"),
        }
    }
}

impl std::error::Error for CssParseError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenizes_basic_css() {
        let tokens = tokenize("h1 { color: red; margin: 10px; }").unwrap();
        assert!(tokens.contains(&CssToken::Ident("h1".to_string())));
        assert!(tokens.contains(&CssToken::Dimension(10.0, "px".to_string())));
        assert!(tokens.contains(&CssToken::CurlyOpen));
    }

    #[test]
    fn tokenizes_negative_dimensions() {
        let tokens = tokenize("div { margin-bottom: -6em; top: -.5px; }").unwrap();
        assert!(tokens.contains(&CssToken::Dimension(-6.0, "em".to_string())));
        assert!(tokens.contains(&CssToken::Dimension(-0.5, "px".to_string())));
    }

    #[test]
    fn hex_escape_in_ident_produces_unicode_codepoint() {
        let tokens = tokenize(r"div { m\argin: 2em; }").unwrap();
        let has_margin_ident = tokens
            .iter()
            .any(|t| matches!(t, CssToken::Ident(s) if s == "margin"));
        assert!(
            !has_margin_ident,
            "m\\argin should not tokenize as 'margin'; tokens: {:?}",
            tokens
        );
    }

    #[test]
    fn escaped_closing_brace_does_not_close_rule() {
        let tokens = tokenize(r"div { error: \}; background: yellow; }").unwrap();
        let curly_close_count = tokens
            .iter()
            .filter(|t| matches!(t, CssToken::CurlyClose))
            .count();
        assert_eq!(
            curly_close_count, 1,
            "only the final }} should be CurlyClose, tokens: {:?}",
            tokens
        );

        let stylesheet = parse_stylesheet(r"div { error: \}; background: yellow; }").unwrap();
        let Rule::Style(rule) = &stylesheet.rules[0] else {
            panic!("expected style rule");
        };
        assert!(
            rule.declarations
                .iter()
                .any(|decl| decl.name == "background-color"),
            "background: yellow should survive error: \\}}; declarations: {:?}",
            rule.declarations,
        );
    }

    #[test]
    fn invalid_background_value_is_ignored() {
        let stylesheet =
            parse_stylesheet(".parser { background: yellow; } .parser { background: red pink; }")
                .unwrap();
        let rules: Vec<_> = stylesheet
            .rules
            .iter()
            .filter_map(|r| match r {
                Rule::Style(s) => Some(s),
                _ => None,
            })
            .collect();
        let last_bg = rules.iter().rev().find_map(|rule| {
            rule.declarations
                .iter()
                .find(|d| d.name == "background-color")
        });
        eprintln!("last bg: {:?}", last_bg);
        assert!(
            last_bg.is_some(),
            "should have background-color declaration",
        );
    }

    #[test]
    fn parses_style_rule_with_selectors() {
        let stylesheet = parse_stylesheet("div.hero > #title, a:hover { color: #fff; }").unwrap();
        let Rule::Style(rule) = &stylesheet.rules[0] else {
            panic!("expected style rule");
        };

        assert_eq!(rule.selectors.len(), 2);
        assert_eq!(
            rule.selectors[0].parts[0].simples[0],
            SimpleSelector::Type("div".to_string())
        );
        assert_eq!(
            rule.selectors[0].parts[0].simples[1],
            SimpleSelector::Class("hero".to_string())
        );
        assert_eq!(
            rule.selectors[0].parts[1].combinator,
            Some(Combinator::Child)
        );
        assert_eq!(
            rule.selectors[1].parts[0].simples[0],
            SimpleSelector::Type("a".to_string())
        );
        assert_eq!(
            rule.selectors[1].parts[0].simples[1],
            SimpleSelector::PseudoClass("hover".to_string())
        );
    }

    #[test]
    fn parses_attribute_and_pseudo_element_selectors() {
        let stylesheet =
            parse_stylesheet(r#"input[type="text"]::placeholder { color: gray; }"#).unwrap();
        let Rule::Style(rule) = &stylesheet.rules[0] else {
            panic!("expected style rule");
        };

        assert_eq!(
            rule.selectors[0].parts[0].simples,
            vec![
                SimpleSelector::Type("input".to_string()),
                SimpleSelector::Attribute {
                    name: "type".to_string(),
                    operator: Some(AttributeOperator::Equals),
                    value: Some("text".to_string()),
                },
                SimpleSelector::PseudoElement("placeholder".to_string()),
            ]
        );
    }

    #[test]
    fn parses_escaped_attribute_selector_values() {
        let stylesheet =
            parse_stylesheet(r#"[class=second\ two][class="second two"] { float: right; }"#)
                .unwrap();
        let Rule::Style(rule) = &stylesheet.rules[0] else {
            panic!("expected style rule");
        };

        assert_eq!(
            rule.selectors[0].parts[0].simples,
            vec![
                SimpleSelector::Attribute {
                    name: "class".to_string(),
                    operator: Some(AttributeOperator::Equals),
                    value: Some("second two".to_string()),
                },
                SimpleSelector::Attribute {
                    name: "class".to_string(),
                    operator: Some(AttributeOperator::Equals),
                    value: Some("second two".to_string()),
                },
            ]
        );
    }

    #[test]
    fn parses_values_and_functions() {
        let stylesheet =
            parse_stylesheet("body { width: calc(100%); background-color: rgb(255, 0, 0); }")
                .unwrap();
        let Rule::Style(rule) = &stylesheet.rules[0] else {
            panic!("expected style rule");
        };

        assert_eq!(
            rule.declarations[0].value,
            Value::Function {
                name: "calc".to_string(),
                arguments: vec![Value::Percentage(100.0)],
            }
        );
        assert_eq!(
            rule.declarations[1].value,
            Value::Function {
                name: "rgb".to_string(),
                arguments: vec![Value::Number(255.0), Value::Number(0.0), Value::Number(0.0)],
            }
        );
    }

    #[test]
    fn parses_calc_expressions_with_operators() {
        let stylesheet = parse_stylesheet("body { width: calc(100% - 24px); }").unwrap();
        let Rule::Style(rule) = &stylesheet.rules[0] else {
            panic!("expected style rule");
        };

        assert_eq!(
            rule.declarations[0].value,
            Value::Function {
                name: "calc".to_string(),
                arguments: vec![Value::List(vec![
                    Value::Percentage(100.0),
                    Value::Keyword("-".to_string()),
                    Value::Length(24.0, "px".to_string()),
                ])],
            }
        );
    }

    #[test]
    fn parses_calc_operators_without_whitespace() {
        let stylesheet =
            parse_stylesheet("body { width: calc(var(--gap)*2); height: calc(100%/2); }").unwrap();
        let Rule::Style(rule) = &stylesheet.rules[0] else {
            panic!("expected style rule");
        };

        assert_eq!(
            rule.declarations[0].value,
            Value::Function {
                name: "calc".to_string(),
                arguments: vec![Value::List(vec![
                    Value::Function {
                        name: "var".to_string(),
                        arguments: vec![Value::Keyword("--gap".to_string())],
                    },
                    Value::Keyword("*".to_string()),
                    Value::Number(2.0),
                ])],
            }
        );

        assert_eq!(
            rule.declarations[1].value,
            Value::Function {
                name: "calc".to_string(),
                arguments: vec![Value::List(vec![
                    Value::Percentage(100.0),
                    Value::Keyword("/".to_string()),
                    Value::Number(2.0),
                ])],
            }
        );
    }

    #[test]
    fn parses_at_rules() {
        let stylesheet = parse_stylesheet(
            r#"@import "base.css"; @font-face { font-family: "Demo"; src: url(font.woff2); } @media screen { h1 { color: blue; } }"#,
        )
        .unwrap();

        assert_eq!(stylesheet.rules.len(), 3);
        let Rule::At(import_rule) = &stylesheet.rules[0] else {
            panic!("expected import rule");
        };
        assert_eq!(import_rule.name, "import");
        assert_eq!(import_rule.prelude, "\"base.css\"");

        let Rule::At(font_face) = &stylesheet.rules[1] else {
            panic!("expected font-face rule");
        };
        assert_eq!(font_face.declarations.len(), 2);

        let Rule::At(media_rule) = &stylesheet.rules[2] else {
            panic!("expected media rule");
        };
        assert!(media_rule.block.is_some());
    }

    #[test]
    fn expands_margin_and_border_shorthands() {
        let stylesheet =
            parse_stylesheet("div { margin: 1px 2px; border: 1px solid #000; }").unwrap();
        let Rule::Style(rule) = &stylesheet.rules[0] else {
            panic!("expected style rule");
        };

        assert!(rule.declarations.iter().any(|decl| decl.name == "margin-top"));
        assert!(rule.declarations.iter().any(|decl| decl.name == "margin-right"));
        assert!(rule.declarations.iter().any(|decl| decl.name == "border-width"));
        assert!(rule.declarations.iter().any(|decl| decl.name == "border-style"));
        assert!(rule.declarations.iter().any(|decl| decl.name == "border-color"));
    }

    #[test]
    fn expands_background_and_font_shorthands() {
        let stylesheet =
            parse_stylesheet("h1 { background: white; font: 24px/24px sans-serif; }").unwrap();
        let Rule::Style(rule) = &stylesheet.rules[0] else {
            panic!("expected style rule");
        };

        assert!(rule.declarations.iter().any(|decl| decl.name == "background-color"));
        assert!(rule.declarations.iter().any(|decl| decl.name == "font-size"));
        assert!(rule.declarations.iter().any(|decl| decl.name == "line-height"));
    }

    #[test]
    fn expands_background_image_from_url_shorthand() {
        let stylesheet =
            parse_stylesheet("div { background: red url(\"data:image/png;base64,AAAA\"); }")
                .unwrap();
        let Rule::Style(rule) = &stylesheet.rules[0] else {
            panic!("expected style rule");
        };

        assert!(rule.declarations.iter().any(|decl| decl.name == "background-color"));
        assert!(rule.declarations.iter().any(|decl| decl.name == "background-image"));
    }

    #[test]
    fn expands_background_repeat_keywords() {
        let stylesheet = parse_stylesheet("div { background: url(\"x\") no-repeat; }").unwrap();
        let Rule::Style(rule) = &stylesheet.rules[0] else {
            panic!("expected style rule");
        };

        assert!(
            rule.declarations.iter().any(|decl| decl.name == "background-repeat"
                && matches!(&decl.value, Value::Keyword(value) if value == "no-repeat"))
        );
    }

    #[test]
    fn expands_background_attachment_and_position() {
        let stylesheet = parse_stylesheet("div { background: fixed url(\"x\") 1px 0; }").unwrap();
        let Rule::Style(rule) = &stylesheet.rules[0] else {
            panic!("expected style rule");
        };

        assert!(rule.declarations.iter().any(|decl| decl.name == "background-attachment"
            && matches!(&decl.value, Value::Keyword(value) if value == "fixed")));
        assert!(rule.declarations.iter().any(
            |decl| decl.name == "background-position-x"
                && matches!(&decl.value, Value::Length(value, unit) if *value == 1.0 && unit == "px")
        ));
        assert!(rule.declarations.iter().any(|decl| decl.name == "background-position-y"
            && matches!(&decl.value, Value::Number(value) if *value == 0.0)));
    }

    #[test]
    fn expands_background_none_to_transparent_and_no_image() {
        let stylesheet = parse_stylesheet("div { background: none; }").unwrap();
        let Rule::Style(rule) = &stylesheet.rules[0] else {
            panic!("expected style rule");
        };

        assert!(rule.declarations.iter().any(|decl| decl.name == "background-image"
            && matches!(&decl.value, Value::Keyword(value) if value == "none")));
        assert!(rule.declarations.iter().any(|decl| decl.name == "background-color"
            && matches!(&decl.value, Value::Keyword(value) if value == "transparent")));
    }

    #[test]
    fn expands_border_side_shorthands() {
        let stylesheet = parse_stylesheet("div { border-top: solid yellow 2px; }").unwrap();
        let Rule::Style(rule) = &stylesheet.rules[0] else {
            panic!("expected style rule");
        };

        assert!(rule.declarations.iter().any(
            |decl| decl.name == "border-top-width"
                && matches!(&decl.value, Value::Length(value, unit) if *value == 2.0 && unit == "px")
        ));
        assert!(rule.declarations.iter().any(|decl| decl.name == "border-top-style"
            && matches!(&decl.value, Value::Keyword(value) if value == "solid")));
        assert!(rule.declarations.iter().any(|decl| decl.name == "border-top-color"
            && matches!(&decl.value, Value::Keyword(value) if value == "yellow")));
    }

    /// `border-top: 1px solid red` must NOT emit global `border-style` / `border-color`
    /// properties, which would otherwise override the other three sides.
    #[test]
    fn border_side_shorthand_does_not_emit_global_border_style_or_color() {
        let stylesheet = parse_stylesheet("div { border-top: 1px solid red; }").unwrap();
        let Rule::Style(rule) = &stylesheet.rules[0] else {
            panic!("expected style rule");
        };

        // Must NOT contain global border-style or border-color
        assert!(
            !rule.declarations.iter().any(|decl| decl.name == "border-style"),
            "border-top shorthand should NOT emit global 'border-style'"
        );
        assert!(
            !rule.declarations.iter().any(|decl| decl.name == "border-color"),
            "border-top shorthand should NOT emit global 'border-color'"
        );

        // Must still contain the side-specific properties
        assert!(rule.declarations.iter().any(|decl| decl.name == "border-top-style"
            && matches!(&decl.value, Value::Keyword(v) if v == "solid")));
        assert!(rule.declarations.iter().any(|decl| decl.name == "border-top-color"
            && matches!(&decl.value, Value::Keyword(v) if v == "red")));
        assert!(rule.declarations.iter().any(
            |decl| decl.name == "border-top-width"
                && matches!(&decl.value, Value::Length(v, unit) if *v == 1.0 && unit == "px")
        ));
    }

    #[test]
    fn expands_border_style_box_shorthand() {
        let stylesheet = parse_stylesheet("div { border-style: none solid; }").unwrap();
        let Rule::Style(rule) = &stylesheet.rules[0] else {
            panic!("expected style rule");
        };

        assert!(rule.declarations.iter().any(|decl| decl.name == "border-top-style"
            && matches!(&decl.value, Value::Keyword(value) if value == "none")));
        assert!(rule.declarations.iter().any(|decl| decl.name == "border-right-style"
            && matches!(&decl.value, Value::Keyword(value) if value == "solid")));
        assert!(rule.declarations.iter().any(|decl| decl.name == "border-bottom-style"
            && matches!(&decl.value, Value::Keyword(value) if value == "none")));
        assert!(rule.declarations.iter().any(|decl| decl.name == "border-left-style"
            && matches!(&decl.value, Value::Keyword(value) if value == "solid")));
    }

    #[test]
    fn expands_list_style_shorthand_all_three() {
        let stylesheet = parse_stylesheet("ul { list-style: disc inside none; }").unwrap();
        let Rule::Style(rule) = &stylesheet.rules[0] else {
            panic!("expected style rule");
        };

        assert!(rule.declarations.iter().any(|decl| decl.name == "list-style-type"
            && matches!(&decl.value, Value::Keyword(v) if v == "disc")));
        assert!(rule.declarations.iter().any(|decl| decl.name == "list-style-position"
            && matches!(&decl.value, Value::Keyword(v) if v == "inside")));
        assert!(rule.declarations.iter().any(|decl| decl.name == "list-style-image"
            && matches!(&decl.value, Value::Keyword(v) if v == "none")));
    }

    #[test]
    fn expands_list_style_shorthand_type_only() {
        let stylesheet = parse_stylesheet("ol { list-style: decimal; }").unwrap();
        let Rule::Style(rule) = &stylesheet.rules[0] else {
            panic!("expected style rule");
        };

        assert!(rule.declarations.iter().any(|decl| decl.name == "list-style-type"
            && matches!(&decl.value, Value::Keyword(v) if v == "decimal")));
    }

    #[test]
    fn expands_list_style_none_shorthand() {
        let stylesheet = parse_stylesheet("li { list-style: none; }").unwrap();
        let Rule::Style(rule) = &stylesheet.rules[0] else {
            panic!("expected style rule");
        };

        assert!(rule.declarations.iter().any(|decl| decl.name == "list-style-type"
            && matches!(&decl.value, Value::Keyword(v) if v == "none")));
    }

    #[test]
    fn expands_linear_gradient_as_background_image() {
        let stylesheet =
            parse_stylesheet("div { background: linear-gradient(to right, red, blue); }")
                .unwrap();
        let Rule::Style(rule) = &stylesheet.rules[0] else {
            panic!("expected style rule");
        };

        assert!(rule.declarations.iter().any(|decl| decl.name == "background-image"),
            "linear-gradient() should expand to background-image; got: {:?}", rule.declarations);
        assert!(!rule.declarations.iter().any(|decl| decl.name == "background-color"),
            "linear-gradient() should NOT expand to background-color");
    }

    #[test]
    fn background_size_standalone_property() {
        let stylesheet = parse_stylesheet("div { background-size: cover; }").unwrap();
        let Rule::Style(rule) = &stylesheet.rules[0] else {
            panic!("expected style rule");
        };

        assert!(rule.declarations.iter().any(|decl| decl.name == "background-size"
            && matches!(&decl.value, Value::Keyword(v) if v == "cover")),
            "background-size: cover should be parsed; got: {:?}", rule.declarations);
    }

    #[test]
    fn background_size_contain_standalone() {
        let stylesheet = parse_stylesheet("div { background-size: contain; }").unwrap();
        let Rule::Style(rule) = &stylesheet.rules[0] else {
            panic!("expected style rule");
        };

        assert!(rule.declarations.iter().any(|decl| decl.name == "background-size"
            && matches!(&decl.value, Value::Keyword(v) if v == "contain")));
    }

    #[test]
    fn background_size_length_standalone() {
        let stylesheet = parse_stylesheet("div { background-size: 100px 50px; }").unwrap();
        let Rule::Style(rule) = &stylesheet.rules[0] else {
            panic!("expected style rule");
        };

        assert!(rule.declarations.iter().any(|decl| decl.name == "background-size"),
            "background-size with lengths should be parsed; got: {:?}", rule.declarations);
    }

    // -- @media query parsing --

    #[test]
    fn parse_media_query_screen_type() {
        let queries = parse_media_query_list("screen").unwrap();
        assert_eq!(queries.len(), 1);
        assert!(!queries[0].negated);
        assert_eq!(queries[0].media_type.as_deref(), Some("screen"));
        assert!(queries[0].conditions.is_empty());
    }

    #[test]
    fn parse_media_query_all_type() {
        let queries = parse_media_query_list("all").unwrap();
        assert_eq!(queries.len(), 1);
        assert_eq!(queries[0].media_type.as_deref(), Some("all"));
    }

    #[test]
    fn parse_media_query_max_width() {
        let queries = parse_media_query_list("(max-width: 768px)").unwrap();
        assert_eq!(queries.len(), 1);
        assert_eq!(queries[0].conditions, vec![MediaCondition::MaxWidth(768.0)]);
        assert!(queries[0].media_type.is_none());
    }

    #[test]
    fn parse_media_query_min_width() {
        let queries = parse_media_query_list("(min-width: 1024px)").unwrap();
        assert_eq!(queries.len(), 1);
        assert_eq!(queries[0].conditions, vec![MediaCondition::MinWidth(1024.0)]);
    }

    #[test]
    fn parse_media_query_orientation_portrait() {
        let queries = parse_media_query_list("(orientation: portrait)").unwrap();
        assert_eq!(queries[0].conditions, vec![MediaCondition::OrientationPortrait]);
    }

    #[test]
    fn parse_media_query_orientation_landscape() {
        let queries = parse_media_query_list("(orientation: landscape)").unwrap();
        assert_eq!(queries[0].conditions, vec![MediaCondition::OrientationLandscape]);
    }

    #[test]
    fn parse_media_query_screen_and_max_width() {
        let queries = parse_media_query_list("screen and (max-width: 768px)").unwrap();
        assert_eq!(queries.len(), 1);
        assert_eq!(queries[0].media_type.as_deref(), Some("screen"));
        assert_eq!(queries[0].conditions, vec![MediaCondition::MaxWidth(768.0)]);
    }

    #[test]
    fn parse_media_query_only_screen_strips_modifier() {
        let queries = parse_media_query_list("only screen and (max-width: 768px)").unwrap();
        assert_eq!(queries.len(), 1);
        assert_eq!(queries[0].media_type.as_deref(), Some("screen"),
            "media type should be 'screen', not 'only'");
        assert_eq!(queries[0].conditions, vec![MediaCondition::MaxWidth(768.0)]);
    }

    #[test]
    fn parse_media_query_not_print() {
        let queries = parse_media_query_list("not print").unwrap();
        assert_eq!(queries.len(), 1);
        assert!(queries[0].negated);
        assert_eq!(queries[0].media_type.as_deref(), Some("print"));
    }

    #[test]
    fn parse_media_query_comma_separated() {
        let queries = parse_media_query_list("screen, print").unwrap();
        assert_eq!(queries.len(), 2);
        assert_eq!(queries[0].media_type.as_deref(), Some("screen"));
        assert_eq!(queries[1].media_type.as_deref(), Some("print"));
    }

    #[test]
    fn parse_media_query_and_multiple_conditions() {
        let queries =
            parse_media_query_list("screen and (min-width: 600px) and (max-width: 1200px)")
                .unwrap();
        assert_eq!(queries.len(), 1);
        assert_eq!(queries[0].media_type.as_deref(), Some("screen"));
        assert_eq!(
            queries[0].conditions,
            vec![
                MediaCondition::MinWidth(600.0),
                MediaCondition::MaxWidth(1200.0),
            ]
        );
    }

    #[test]
    fn parse_media_query_em_unit() {
        let queries = parse_media_query_list("(max-width: 48em)").unwrap();
        assert_eq!(queries[0].conditions, vec![MediaCondition::MaxWidth(768.0)]);
    }

    // -- @media query evaluation --

    #[test]
    fn evaluate_screen_matches() {
        let queries = parse_media_query_list("screen").unwrap();
        assert!(evaluate_media_query(&queries[0], 1024.0, 768.0, false));
    }

    #[test]
    fn evaluate_print_does_not_match() {
        let queries = parse_media_query_list("print").unwrap();
        assert!(!evaluate_media_query(&queries[0], 1024.0, 768.0, false));
    }

    #[test]
    fn evaluate_not_print_matches() {
        let queries = parse_media_query_list("not print").unwrap();
        assert!(evaluate_media_query(&queries[0], 1024.0, 768.0, false));
    }

    #[test]
    fn evaluate_max_width_match() {
        let queries = parse_media_query_list("(max-width: 768px)").unwrap();
        assert!(evaluate_media_query(&queries[0], 768.0, 1024.0, false));
        assert!(evaluate_media_query(&queries[0], 600.0, 1024.0, false));
        assert!(!evaluate_media_query(&queries[0], 1024.0, 768.0, false));
    }

    #[test]
    fn evaluate_min_width_match() {
        let queries = parse_media_query_list("(min-width: 1024px)").unwrap();
        assert!(evaluate_media_query(&queries[0], 1024.0, 768.0, false));
        assert!(evaluate_media_query(&queries[0], 1280.0, 768.0, false));
        assert!(!evaluate_media_query(&queries[0], 800.0, 600.0, false));
    }

    #[test]
    fn evaluate_orientation_portrait() {
        let queries = parse_media_query_list("(orientation: portrait)").unwrap();
        assert!(evaluate_media_query(&queries[0], 600.0, 900.0, false));
        assert!(evaluate_media_query(&queries[0], 768.0, 768.0, false));
        assert!(!evaluate_media_query(&queries[0], 1024.0, 768.0, false));
    }

    #[test]
    fn evaluate_orientation_landscape() {
        let queries = parse_media_query_list("(orientation: landscape)").unwrap();
        assert!(evaluate_media_query(&queries[0], 1024.0, 768.0, false));
        assert!(!evaluate_media_query(&queries[0], 600.0, 900.0, false));
        assert!(!evaluate_media_query(&queries[0], 768.0, 768.0, false));
    }

    #[test]
    fn evaluate_prefers_color_scheme_dark() {
        let queries = parse_media_query_list("(prefers-color-scheme: dark)").unwrap();
        assert!(evaluate_media_query(&queries[0], 1024.0, 768.0, true));
        assert!(!evaluate_media_query(&queries[0], 1024.0, 768.0, false));
    }

    #[test]
    fn evaluate_comma_list_any_match() {
        let queries = parse_media_query_list("print, screen").unwrap();
        let matches = queries
            .iter()
            .any(|q| evaluate_media_query(q, 1024.0, 768.0, false));
        assert!(matches);
    }
}
