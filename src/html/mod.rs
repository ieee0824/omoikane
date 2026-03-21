//! HTML parsing primitives.
//!
//! This module currently exposes the HTML tokenizer, which converts source text
//! into HTML5-style tokens for later tree construction.

pub(crate) mod encoding;
mod tokenizer;
mod tree_builder;

pub(crate) use encoding::decode_html_response;
pub use tokenizer::{Attribute, DoctypeToken, HtmlParseError, Token, Tokenizer};
pub use tree_builder::{InsertionMode, ParseResult, TreeBuilder};
