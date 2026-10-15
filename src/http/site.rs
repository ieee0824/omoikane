//! Schemeful site keys shared by cookies and partitioned browser state.

use super::Url;
use std::net::IpAddr;

/// A URL's scheme and registrable domain, or its complete host when it has no
/// registrable domain (such as an IP address or `localhost`). Ports are not
/// part of a site.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct SchemefulSite {
    scheme: String,
    domain: String,
}

impl SchemefulSite {
    pub(crate) fn from_url(url: &Url) -> Self {
        Self::from_parts(url.scheme(), url.host())
    }

    pub(crate) fn from_parts(scheme: &str, host: &str) -> Self {
        let host = host.trim_matches(['[', ']']);
        let domain = if host.parse::<IpAddr>().is_ok() {
            host
        } else {
            psl::domain_str(host).unwrap_or(host)
        };
        Self {
            scheme: scheme.to_ascii_lowercase(),
            domain: domain.to_ascii_lowercase(),
        }
    }

    /// Compares registrable domains while ignoring URL schemes and ports.
    pub(crate) fn is_schemelessly_same_site(&self, other: &Self) -> bool {
        self.domain == other.domain
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schemeful_site_groups_subdomains_without_crossing_scheme_or_host() {
        let site = SchemefulSite::from_url(&"https://a.example.co.uk:8443/one".parse().unwrap());
        assert_eq!(
            site,
            SchemefulSite::from_url(&"https://b.example.co.uk/two".parse().unwrap())
        );
        assert_ne!(
            site,
            SchemefulSite::from_url(&"http://b.example.co.uk/two".parse().unwrap())
        );
        assert_ne!(
            site,
            SchemefulSite::from_url(&"https://other.co.uk/two".parse().unwrap())
        );
        assert_ne!(
            SchemefulSite::from_parts("https", "127.0.0.1"),
            SchemefulSite::from_parts("https", "127.0.0.2")
        );
        assert_ne!(
            SchemefulSite::from_parts("https", "127.1.0.1"),
            SchemefulSite::from_parts("https", "127.2.0.1")
        );
    }

    #[test]
    fn schemeless_site_comparison_keeps_registrable_domain_boundaries() {
        let secure = SchemefulSite::from_parts("https", "a.example.co.uk");
        let insecure = SchemefulSite::from_parts("http", "b.example.co.uk");
        assert!(secure.is_schemelessly_same_site(&insecure));
        assert!(
            !secure.is_schemelessly_same_site(&SchemefulSite::from_parts("https", "other.co.uk"))
        );
    }
}
