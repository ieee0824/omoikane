//! CSS parsing primitives.
//!
//! The CSS layer exposes a tokenizer and a stylesheet parser that later style
//! resolution phases can reuse directly.

use std::fmt;

mod matcher;
mod media;
mod parser;
mod shorthand;
mod scope;
mod supports;
pub(crate) mod style;
mod tokenizer;

pub use matcher::{
    PseudoElement, Specificity, matches_selector, matches_selector_with_pseudo,
    selector_pseudo_element, specificity,
};
pub use media::{evaluate_media_query, parse_media_query_list};
pub use parser::{
    extract_font_face_rules, parse_selector_list, parse_style_attribute, parse_stylesheet,
};
pub use style::{ComputedStyle, ComputedValue, Origin, StyleResolver, StylesheetInput};
pub(crate) use style::supports_declaration;
pub(crate) use scope::{ScopePrelude, parse_scope_prelude};
pub(crate) use supports::supports_condition_matches;
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

/// A selector evaluated relative to an anchor element, as used by `:has()`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelativeSelector {
    pub leading_combinator: Combinator,
    pub selector: Selector,
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
    /// `:is(<selector-list>)` -- matches any selector in the argument list.
    Is(Vec<Selector>),
    /// `:where(<selector-list>)` -- like `:is()`, with zero specificity.
    Where(Vec<Selector>),
    /// `:not(<selector-list>)` -- negates every selector in the argument list.
    Not(Vec<Selector>),
    /// `:has(<relative-selector-list>)` -- matches from this anchor.
    Has(Vec<RelativeSelector>),
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
    FontFace(FontFaceRule),
}

