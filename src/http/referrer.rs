//! Referrer policy parsing and request URL reduction.

/// Policy used to compute an HTTP request's Referer header.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum ReferrerPolicy {
    NoReferrer,
    NoReferrerWhenDowngrade,
    SameOrigin,
    Origin,
    StrictOrigin,
    OriginWhenCrossOrigin,
    #[default]
    StrictOriginWhenCrossOrigin,
    UnsafeUrl,
}

impl ReferrerPolicy {
    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "no-referrer" => Some(Self::NoReferrer),
            "no-referrer-when-downgrade" => Some(Self::NoReferrerWhenDowngrade),
            "same-origin" => Some(Self::SameOrigin),
            "origin" => Some(Self::Origin),
            "strict-origin" => Some(Self::StrictOrigin),
            "origin-when-cross-origin" => Some(Self::OriginWhenCrossOrigin),
            "strict-origin-when-cross-origin" => Some(Self::StrictOriginWhenCrossOrigin),
            "unsafe-url" => Some(Self::UnsafeUrl),
            _ => None,
        }
    }

    /// HTML meta referrer keywords include four legacy aliases. Unlike an
    /// HTTP header token, its content value is not whitespace-trimmed.
    pub(crate) fn parse_meta(value: &str) -> Option<Self> {
        if value != value.trim() {
            return None;
        }
        match value.to_ascii_lowercase().as_str() {
            "never" => Some(Self::NoReferrer),
            "default" => Some(Self::default()),
            "always" => Some(Self::UnsafeUrl),
            "origin-when-crossorigin" => Some(Self::OriginWhenCrossOrigin),
            value => Self::parse(value),
        }
    }
}

/// Uses the last recognized token across repeated Referrer-Policy headers.
pub(crate) fn response_referrer_policy(headers: &[(String, String)]) -> Option<ReferrerPolicy> {
    headers
        .iter()
        .filter(|(name, _)| name.eq_ignore_ascii_case("referrer-policy"))
        .flat_map(|(_, value)| value.split(','))
        .filter_map(ReferrerPolicy::parse)
        .last()
}

/// Reduces a source URL for one destination according to its referrer policy.
pub(crate) fn navigation_referrer_with_policy(
    source: &str,
    destination: &str,
    policy: ReferrerPolicy,
) -> String {
    let (Ok(mut source), Ok(destination)) =
        (::url::Url::parse(source), ::url::Url::parse(destination))
    else {
        return String::new();
    };
    if !matches!(source.scheme(), "http" | "https") {
        return String::new();
    }
    source.set_fragment(None);
    let _ = source.set_username("");
    let _ = source.set_password(None);
    if !matches!(destination.scheme(), "http" | "https") {
        return String::new();
    }
    let same_origin = source.origin() == destination.origin();
    let downgrade =
        is_potentially_trustworthy(&source) && !is_potentially_trustworthy(&destination);
    let origin = format!("{}/", source.origin().ascii_serialization());
    let full = if source.as_str().len() > 4096 {
        origin.clone()
    } else {
        source.to_string()
    };
    match policy {
        ReferrerPolicy::NoReferrer => String::new(),
        ReferrerPolicy::NoReferrerWhenDowngrade => {
            if downgrade {
                String::new()
            } else {
                full
            }
        }
        ReferrerPolicy::SameOrigin => {
            if same_origin {
                full
            } else {
                String::new()
            }
        }
        ReferrerPolicy::Origin => origin,
        ReferrerPolicy::StrictOrigin => {
            if downgrade {
                String::new()
            } else {
                origin
            }
        }
        ReferrerPolicy::OriginWhenCrossOrigin => {
            if same_origin {
                full
            } else {
                origin
            }
        }
        ReferrerPolicy::StrictOriginWhenCrossOrigin => {
            if same_origin {
                full
            } else if downgrade {
                String::new()
            } else {
                origin
            }
        }
        ReferrerPolicy::UnsafeUrl => full,
    }
}

/// Secure Contexts treats loopback HTTP URLs as trustworthy too. A request
/// leaving one for ordinary HTTP is therefore a referrer-policy downgrade.
fn is_potentially_trustworthy(url: &::url::Url) -> bool {
    if url.scheme() == "https" {
        return true;
    }
    if url.scheme() != "http" {
        return false;
    }
    match url.host() {
        Some(::url::Host::Domain(host)) => {
            host == "localhost"
                || host == "localhost."
                || host.ends_with(".localhost")
                || host.ends_with(".localhost.")
        }
        Some(::url::Host::Ipv4(address)) => address.octets()[0] == 127,
        Some(::url::Host::Ipv6(address)) => address.is_loopback(),
        None => false,
    }
}
