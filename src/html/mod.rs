//! HTML parsing primitives.
//!
//! This module currently exposes the HTML tokenizer, which converts source text
//! into HTML5-style tokens for later tree construction.

mod tokenizer;

pub use tokenizer::{Attribute, DoctypeToken, HtmlParseError, Token, Tokenizer};
