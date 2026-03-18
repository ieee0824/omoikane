//! HTTP/1.1 client implementation.
//!
//! This module provides a minimal HTTP/1.1 client built on top of `std::net::TcpStream`.
//! It supports basic request/response semantics including `Content-Length` and
//! `Transfer-Encoding: chunked` body handling, TLS via rustls, cookie management,
//! and automatic redirect following.

mod url;
mod request;
mod response;
mod connection;
mod cookie;
mod client;

pub use url::Url;
pub use request::{HttpRequest, Method};
pub use response::HttpResponse;
pub use connection::send;
pub use cookie::{Cookie, CookieJar, SameSite};
pub use client::Client;
