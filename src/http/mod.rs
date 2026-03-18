//! HTTP/1.1 client implementation.
//!
//! This module provides a minimal HTTP/1.1 client built on top of `std::net::TcpStream`.
//! It supports basic request/response semantics including `Content-Length` and
//! `Transfer-Encoding: chunked` body handling.

mod url;
mod request;
mod response;
mod connection;

pub use url::Url;
pub use request::{HttpRequest, Method};
pub use response::HttpResponse;
pub use connection::send;