/// A parsed `@font-face` rule.
///
/// Holds the descriptors needed to register and select a web font.
#[derive(Debug, Clone, PartialEq)]
pub struct FontFaceRule {
    /// The font family name declared in `font-family`.
    pub font_family: String,
    /// The URL of the font file from `src: url(...)`.
    pub src_url: String,
    /// Optional format hint, e.g. `"woff2"`, `"truetype"`.
    pub format: Option<String>,
    /// Optional font-weight descriptor (e.g. `"bold"`, `"400"`).
    pub font_weight: Option<String>,
    /// Optional font-style descriptor (e.g. `"italic"`, `"normal"`).
    pub font_style: Option<String>,
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
    /// Range syntax `(width < <length>)`.
    MaxWidthExclusive(f32),
    /// Range syntax `(width > <length>)`.
    MinWidthExclusive(f32),
    /// `(max-height: <length>)` -- viewport height <= value.
    MaxHeight(f32),
    /// `(min-height: <length>)` -- viewport height >= value.
    MinHeight(f32),
    /// Range syntax `(height < <length>)`.
    MaxHeightExclusive(f32),
    /// Range syntax `(height > <length>)`.
    MinHeightExclusive(f32),
    /// `(orientation: portrait)` -- height >= width.
    OrientationPortrait,
    /// `(orientation: landscape)` -- width > height.
    OrientationLandscape,
    /// `(prefers-color-scheme: dark)`.
    PrefersColorSchemeDark,
    /// `(prefers-color-scheme: light)`.
    PrefersColorSchemeLight,
    /// `(color)` or `(min/max-color: <integer>)`, in bits per color component.
    Color { minimum: Option<u32>, maximum: Option<u32> },
    /// `(monochrome)` or `(min/max-monochrome: <integer>)`, in bits per pixel.
    Monochrome { minimum: Option<u32>, maximum: Option<u32> },
    /// An unrecognised condition -- never matches.
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
            r#"@import "base.css"; @font-face { font-family: "Demo"; src: url(font.woff2); } @media screen { h1 { color: blue; } } @supports (display: grid) { main { display: grid; } }"#,
        )
        .unwrap();

        assert_eq!(stylesheet.rules.len(), 4);
        let Rule::At(import_rule) = &stylesheet.rules[0] else {
            panic!("expected import rule");
        };
        assert_eq!(import_rule.name, "import");
        assert_eq!(import_rule.prelude, "\"base.css\"");

        let Rule::FontFace(font_face) = &stylesheet.rules[1] else {
            panic!("expected font-face rule, got {:?}", stylesheet.rules[1]);
        };
        assert_eq!(font_face.font_family, "Demo");
        assert_eq!(font_face.src_url, "font.woff2");

        let Rule::At(media_rule) = &stylesheet.rules[2] else {
            panic!("expected media rule");
        };
        assert!(media_rule.block.is_some());

        let Rule::At(supports_rule) = &stylesheet.rules[3] else {
            panic!("expected supports rule");
        };
        assert_eq!(supports_rule.name, "supports");
        assert_eq!(supports_rule.prelude, "(display: grid)");
        assert_eq!(supports_rule.block.as_ref().map(Vec::len), Some(1));
    }

    #[test]
    fn parses_general_enclosed_braces_in_supports_prelude() {
        let stylesheet = parse_stylesheet("@supports ({future}) { main { display: block; } }")
            .expect("parse supports general-enclosed condition");
        let Rule::At(rule) = &stylesheet.rules[0] else {
            panic!("expected supports rule");
        };
        assert_eq!(rule.prelude, "({future})");
        assert_eq!(rule.block.as_ref().map(Vec::len), Some(1));
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
    fn expands_unitless_zero_border_width() {
        let stylesheet = parse_stylesheet("button { border: 0; }").unwrap();
        let Rule::Style(rule) = &stylesheet.rules[0] else {
            panic!("expected style rule");
        };

        assert!(rule.declarations.iter().any(|decl| {
            decl.name == "border-top-width"
                && matches!(decl.value, Value::Number(value) if value == 0.0)
        }));
    }

    #[test]
    fn preserves_multi_token_important_shorthands() {
        let stylesheet = parse_stylesheet(
            "div { margin: 1px 2px !important; border: 1px solid red !important; }",
        )
        .unwrap();
        let Rule::Style(rule) = &stylesheet.rules[0] else {
            panic!("expected style rule");
        };

        for (name, expected) in [
            ("margin-top", 1.0),
            ("margin-right", 2.0),
            ("margin-bottom", 1.0),
            ("margin-left", 2.0),
        ] {
            assert!(rule.declarations.iter().any(|declaration| {
                declaration.name == name
                    && declaration.important
                    && matches!(&declaration.value, Value::Length(value, unit) if *value == expected && unit == "px")
            }));
        }

        for (name, expected) in [
            ("border-width", Value::Length(1.0, "px".to_string())),
            ("border-style", Value::Keyword("solid".to_string())),
            ("border-color", Value::Keyword("red".to_string())),
        ] {
            assert!(rule.declarations.iter().any(|declaration| {
                declaration.name == name
                    && declaration.important
                    && declaration.value == expected
            }));
        }
    }

    #[test]
    fn recognizes_case_and_whitespace_variants_of_important() {
        for css in ["div { color: red !IMPORTANT; }", "div { color: red ! important; }"] {
            let stylesheet = parse_stylesheet(css).unwrap();
            let Rule::Style(rule) = &stylesheet.rules[0] else {
                panic!("expected style rule");
            };
            assert_eq!(rule.declarations.len(), 1);
            assert_eq!(rule.declarations[0].value, Value::Keyword("red".to_string()));
            assert!(rule.declarations[0].important);
        }
    }

    #[test]
    fn preserves_semicolons_inside_unquoted_urls() {
        let stylesheet =
            parse_stylesheet("div { background: url(data:image/png;base64,AAA) }").unwrap();
        let Rule::Style(rule) = &stylesheet.rules[0] else {
            panic!("expected style rule");
        };
        assert!(rule.declarations.iter().any(|declaration| {
            declaration.name == "background-image"
                && declaration.value
                    == Value::Keyword("url(data:image/png;base64,AAA)".to_string())
        }));
    }

    #[test]
    fn parses_style_attributes_as_forgiving_declaration_lists() {
        let declarations = parse_style_attribute("width: 100px; color: red");
        assert_eq!(declarations.len(), 2);
        assert_eq!(declarations[0].name, "width");
        assert_eq!(declarations[0].value, Value::Length(100.0, "px".to_string()));
        assert_eq!(declarations[1].name, "color");
        assert_eq!(declarations[1].value, Value::Keyword("red".to_string()));

        let declarations = parse_style_attribute("width: 100px");
        assert_eq!(declarations.len(), 1);
        assert_eq!(declarations[0].name, "width");

        let declarations = parse_style_attribute(";; color: red ;");
        assert_eq!(declarations.len(), 1);
        assert_eq!(declarations[0].name, "color");

        let declarations = parse_style_attribute("color red; width: 10px");
        assert_eq!(declarations.len(), 1);
        assert_eq!(declarations[0].name, "width");
        assert_eq!(declarations[0].value, Value::Length(10.0, "px".to_string()));

        assert!(parse_style_attribute("").is_empty());
        assert!(parse_style_attribute("   ").is_empty());
        assert!(parse_style_attribute("content: 'abc").is_empty());
    }

    #[test]
    fn style_attribute_recovers_declarations_before_tokenization_error() {
        let declarations = parse_style_attribute("color: red; content: 'abc");
        assert_eq!(declarations.len(), 1);
        assert_eq!(declarations[0].name, "color");
        assert_eq!(declarations[0].value, Value::Keyword("red".to_string()));

        assert!(parse_style_attribute("content: 'abc; color: red").is_empty());
        assert!(parse_style_attribute("content: 'abc").is_empty());
    }

    #[test]
    fn important_marker_must_be_at_top_level() {
        let declarations = parse_style_attribute("width: calc(1px + 2px) !important");
        assert_eq!(declarations.len(), 1);
        assert_eq!(declarations[0].name, "width");
        assert!(declarations[0].important);

        assert!(parse_style_attribute("width: foo(bar !important").is_empty());
        assert!(
            parse_style_attribute("width: foo(bar !important; color: red").is_empty()
        );
    }

    #[test]
    fn style_attribute_semicolons_inside_brackets_do_not_split_declarations() {
        let declarations =
            parse_style_attribute("grid-template-columns: [a;b] 1fr; color: red");
        assert_eq!(declarations.len(), 2);
        assert_eq!(declarations[0].name, "grid-template-columns");
        assert_eq!(declarations[1].name, "color");
        assert_eq!(declarations[1].value, Value::Keyword("red".to_string()));
    }

    #[test]
    fn style_attributes_preserve_urls_and_important_shorthands() {
        let declarations = parse_style_attribute("background: url(data:image/png;base64,AAA)");
        assert!(declarations.iter().any(|declaration| {
            declaration.name == "background-image"
                && declaration.value
                    == Value::Keyword("url(data:image/png;base64,AAA)".to_string())
        }));

        let declarations = parse_style_attribute("color: blue !important");
        assert_eq!(declarations.len(), 1);
        assert_eq!(declarations[0].value, Value::Keyword("blue".to_string()));
        assert!(declarations[0].important);

        let declarations = parse_style_attribute("margin: 1px 2px !important");
        assert_eq!(declarations.len(), 4);
        for (declaration, expected) in declarations.iter().zip([1.0, 2.0, 1.0, 2.0]) {
            assert!(declaration.important);
            assert_eq!(declaration.value, Value::Length(expected, "px".to_string()));
        }
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
    fn expands_background_position_keyword() {
        let stylesheet =
            parse_stylesheet("#hero { background: url(hero.png) no-repeat right 0; }").unwrap();
        let Rule::Style(rule) = &stylesheet.rules[0] else {
            panic!("expected style rule");
        };

        assert!(rule.declarations.iter().any(|decl| decl.name == "background-image"));
        assert!(rule.declarations.iter().any(|decl| decl.name == "background-position-x"
            && matches!(&decl.value, Value::Keyword(value) if value == "right")));
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
    fn parse_media_query_level_four_width_ranges() {
        let queries = parse_media_query_list("(width >= 851px), (48rem <= width)").unwrap();
        assert_eq!(queries[0].conditions, vec![MediaCondition::MinWidth(851.0)]);
        assert_eq!(queries[1].conditions, vec![MediaCondition::MinWidth(768.0)]);

        let queries = parse_media_query_list("(width <= 1000px), (720px >= height)").unwrap();
        assert_eq!(queries[0].conditions, vec![MediaCondition::MaxWidth(1000.0)]);
        assert_eq!(queries[1].conditions, vec![MediaCondition::MaxHeight(720.0)]);
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

    #[test]
    fn parse_media_query_color_and_monochrome_features() {
        let queries = parse_media_query_list(
            "(color) and (min-color: 1) and (max-monochrome: 0)",
        ).unwrap();
        assert_eq!(queries[0].conditions, vec![
            MediaCondition::Color { minimum: None, maximum: None },
            MediaCondition::Color { minimum: Some(1), maximum: None },
            MediaCondition::Monochrome { minimum: None, maximum: Some(0) },
        ]);
    }

    #[test]
    fn parse_media_query_unknown_feature_is_preserved_as_false_condition() {
        let queries = parse_media_query_list("only all and (future-feature: 1)").unwrap();
        assert_eq!(queries[0].conditions, vec![MediaCondition::Unknown]);
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
    fn evaluate_negated_level_four_width_range() {
        let query = &parse_media_query_list("not all and (width >= 851px)").unwrap()[0];
        assert!(!evaluate_media_query(query, 1280.0, 720.0, false));
        assert!(evaluate_media_query(query, 800.0, 720.0, false));
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

    #[test]
    fn evaluate_color_and_monochrome_features() {
        let matches = |query: &str| {
            let queries = parse_media_query_list(query).unwrap();
            queries.iter().any(|q| evaluate_media_query(q, 0.0, 0.0, false))
        };
        assert!(matches("(color)"));
        assert!(matches("(min-color: 1)"));
        assert!(!matches("(max-color: 0)"));
        assert!(!matches("(monochrome)"));
        assert!(matches("(max-monochrome: 0)"));
        assert!(!matches("(min-monochrome: 1)"));
    }

    #[test]
    fn evaluate_not_negates_the_whole_query() {
        let queries = parse_media_query_list("not all and (min-color: 1)").unwrap();
        assert!(!evaluate_media_query(&queries[0], 0.0, 0.0, false));
        let queries = parse_media_query_list("not all and (min-monochrome: 1)").unwrap();
        assert!(evaluate_media_query(&queries[0], 0.0, 0.0, false));
    }

    // -- @font-face parsing --

    #[test]
    fn parses_font_face_with_format_hint() {
        let stylesheet = parse_stylesheet(
            r#"@font-face { font-family: "MyFont"; src: url("https://example.com/font.woff2") format("woff2"); }"#,
        )
        .unwrap();

        assert_eq!(stylesheet.rules.len(), 1);
        let Rule::FontFace(ff) = &stylesheet.rules[0] else {
            panic!("expected FontFace rule, got {:?}", stylesheet.rules[0]);
        };
        assert_eq!(ff.font_family, "MyFont");
        assert_eq!(ff.src_url, "https://example.com/font.woff2");
        assert_eq!(ff.format.as_deref(), Some("woff2"));
    }

    #[test]
    fn font_face_uses_last_source_as_compatible_fallback() {
        let stylesheet = parse_stylesheet(
            r#"@font-face { font-family: Chirp; src: url(chirp.woff2) format("woff2"), url(chirp.woff) format("woff"); }"#,
        )
        .unwrap();

        let Rule::FontFace(ff) = &stylesheet.rules[0] else {
            panic!("expected FontFace rule");
        };
        assert_eq!(ff.src_url, "chirp.woff");
        assert_eq!(ff.format.as_deref(), Some("woff"));
    }

    #[test]
    fn parses_font_face_with_weight_and_style() {
        let stylesheet = parse_stylesheet(
            r#"@font-face { font-family: "MyFont"; src: url(font.ttf); font-weight: bold; font-style: italic; }"#,
        )
        .unwrap();

        let Rule::FontFace(ff) = &stylesheet.rules[0] else {
            panic!("expected FontFace rule");
        };
        assert_eq!(ff.font_family, "MyFont");
        assert_eq!(ff.src_url, "font.ttf");
        assert_eq!(ff.font_weight.as_deref(), Some("bold"));
        assert_eq!(ff.font_style.as_deref(), Some("italic"));
    }

    #[test]
    fn parses_font_face_unquoted_family() {
        let stylesheet = parse_stylesheet(
            r#"@font-face { font-family: CustomFont; src: url(custom.otf); }"#,
        )
        .unwrap();

        let Rule::FontFace(ff) = &stylesheet.rules[0] else {
            panic!("expected FontFace rule");
        };
        assert_eq!(ff.font_family, "CustomFont");
    }

    #[test]
    fn extract_font_face_rules_collects_all() {
        let stylesheet = parse_stylesheet(
            r#"
            body { color: black; }
            @font-face { font-family: "A"; src: url(a.ttf); }
            h1 { font-size: 24px; }
            @font-face { font-family: "B"; src: url(b.woff) format("woff"); }
            "#,
        )
        .unwrap();

        let ff_rules = extract_font_face_rules(&stylesheet);
        assert_eq!(ff_rules.len(), 2);
        assert_eq!(ff_rules[0].font_family, "A");
        assert_eq!(ff_rules[1].font_family, "B");
        assert_eq!(ff_rules[1].format.as_deref(), Some("woff"));
    }

    #[test]
    fn font_face_without_src_falls_back_to_at_rule() {
        let stylesheet = parse_stylesheet(
            r#"@font-face { font-family: "NoSrc"; }"#,
        )
        .unwrap();

        // Without src, it should fall back to a generic AtRule
        assert!(matches!(&stylesheet.rules[0], Rule::At(_)));
    }

    // ── Attribute selector parsing ──────────────────────────────────────

    #[test]
    fn parses_attribute_presence_selector() {
        let stylesheet = parse_stylesheet("[disabled] { opacity: 0.5; }").unwrap();
        let Rule::Style(rule) = &stylesheet.rules[0] else {
            panic!("expected style rule");
        };
        assert_eq!(
            rule.selectors[0].parts[0].simples,
            vec![SimpleSelector::Attribute {
                name: "disabled".to_string(),
                operator: None,
                value: None,
            }]
        );
    }

    #[test]
    fn parses_attribute_equals_selector() {
        let stylesheet = parse_stylesheet(r#"[type="text"] { color: black; }"#).unwrap();
        let Rule::Style(rule) = &stylesheet.rules[0] else {
            panic!("expected style rule");
        };
        assert_eq!(
            rule.selectors[0].parts[0].simples[0],
            SimpleSelector::Attribute {
                name: "type".to_string(),
                operator: Some(AttributeOperator::Equals),
                value: Some("text".to_string()),
            }
        );
    }

    #[test]
    fn parses_attribute_includes_selector() {
        let stylesheet = parse_stylesheet(r#"[class~="active"] { color: red; }"#).unwrap();
        let Rule::Style(rule) = &stylesheet.rules[0] else {
            panic!("expected style rule");
        };
        assert_eq!(
            rule.selectors[0].parts[0].simples[0],
            SimpleSelector::Attribute {
                name: "class".to_string(),
                operator: Some(AttributeOperator::Includes),
                value: Some("active".to_string()),
            }
        );
    }

    #[test]
    fn parses_attribute_starts_with_selector() {
        let stylesheet = parse_stylesheet(r#"[href^="https"] { color: green; }"#).unwrap();
        let Rule::Style(rule) = &stylesheet.rules[0] else {
            panic!("expected style rule");
        };
        assert_eq!(
            rule.selectors[0].parts[0].simples[0],
            SimpleSelector::Attribute {
                name: "href".to_string(),
                operator: Some(AttributeOperator::StartsWith),
                value: Some("https".to_string()),
            }
        );
    }

    #[test]
    fn parses_attribute_ends_with_selector() {
        let stylesheet = parse_stylesheet(r#"[src$=".png"] { border: none; }"#).unwrap();
        let Rule::Style(rule) = &stylesheet.rules[0] else {
            panic!("expected style rule");
        };
        assert_eq!(
            rule.selectors[0].parts[0].simples[0],
            SimpleSelector::Attribute {
                name: "src".to_string(),
                operator: Some(AttributeOperator::EndsWith),
                value: Some(".png".to_string()),
            }
        );
    }

    #[test]
    fn parses_attribute_contains_selector() {
        let stylesheet = parse_stylesheet(r#"[data-value*="mid"] { display: block; }"#).unwrap();
        let Rule::Style(rule) = &stylesheet.rules[0] else {
            panic!("expected style rule");
        };
        assert_eq!(
            rule.selectors[0].parts[0].simples[0],
            SimpleSelector::Attribute {
                name: "data-value".to_string(),
                operator: Some(AttributeOperator::Contains),
                value: Some("mid".to_string()),
            }
        );
    }

    #[test]
    fn parses_attribute_dash_match_selector() {
        let stylesheet = parse_stylesheet(r#"[lang|="en"] { font-size: 14px; }"#).unwrap();
        let Rule::Style(rule) = &stylesheet.rules[0] else {
            panic!("expected style rule");
        };
        assert_eq!(
            rule.selectors[0].parts[0].simples[0],
            SimpleSelector::Attribute {
                name: "lang".to_string(),
                operator: Some(AttributeOperator::DashMatch),
                value: Some("en".to_string()),
            }
        );
    }

    #[test]
    fn parses_multiple_attribute_selectors_on_same_element() {
        let stylesheet =
            parse_stylesheet(r#"input[type="text"][required] { border: 1px solid red; }"#)
                .unwrap();
        let Rule::Style(rule) = &stylesheet.rules[0] else {
            panic!("expected style rule");
        };
        assert_eq!(rule.selectors[0].parts[0].simples.len(), 3);
        assert_eq!(
            rule.selectors[0].parts[0].simples[0],
            SimpleSelector::Type("input".to_string())
        );
        assert_eq!(
            rule.selectors[0].parts[0].simples[1],
            SimpleSelector::Attribute {
                name: "type".to_string(),
                operator: Some(AttributeOperator::Equals),
                value: Some("text".to_string()),
            }
        );
        assert_eq!(
            rule.selectors[0].parts[0].simples[2],
            SimpleSelector::Attribute {
                name: "required".to_string(),
                operator: None,
                value: None,
            }
        );
    }

    // ── Pseudo selector parsing ─────────────────────────────────────────

    #[test]
    fn parses_pseudo_class_without_arguments() {
        let stylesheet = parse_stylesheet("a:hover { color: blue; }").unwrap();
        let Rule::Style(rule) = &stylesheet.rules[0] else {
            panic!("expected style rule");
        };
        assert_eq!(
            rule.selectors[0].parts[0].simples[1],
            SimpleSelector::PseudoClass("hover".to_string())
        );
    }

    #[test]
    fn parses_pseudo_class_with_arguments() {
        let stylesheet = parse_stylesheet("li:nth-child(2n+1) { color: red; }").unwrap();
        let Rule::Style(rule) = &stylesheet.rules[0] else {
            panic!("expected style rule");
        };
        // The tokenizer produces Delim('+') between 2n and 1, and render_tokens
        // concatenates them without inserting whitespace around delimiters.
        let pseudo = &rule.selectors[0].parts[0].simples[1];
        match pseudo {
            SimpleSelector::PseudoClass(value) => {
                assert!(
                    value.starts_with("nth-child("),
                    "expected nth-child(...), got: {value}"
                );
                assert!(
                    value.contains("2n"),
                    "should contain 2n, got: {value}"
                );
            }
            _ => panic!("expected PseudoClass, got: {:?}", pseudo),
        }
    }

    #[test]
    fn parses_has_relative_selector_with_nested_parentheses() {
        let stylesheet =
            parse_stylesheet("div:has(> span:nth-child(odd)) { color: red; }").unwrap();
        let Rule::Style(rule) = &stylesheet.rules[0] else {
            panic!("expected style rule");
        };
        let SimpleSelector::Has(relative) = &rule.selectors[0].parts[0].simples[1] else {
            panic!("expected parsed :has()");
        };
        assert_eq!(relative.len(), 1);
        assert_eq!(relative[0].leading_combinator, Combinator::Child);
        assert_eq!(
            relative[0].selector.parts[0].simples,
            vec![
                SimpleSelector::Type("span".to_string()),
                SimpleSelector::PseudoClass("nth-child(odd)".to_string()),
            ]
        );
    }

    #[test]
    fn has_uses_strict_relative_selector_list_and_disallows_nesting() {
        for invalid in [
            ":has()",
            ":has(123)",
            ":has(.valid, 123)",
            ":has(.child:has(.nested))",
            ":has(::before)",
            ":has(:before)",
            ":has(:AFTER)",
        ] {
            assert!(parse_selector_list(invalid).is_err(), "accepted {invalid:?}");
        }

        assert!(parse_selector_list(":has(:is(:has(*), script))").is_ok());
        assert!(parse_selector_list(":has(:where(:has(*)))").is_ok());
    }

    #[test]
    fn parses_pseudo_element_with_double_colon() {
        let stylesheet = parse_stylesheet("p::first-line { font-weight: bold; }").unwrap();
        let Rule::Style(rule) = &stylesheet.rules[0] else {
            panic!("expected style rule");
        };
        assert_eq!(
            rule.selectors[0].parts[0].simples[1],
            SimpleSelector::PseudoElement("first-line".to_string())
        );
    }

    #[test]
    fn parses_not_pseudo_class() {
        let stylesheet = parse_stylesheet(":not(.hidden) { display: block; }").unwrap();
        let Rule::Style(rule) = &stylesheet.rules[0] else {
            panic!("expected style rule");
        };
        assert_eq!(
            rule.selectors[0].parts[0].simples[0],
            SimpleSelector::Not(vec![Selector {
                parts: vec![SelectorPart {
                    combinator: None,
                    simples: vec![SimpleSelector::Class("hidden".to_string())],
                }],
            }])
        );
    }

    #[test]
    fn parses_not_with_type_selector() {
        let stylesheet = parse_stylesheet("div:not(span) { margin: 0; }").unwrap();
        let Rule::Style(rule) = &stylesheet.rules[0] else {
            panic!("expected style rule");
        };
        assert_eq!(
            rule.selectors[0].parts[0].simples[1],
            SimpleSelector::Not(vec![Selector {
                parts: vec![SelectorPart {
                    combinator: None,
                    simples: vec![SimpleSelector::Type("span".to_string())],
                }],
            }])
        );
    }

    #[test]
    fn parses_not_with_attribute_selector() {
        let stylesheet =
            parse_stylesheet(r#":not([disabled]) { opacity: 1; }"#).unwrap();
        let Rule::Style(rule) = &stylesheet.rules[0] else {
            panic!("expected style rule");
        };
        assert_eq!(
            rule.selectors[0].parts[0].simples[0],
            SimpleSelector::Not(vec![Selector {
                parts: vec![SelectorPart {
                    combinator: None,
                    simples: vec![SimpleSelector::Attribute {
                        name: "disabled".to_string(),
                        operator: None,
                        value: None,
                    }],
                }],
            }])
        );
    }

    #[test]
    fn parses_chained_pseudo_classes() {
        let stylesheet =
            parse_stylesheet("a:hover:focus { outline: none; }").unwrap();
        let Rule::Style(rule) = &stylesheet.rules[0] else {
            panic!("expected style rule");
        };
        assert_eq!(
            rule.selectors[0].parts[0].simples,
            vec![
                SimpleSelector::Type("a".to_string()),
                SimpleSelector::PseudoClass("hover".to_string()),
                SimpleSelector::PseudoClass("focus".to_string()),
            ]
        );
    }

    #[test]
    fn parses_complex_compound_selector() {
        // Type + class + attribute + pseudo-class + pseudo-element
        let stylesheet = parse_stylesheet(
            r#"input.form-control[type="email"]:focus::placeholder { color: gray; }"#,
        )
        .unwrap();
        let Rule::Style(rule) = &stylesheet.rules[0] else {
            panic!("expected style rule");
        };
        assert_eq!(
            rule.selectors[0].parts[0].simples,
            vec![
                SimpleSelector::Type("input".to_string()),
                SimpleSelector::Class("form-control".to_string()),
                SimpleSelector::Attribute {
                    name: "type".to_string(),
                    operator: Some(AttributeOperator::Equals),
                    value: Some("email".to_string()),
                },
                SimpleSelector::PseudoClass("focus".to_string()),
                SimpleSelector::PseudoElement("placeholder".to_string()),
            ]
        );
    }
}
