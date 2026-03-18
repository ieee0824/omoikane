//! HTML parsing primitives.
//!
//! This module currently exposes the HTML tokenizer, which converts source text
//! into HTML5-style tokens for later tree construction.

mod tokenizer;
mod tree_builder;

pub use tokenizer::{Attribute, DoctypeToken, HtmlParseError, Token, Tokenizer};
pub use tree_builder::{InsertionMode, ParseResult, TreeBuilder};
