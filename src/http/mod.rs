//! HTTP/1.1 client implementation.
//!
//! This module provides a minimal HTTP/1.1 client built on top of `std::net::TcpStream`.
//! It supports basic request/response semantics including `Content-Length` and
//! `Transfer-Encoding: chunked` body handling, TLS via rustls, cookie management,
//! and automatic redirect following.

mod client;
mod connection;
mod cookie;
pub(crate) mod corp;
pub mod cors;
mod data_uri;
mod http2;
mod referrer;
mod request;
mod response;
mod site;
pub mod url;

#[cfg(test)]
mod request_tests;
#[cfg(test)]
mod response_tests;

pub(crate) use client::unpartitioned_cookie_access_allowed;
pub use client::{Client, HttpTransport, WebSocketRoute};
pub(crate) use connection::{is_public_ip, websocket_tls_config};
pub use connection::{send, send_with_options};
pub(crate) use cookie::CookieStoreSubscription;
pub use cookie::{Cookie, CookieChangeSubscription, CookieJar, SameSite};
pub(crate) use cookie::{CookieAccessContext, CookieStoreChange, CookieStoreWrite};
pub(crate) use data_uri::percent_decode_bytes;
pub use data_uri::{DataUri, parse_data_uri};
pub(crate) use referrer::{
    ReferrerPolicy, navigation_referrer_with_policy, response_referrer_policy,
};
pub(crate) use request::{
    CookieRequestContext, is_forbidden_request_header, is_http_token, is_valid_header,
};
pub use request::{HttpRequest, Method, default_user_agent};
pub use response::{HttpParseError, HttpResponse};
pub(crate) use site::SchemefulSite;
pub use url::Url;
