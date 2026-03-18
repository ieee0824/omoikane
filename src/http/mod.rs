//! HTTP/1.1 client implementation.
//!
//! This module provides a minimal HTTP/1.1 client built on top of `std::net::TcpStream`.
//! It supports basic request/response semantics including `Content-Length` and
//! `Transfer-Encoding: chunked` body handling, TLS via rustls, cookie management,
//! and automatic redirect following.

mod client;
mod connection;
mod cookie;
mod http2;
mod request;
mod response;
mod url;

pub use client::Client;
pub use connection::send;
pub use cookie::{Cookie, CookieJar, SameSite};
pub use request::{HttpRequest, Method};
pub use response::HttpResponse;
pub use url::Url;
