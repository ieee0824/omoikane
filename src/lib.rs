//! Core library for the Omoikane headless browser engine.

// Layout and paint pipelines intentionally pass explicit rendering context.
#![allow(clippy::too_many_arguments)]

pub mod cdp;
pub mod css;
pub mod data;
pub mod dom;
pub mod ffi;
pub mod font;
pub mod html;
pub mod http;
pub mod js;
pub mod layout;
pub mod paint;
mod screenshot;
pub mod svg;
pub mod xml;
