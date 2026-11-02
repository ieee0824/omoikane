//! Response Permissions-Policy parsing for implemented policy-controlled APIs.

use super::{
    BootstrapBindings, Context, DocumentSecurityOrigin, JsNativeError, JsResult, JsValue,
    NativeFunction, Node, NodeType, StorageOrigin, ensure_same_origin_document,
    owner_document_for_node, parse_node_id, register_private_builtin_callable, with_host_state,
};
use base64::Engine;
use std::collections::HashMap;

/// Policy-controlled features represented by this browser's security state.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(super) enum PolicyFeature {
    Fullscreen,
    Geolocation,
    Gamepad,
    ScreenWakeLock,
    CrossOriginIsolated,
    StorageAccess,
    FocusWithoutUserActivation,
}

impl PolicyFeature {
    pub(super) const ALL: [Self; 7] = [
        Self::Fullscreen,
        Self::Geolocation,
        Self::Gamepad,
        Self::ScreenWakeLock,
        Self::CrossOriginIsolated,
        Self::StorageAccess,
        Self::FocusWithoutUserActivation,
    ];

    pub(super) fn name(self) -> &'static str {
        match self {
            Self::Fullscreen => "fullscreen",
            Self::Geolocation => "geolocation",
            Self::Gamepad => "gamepad",
            Self::ScreenWakeLock => "screen-wake-lock",
            Self::CrossOriginIsolated => "cross-origin-isolated",
            Self::StorageAccess => "storage-access",
            Self::FocusWithoutUserActivation => "focus-without-user-activation",
        }
    }

    pub(super) fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|feature| feature.name() == name)
    }

    /// Storage Access is delegated to child Documents by default; the other
    /// represented features require a same-origin child or explicit delegation.
    pub(super) fn default_allows(
        self,
        target: &DocumentSecurityOrigin,
        owner: &DocumentSecurityOrigin,
    ) -> bool {
        self == Self::StorageAccess || target == owner
    }

    fn default_allowlist(self, owner: &DocumentSecurityOrigin) -> Vec<String> {
        if self == Self::StorageAccess {
            vec!["*".to_owned()]
        } else {
            vec![serialize_origin(owner)]
        }
    }
}

/// Declared response policy for each represented feature. An omitted member
/// permits delegation; the feature's default allowlist applies when querying
/// the document's observable policy or serializing its allowlist.
#[derive(Clone, Debug, Default)]
pub(super) struct DocumentPolicy {
    declarations: HashMap<PolicyFeature, FeatureAllowlist>,
}

impl DocumentPolicy {
    pub(super) fn from_response_headers(headers: &[(String, String)]) -> Self {
        let declarations = PolicyFeature::ALL
            .into_iter()
            .filter_map(|feature| {
                FeatureAllowlist::from_response_headers(headers, feature)
                    .map(|allowlist| (feature, allowlist))
            })
            .collect();
        Self { declarations }
    }

    pub(super) fn allows(
        &self,
        feature: PolicyFeature,
        target: &DocumentSecurityOrigin,
        owner: &DocumentSecurityOrigin,
    ) -> bool {
        self.declarations
            .get(&feature)
            .map_or(feature.default_allows(target, owner), |allowlist| {
                allowlist.allows(target, owner)
            })
    }

    /// Tests only the response declaration. Without a member, the response
    /// does not restrict delegation; the iframe container policy still does.
    pub(super) fn permits_delegation(
        &self,
        feature: PolicyFeature,
        target: &DocumentSecurityOrigin,
        owner: &DocumentSecurityOrigin,
    ) -> bool {
        self.declarations
            .get(&feature)
            .is_none_or(|allowlist| allowlist.allows(target, owner))
    }

    /// Returns the declared allowlist when the feature remains enabled here.
    pub(super) fn allowlist(
        &self,
        feature: PolicyFeature,
        owner: &DocumentSecurityOrigin,
    ) -> Vec<String> {
        self.declarations.get(&feature).map_or_else(
            || feature.default_allowlist(owner),
            |policy| policy.serialize(owner),
        )
    }
}

/// One parsed response directive. An empty allowlist explicitly disables the
/// feature, while `All` allows delegation to any origin.
#[derive(Clone, Debug, PartialEq, Eq)]
enum FeatureAllowlist {
    All,
    Origins {
        self_origin: bool,
        explicit: Vec<OriginExpression>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum OriginExpression {
    Scheme(String),
    Host {
        scheme: String,
        host: String,
        subdomains_only: bool,
        port: HostPort,
        path: Option<String>,
        serialized: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum HostPort {
    Default,
    Any,
    Explicit(u16),
}

impl OriginExpression {
    fn allows(&self, target: &StorageOrigin) -> bool {
        match self {
            Self::Scheme(scheme) => url::Url::parse(&target.serialize())
                .is_ok_and(|url| scheme_matches(scheme, url.scheme())),
            Self::Host {
                scheme,
                host,
                subdomains_only,
                port,
                path,
                ..
            } => {
                let Ok(url) = url::Url::parse(&target.serialize()) else {
                    return false;
                };
                let Some(url::Host::Domain(target_host)) = url.host() else {
                    return false;
                };
                let matches_host = if host == "*" {
                    true
                } else if *subdomains_only {
                    target_host
                        .strip_suffix(host.as_str())
                        .is_some_and(|prefix| !prefix.is_empty() && prefix.ends_with('.'))
                } else {
                    target_host == host
                };
                let matches_port = match port {
                    HostPort::Default => url.port().is_none(),
                    HostPort::Any => true,
                    HostPort::Explicit(required) => url.port_or_known_default() == Some(*required),
                };
                // Permissions Policy matches the URL of the serialized origin,
                // whose path is always `/`.
                let matches_path = path.as_deref().is_none_or(|path| path == "/");
                scheme_matches(scheme, url.scheme()) && matches_host && matches_port && matches_path
            }
        }
    }

    fn serialize(&self) -> String {
        match self {
            Self::Scheme(scheme) => format!("{scheme}:"),
            Self::Host { serialized, .. } => serialized.clone(),
        }
    }
}

fn scheme_matches(source: &str, target: &str) -> bool {
    source == target
        || matches!(
            (source, target),
            ("http", "https") | ("ws", "wss" | "http" | "https") | ("wss", "https")
        )
}

impl FeatureAllowlist {
    /// Parses the supported member of a structured Permissions-Policy header.
    /// A malformed dictionary invalidates the whole header; directives for
    /// unsupported APIs are ignored after syntax validation.
    pub(super) fn from_response_headers(
        headers: &[(String, String)],
        feature: PolicyFeature,
    ) -> Option<Self> {
        let combined = headers
            .iter()
            .filter(|(name, _)| name.eq_ignore_ascii_case("permissions-policy"))
            .map(|(_, value)| value.as_str())
            .collect::<Vec<_>>()
            .join(",");
        if combined.is_empty() || !combined.is_ascii() {
            return None;
        }
        let members = split_structured(&combined, ',')
            .into_iter()
            .map(parse_structured_dictionary_member)
            .collect::<Option<Vec<_>>>()?;
        members
            .into_iter()
            // Structured Field dictionaries replace a duplicate key with its
            // last member, even when that member has an unsupported value.
            .filter(|(name, _)| *name == feature.name())
            .last()
            .and_then(|(_, value)| value)
            .and_then(Self::parse_value)
    }

    fn parse_value(value: &str) -> Option<Self> {
        let value = structured_item_without_parameters(value)?;
        if value == "*" {
            return Some(Self::All);
        }
        let inner = if value == "()" {
            ""
        } else if let Some(inner) = value.strip_prefix('(').and_then(|s| s.strip_suffix(')')) {
            inner
        } else if value == "self" || value.starts_with('"') {
            value
        } else {
            // Unknown bare member values are ignored, unlike unknown items
            // inside a syntactically valid Inner List.
            return None;
        };
        if inner.is_empty() && value != "()" {
            return None;
        }

        let items = split_structured(inner, ' ')
            .into_iter()
            .filter(|item| !item.trim().is_empty())
            .map(structured_item_without_parameters)
            .collect::<Option<Vec<_>>>()?;
        if items.iter().any(|item| !is_structured_bare_item(item)) {
            return None;
        }
        if items.contains(&"*") {
            return Some(Self::All);
        }

        let mut self_origin = false;
        let mut explicit = Vec::new();
        for item in items {
            if item == "self" {
                self_origin = true;
            } else if item.starts_with('"') {
                if !is_structured_string(item) {
                    return None;
                }
                // A well-formed String which is not a supported origin
                // expression contributes nothing to the allowlist.
                if let Some(origin) = item
                    .strip_prefix('"')
                    .and_then(|item| item.strip_suffix('"'))
                    .and_then(parse_origin_expression)
                {
                    explicit.push(origin);
                }
            } else if !is_ignored_structured_item(item) {
                return None;
            } else {
                // Other valid Inner List items are ignored by the policy
                // processing algorithm; an empty result denies the feature.
            }
        }
        Some(Self::Origins {
            self_origin,
            explicit,
        })
    }

    pub(super) fn allows(
        &self,
        target: &DocumentSecurityOrigin,
        owner: &DocumentSecurityOrigin,
    ) -> bool {
        match self {
            Self::All => true,
            Self::Origins {
                self_origin,
                explicit,
            } => {
                (*self_origin && target == owner)
                    || matches!(target, DocumentSecurityOrigin::Tuple(origin) if explicit.iter().any(|expression| expression.allows(origin)))
            }
        }
    }

    fn serialize(&self, owner: &DocumentSecurityOrigin) -> Vec<String> {
        match self {
            Self::All => vec!["*".to_owned()],
            Self::Origins {
                self_origin,
                explicit,
            } => {
                let mut origins = Vec::new();
                if *self_origin {
                    origins.push(serialize_origin(owner));
                }
                for origin in explicit {
                    let serialized = origin.serialize();
                    if !origins.contains(&serialized) {
                        origins.push(serialized);
                    }
                }
                origins
            }
        }
    }
}

fn split_structured(value: &str, delimiter: char) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut depth = 0_usize;
    let mut quoted = false;
    let mut escaped = false;
    for (index, ch) in value.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if quoted && ch == '\\' {
            escaped = true;
            continue;
        }
        if ch == '"' {
            quoted = !quoted;
            continue;
        }
        if quoted {
            continue;
        }
        match ch {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            _ if ch == delimiter && depth == 0 => {
                parts.push(&value[start..index]);
                start = index + ch.len_utf8();
            }
            _ => {}
        }
    }
    parts.push(&value[start..]);
    parts
}

fn is_structured_token(value: &str) -> bool {
    let mut bytes = value.bytes();
    bytes
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == b'*')
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~:/".contains(&byte))
}

fn is_structured_key(value: &str) -> bool {
    let mut bytes = value.bytes();
    bytes
        .next()
        .is_some_and(|first| first.is_ascii_lowercase() || first == b'*')
        && bytes.all(|byte| {
            byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || matches!(byte, b'_' | b'-' | b'.' | b'*')
        })
}

fn is_structured_string(value: &str) -> bool {
    let Some(inner) = value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
    else {
        return false;
    };
    let mut bytes = inner.bytes();
    while let Some(byte) = bytes.next() {
        match byte {
            b'\\' => {
                if !matches!(bytes.next(), Some(b'\\' | b'"')) {
                    return false;
                }
            }
            b'"' => return false,
            0x20..=0x7e => {}
            _ => return false,
        }
    }
    true
}

fn is_structured_bare_item(value: &str) -> bool {
    is_structured_string(value) || is_ignored_structured_item(value)
}

fn structured_item_without_parameters(value: &str) -> Option<&str> {
    let mut parts = split_structured(value, ';').into_iter();
    let item = parts.next()?;
    if item.is_empty() || item.trim() != item {
        return None;
    }
    for parameter in parts {
        let parameter = parameter.trim_start_matches(' ');
        let (key, value) = parameter
            .split_once('=')
            .map_or((parameter, None), |(key, value)| (key, Some(value)));
        if !is_structured_key(key) || value.is_some_and(|value| !is_structured_bare_item(value)) {
            return None;
        }
    }
    Some(item)
}

fn parse_structured_dictionary_member(member: &str) -> Option<(&str, Option<&str>)> {
    let member = member.trim_matches(|ch| ch == ' ' || ch == '\t');
    let name_end = member
        .find(|ch| ch == '=' || ch == ';')
        .unwrap_or(member.len());
    let name = &member[..name_end];
    if !is_structured_key(name) {
        return None;
    }
    let suffix = &member[name_end..];
    if let Some(value) = suffix.strip_prefix('=') {
        is_structured_member_value(value).then_some((name, Some(value)))
    } else if suffix.is_empty() {
        Some((name, None))
    } else {
        let true_item = format!("?1{suffix}");
        is_structured_member_value(&true_item).then_some((name, None))
    }
}

fn is_structured_member_value(value: &str) -> bool {
    let Some(item) = structured_item_without_parameters(value) else {
        return false;
    };
    if let Some(inner) = item
        .strip_prefix('(')
        .and_then(|item| item.strip_suffix(')'))
    {
        return split_structured(inner, ' ')
            .into_iter()
            .filter(|item| !item.is_empty())
            .all(|item| {
                structured_item_without_parameters(item).is_some_and(is_structured_bare_item)
            });
    }
    is_structured_bare_item(item)
}

fn is_ignored_structured_item(value: &str) -> bool {
    if is_structured_token(value) || matches!(value, "?0" | "?1") {
        return true;
    }
    if let Some(bytes) = value
        .strip_prefix(':')
        .and_then(|value| value.strip_suffix(':'))
    {
        return bytes
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/' | b'='))
            && (base64::engine::general_purpose::STANDARD
                .decode(bytes)
                .is_ok()
                || base64::engine::general_purpose::STANDARD_NO_PAD
                    .decode(bytes)
                    .is_ok());
    }
    let unsigned = value.strip_prefix('-').unwrap_or(value);
    let (integer, fraction) = unsigned
        .split_once('.')
        .map_or((unsigned, None), |(integer, fraction)| {
            (integer, Some(fraction))
        });
    !integer.is_empty()
        && integer.len() <= (if fraction.is_some() { 12 } else { 15 })
        && integer.bytes().all(|byte| byte.is_ascii_digit())
        && fraction.is_none_or(|fraction| {
            !fraction.is_empty()
                && fraction.len() <= 3
                && fraction.bytes().all(|byte| byte.is_ascii_digit())
        })
}

fn serialize_tuple_origin(origin: &StorageOrigin) -> String {
    url::Url::parse(&origin.serialize())
        .map(|url| url.origin().ascii_serialization())
        .unwrap_or_default()
}

fn serialize_origin(origin: &DocumentSecurityOrigin) -> String {
    match origin {
        DocumentSecurityOrigin::Tuple(origin) => serialize_tuple_origin(origin),
        DocumentSecurityOrigin::Opaque(_) => "null".to_owned(),
    }
}

/// Evaluates one iframe container directive against the Document committed in
/// that frame. The declared `src` origin is distinct from a later navigation's
/// actual origin, so an `allow="feature"` entry does not grant that navigation.
pub(super) fn container_allows(
    feature: PolicyFeature,
    allow: Option<&str>,
    allowfullscreen: bool,
    target: &DocumentSecurityOrigin,
    owner: &DocumentSecurityOrigin,
    declared_src: Option<&DocumentSecurityOrigin>,
) -> bool {
    let directive = allow.and_then(|allow| {
        allow
            .split(';')
            .filter(|entry| {
                entry
                    .split_ascii_whitespace()
                    .next()
                    .is_some_and(|name| name.eq_ignore_ascii_case(feature.name()))
            })
            .last()
    });
    let Some(directive) = directive else {
        return feature.default_allows(target, owner)
            || (feature == PolicyFeature::Fullscreen && allowfullscreen);
    };
    let tokens = directive
        .split_ascii_whitespace()
        .skip(1)
        .collect::<Vec<_>>();
    if tokens.is_empty() {
        return declared_src.is_some_and(|src| target == src);
    }
    tokens.into_iter().any(|token| {
        token == "*"
            || (token.eq_ignore_ascii_case("'self'") && target == owner)
            || (token.eq_ignore_ascii_case("'src'")
                && declared_src.is_some_and(|src| target == src))
            || parse_container_expression(token).is_some_and(|allowed| {
                matches!(target, DocumentSecurityOrigin::Tuple(origin) if allowed.allows(origin))
            })
    })
}

fn parse_container_expression(token: &str) -> Option<OriginExpression> {
    // PP §9.3 parses an iframe allow URL, then stores its serialized origin as
    // a source expression. Its path, query, and default-port spelling disappear.
    // Subdomain and port wildcards are valid in response headers, but not in
    // the iframe allow attribute's URL syntax.
    if token.contains('*') {
        return None;
    }
    if !token.contains("://") {
        return parse_origin_expression(token);
    }
    let remainder = token.split_once("://")?.1;
    let authority = remainder
        .split(|character| matches!(character, '/' | '?' | '#'))
        .next()?;
    if authority.contains('@') || authority.contains('\\') {
        return None;
    }
    if let Ok(url) = url::Url::parse(token) {
        if matches!(url.scheme(), "http" | "https")
            && url.host_str().is_some()
            && url.username().is_empty()
            && url.password().is_none()
        {
            return parse_origin_expression(&url.origin().ascii_serialization());
        }
    }
    None
}

fn parse_origin_expression(origin: &str) -> Option<OriginExpression> {
    if let Some(scheme) = origin.strip_suffix(':') {
        let mut bytes = scheme.bytes();
        if bytes.next().is_some_and(|byte| byte.is_ascii_alphabetic())
            && bytes.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'-' | b'.'))
        {
            return Some(OriginExpression::Scheme(scheme.to_ascii_lowercase()));
        }
    }
    let (authority, path) = host_source_parts(origin)?;
    if authority.contains('*') {
        return parse_wildcard_origin(origin);
    }
    let url = url::Url::parse(origin).ok()?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || url.query().is_some()
        || url.fragment().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return None;
    }
    let port = explicit_host_port(origin).map_or(HostPort::Default, HostPort::Explicit);
    host_expression(&url, false, false, port, path)
}

fn host_source_parts(value: &str) -> Option<(&str, Option<&str>)> {
    let remainder = value.split_once("://")?.1;
    let (authority, path) = remainder.find('/').map_or((remainder, None), |index| {
        (&remainder[..index], Some(&remainder[index..]))
    });
    if authority.is_empty()
        || !authority
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b':' | b'*'))
        || path.is_some_and(|path| !valid_csp_path(path))
    {
        return None;
    }
    Some((authority, path))
}

fn valid_csp_path(path: &str) -> bool {
    let bytes = path.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte == b'%' {
            if !bytes
                .get(index + 1..index + 3)
                .is_some_and(|hex| hex.iter().all(|byte| byte.is_ascii_hexdigit()))
            {
                return false;
            }
            index += 3;
        } else if byte.is_ascii_alphanumeric()
            || matches!(
                byte,
                b'/' | b'-'
                    | b'.'
                    | b'_'
                    | b'~'
                    | b'!'
                    | b'$'
                    | b'&'
                    | b'\''
                    | b'('
                    | b')'
                    | b'*'
                    | b'+'
                    | b'='
                    | b':'
                    | b'@'
            )
        {
            index += 1;
        } else {
            return false;
        }
    }
    true
}

fn explicit_host_port(value: &str) -> Option<u16> {
    let authority = host_source_parts(value)?.0;
    authority.rsplit_once(':')?.1.parse().ok()
}

fn host_expression(
    url: &url::Url,
    subdomains_only: bool,
    any_host: bool,
    port: HostPort,
    path: Option<&str>,
) -> Option<OriginExpression> {
    let url::Host::Domain(host) = url.host()? else {
        return None;
    };
    let host_expression = if any_host {
        "*".to_owned()
    } else if subdomains_only {
        format!("*.{host}")
    } else {
        host.to_owned()
    };
    let mut serialized = format!("{}://{host_expression}", url.scheme());
    match &port {
        HostPort::Default => {}
        HostPort::Any => serialized.push_str(":*"),
        HostPort::Explicit(value) => {
            serialized.push(':');
            serialized.push_str(&value.to_string());
        }
    }
    if let Some(path) = path {
        serialized.push_str(path);
    }
    Some(OriginExpression::Host {
        scheme: url.scheme().to_owned(),
        host: if any_host {
            "*".to_owned()
        } else {
            host.to_owned()
        },
        subdomains_only,
        port,
        path: path.map(str::to_owned),
        serialized,
    })
}

fn parse_wildcard_origin(origin: &str) -> Option<OriginExpression> {
    let (scheme, _) = origin.split_once("://")?;
    if !matches!(scheme.to_ascii_lowercase().as_str(), "http" | "https") {
        return None;
    }
    let (authority, path) = host_source_parts(origin)?;
    let subdomains_only = authority.starts_with("*.");
    let any_host = authority == "*" || authority.starts_with("*:");
    let authority = if subdomains_only {
        authority.strip_prefix("*.")?.to_owned()
    } else if any_host {
        format!("example.test{}", authority.strip_prefix('*')?)
    } else {
        authority.to_owned()
    };
    let any_port = authority.ends_with(":*");
    let authority = authority.strip_suffix(":*").unwrap_or(&authority);
    if authority.contains('*') || (!subdomains_only && !any_host && !any_port) {
        return None;
    }
    let url = url::Url::parse(&format!("{scheme}://{authority}")).ok()?;
    if url.host_str().is_none()
        || url.query().is_some()
        || url.fragment().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return None;
    }
    let port = if any_port {
        HostPort::Any
    } else {
        explicit_host_port(origin).map_or(HostPort::Default, HostPort::Explicit)
    };
    host_expression(&url, subdomains_only, any_host, port, path)
}

fn policy_query_native(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let document_id = parse_node_id(args.first(), context)?;
    let owner = with_host_state(|host| {
        let state = host.borrow();
        state
            .get_node(document_id)
            .and_then(|node| {
                if node.node_type() == NodeType::Document {
                    Some(document_id)
                } else {
                    owner_document_for_node(&node)
                        .or_else(|| state.node_lifetime_owner(document_id))
                        .map(|owner| owner.identity())
                }
            })
            .ok_or_else(|| {
                JsNativeError::typ()
                    .with_message("Policy owner is unavailable")
                    .into()
            })
    })?;
    ensure_same_origin_document(context, owner)?;
    let operation = args
        .get(1)
        .cloned()
        .unwrap_or_default()
        .to_string(context)?
        .to_std_string_escaped();
    let feature_name = args
        .get(2)
        .cloned()
        .unwrap_or_default()
        .to_string(context)?
        .to_std_string_escaped();
    let feature = PolicyFeature::from_name(&feature_name);
    let explicit_origin = args.get(3).filter(|value| !value.is_undefined());
    let explicit_origin = explicit_origin
        .map(|value| {
            value
                .to_string(context)
                .map(|value| value.to_std_string_escaped())
        })
        .transpose()?;
    let parsed_origin = explicit_origin
        .as_deref()
        .and_then(StorageOrigin::from_url)
        .map(DocumentSecurityOrigin::Tuple);
    let result = with_host_state(|state| {
        let state = state.borrow();
        if let Some(frame) = state
            .get_node(document_id)
            .filter(|node| node.has_tag_name("iframe"))
        {
            let policy = state.frame_policy(&frame)?;
            let target = if explicit_origin.is_some() {
                parsed_origin.as_ref()
            } else {
                Some(&policy.origin)
            };
            return policy.query(&operation, feature, target);
        }
        if !state.document_is_active(document_id)
            || state
                .get_node(document_id)
                .is_none_or(|node| node.node_type() != NodeType::Document)
        {
            return Err(JsNativeError::typ()
                .with_message("PermissionsPolicy owner is not an active Document")
                .into());
        }
        let value = match operation.as_str() {
            "features" => serde_json::json!(PolicyFeature::ALL.map(PolicyFeature::name)),
            "allowedFeatures" => serde_json::json!(
                PolicyFeature::ALL
                    .into_iter()
                    .filter(|feature| state.feature_enabled_in_document(document_id, *feature))
                    .map(PolicyFeature::name)
                    .collect::<Vec<_>>()
            ),
            "allowsFeature" => {
                let target = if explicit_origin.is_some() {
                    parsed_origin.as_ref()
                } else {
                    state.document_security_origins.get(&document_id)
                };
                serde_json::json!(feature.zip(target).is_some_and(|(feature, target)| {
                    state.feature_allows_origin(document_id, feature, target)
                }))
            }
            "getAllowlistForFeature" => serde_json::json!(
                feature
                    .map(|feature| state.feature_allowlist(document_id, feature))
                    .unwrap_or_default()
            ),
            _ => {
                return Err(JsNativeError::typ()
                    .with_message("Unknown PermissionsPolicy operation")
                    .into());
            }
        };
        Ok(value)
    })?;
    JsValue::from_json(&result, context)
}

/// Registers the private, document-scoped policy introspection hook.
pub(super) fn register(context: &mut Context, bindings: &mut BootstrapBindings) -> JsResult<()> {
    register_private_builtin_callable(
        context,
        bindings,
        boa_engine::js_string!("__omoikane_permissions_policy_query"),
        4,
        NativeFunction::from_copy_closure(policy_query_native),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fullscreen_header_parses_empty_self_and_explicit_origins() {
        let parse = |value: &str| {
            FeatureAllowlist::from_response_headers(
                &[("Permissions-Policy".to_owned(), value.to_owned())],
                PolicyFeature::Fullscreen,
            )
        };
        let owner =
            DocumentSecurityOrigin::Tuple(StorageOrigin::from_url("https://owner.test/").unwrap());
        let child = DocumentSecurityOrigin::Tuple(
            StorageOrigin::from_url("https://child.test:8443/").unwrap(),
        );
        let other =
            DocumentSecurityOrigin::Tuple(StorageOrigin::from_url("https://other.test/").unwrap());
        let blocked = parse("geolocation=*, fullscreen=()").unwrap();
        assert!(!blocked.allows(&owner, &owner));
        assert!(!blocked.allows(&child, &owner));

        let allowed = parse("fullscreen=(self \"https://child.test:8443\")").unwrap();
        assert!(allowed.allows(&owner, &owner));
        assert!(allowed.allows(&child, &owner));
        assert!(!allowed.allows(&other, &owner));
        assert!(parse("fullscreen=*").unwrap().allows(&other, &owner));
        assert!(
            !parse("fullscreen=(invalid)")
                .unwrap()
                .allows(&owner, &owner)
        );
        assert!(!parse("fullscreen=(?0 42)").unwrap().allows(&owner, &owner));
        assert!(
            parse("fullscreen=(self invalid)")
                .unwrap()
                .allows(&owner, &owner)
        );
        assert!(parse("fullscreen=(self *)").unwrap().allows(&other, &owner));
        assert!(
            parse("fullscreen=(\"https://child.test:8443\";note=ok)")
                .unwrap()
                .allows(&child, &owner)
        );
        assert!(
            !parse("fullscreen=(\"invalid origin\")")
                .unwrap()
                .allows(&owner, &owner)
        );
    }

    #[test]
    fn quoted_host_and_port_wildcards_match_only_their_intended_origins() {
        let parse = |value: &str| {
            FeatureAllowlist::from_response_headers(
                &[("Permissions-Policy".to_owned(), value.to_owned())],
                PolicyFeature::Geolocation,
            )
            .unwrap()
        };
        let origin =
            |value: &str| DocumentSecurityOrigin::Tuple(StorageOrigin::from_url(value).unwrap());
        let owner = origin("https://owner.test/");
        let host = parse("geolocation=(\"https://*.example.com\")");
        assert!(host.allows(&origin("https://maps.example.com/"), &owner));
        assert!(host.allows(&origin("https://a.b.example.com/"), &owner));
        assert!(!host.allows(&origin("https://example.com/"), &owner));
        assert!(!host.allows(&origin("https://maps.example.com:8443/"), &owner));
        assert!(!host.allows(&origin("https://evil-example.com/"), &owner));
        assert_eq!(host.serialize(&owner), vec!["https://*.example.com"]);

        let port = parse("geolocation=(\"https://example.com:*\")");
        assert!(port.allows(&origin("https://example.com/"), &owner));
        assert!(port.allows(&origin("https://example.com:8443/"), &owner));
        assert!(!port.allows(&origin("http://example.com:8443/"), &owner));
        assert!(!port.allows(&origin("https://sub.example.com:8443/"), &owner));
        assert_eq!(port.serialize(&owner), vec!["https://example.com:*"]);

        let both = parse("geolocation=(\"https://*.example.com:*\")");
        assert!(both.allows(&origin("https://a.example.com:8443/"), &owner));
        assert!(!both.allows(&origin("https://example.com:8443/"), &owner));
    }

    #[test]
    fn scheme_source_matches_its_scheme_and_secure_upgrade() {
        let parse = |value: &str| {
            FeatureAllowlist::from_response_headers(
                &[("Permissions-Policy".to_owned(), value.to_owned())],
                PolicyFeature::Geolocation,
            )
            .unwrap()
        };
        let origin =
            |value: &str| DocumentSecurityOrigin::Tuple(StorageOrigin::from_url(value).unwrap());
        let owner = origin("https://owner.test/");
        let https = parse("geolocation=(\"https:\")");
        assert!(https.allows(&origin("https://child.test:8443/"), &owner));
        assert!(!https.allows(&origin("http://child.test/"), &owner));
        assert_eq!(https.serialize(&owner), vec!["https:"]);

        let http = parse("geolocation=(\"http:\")");
        assert!(http.allows(&origin("http://child.test/"), &owner));
        assert!(http.allows(&origin("https://child.test/"), &owner));
        assert!(container_allows(
            PolicyFeature::Geolocation,
            Some("geolocation https:"),
            false,
            &origin("https://child.test/"),
            &owner,
            None
        ));
    }

    #[test]
    fn http_host_sources_upgrade_scheme_but_keep_csp_port_matching() {
        let parse = |source: &str| {
            FeatureAllowlist::from_response_headers(
                &[(
                    "Permissions-Policy".to_owned(),
                    format!("geolocation=(\"{source}\")"),
                )],
                PolicyFeature::Geolocation,
            )
            .unwrap()
        };
        let origin =
            |value: &str| DocumentSecurityOrigin::Tuple(StorageOrigin::from_url(value).unwrap());
        let owner = origin("https://owner.test/");
        for (source, target, allowed) in [
            ("http://example.test", "http://example.test/", true),
            ("http://example.test", "https://example.test/", true),
            ("http://example.test", "https://example.test:8443/", false),
            ("http://example.test:80", "http://example.test/", true),
            ("http://example.test:80", "https://example.test/", false),
            ("http://example.test:80", "https://example.test:80/", true),
            ("http://example.test:443", "https://example.test/", true),
            ("http://example.test:443", "http://example.test/", false),
            (
                "http://example.test:8443",
                "https://example.test:8443/",
                true,
            ),
            ("https://example.test", "http://example.test/", false),
            ("https://*", "https://other.test/", true),
            ("https://*", "https://other.test:8443/", false),
            ("https://*", "http://other.test/", false),
            ("https://*", "https://127.0.0.1/", false),
            ("https://*:*", "https://other.test:8443/", true),
            ("https://*:443", "https://other.test/", true),
            ("http://*", "https://other.test/", true),
            ("http://*.example.test", "https://a.example.test/", true),
            ("http://*.example.test", "https://example.test/", false),
            (
                "http://*.example.test",
                "https://a.example.test:8443/",
                false,
            ),
            ("http://*.example.test:80", "https://a.example.test/", false),
            (
                "http://*.example.test:80",
                "https://a.example.test:80/",
                true,
            ),
            (
                "http://*.example.test:*",
                "https://a.example.test:8443/",
                true,
            ),
        ] {
            assert_eq!(
                parse(source).allows(&origin(target), &owner),
                allowed,
                "{source} → {target}"
            );
        }
        assert_eq!(
            parse("http://example.test:80").serialize(&owner),
            vec!["http://example.test:80"]
        );
        assert_eq!(
            url::Url::parse("http://example.test:80/").unwrap().port(),
            None
        );
        assert_eq!(
            url::Url::parse("https://example.test:443/").unwrap().port(),
            None
        );
        let root_path = parse("https://example.test/");
        assert!(root_path.allows(&origin("https://example.test/"), &owner));
        assert_eq!(root_path.serialize(&owner), vec!["https://example.test/"]);
        let other_path = parse("https://example.test/maps");
        assert!(!other_path.allows(&origin("https://example.test/"), &owner));
        assert_eq!(
            other_path.serialize(&owner),
            vec!["https://example.test/maps"]
        );
        assert!(parse("http://*.example.test/").allows(&origin("https://a.example.test/"), &owner));
        assert!(
            !parse("http://*.example.test/maps").allows(&origin("https://a.example.test/"), &owner)
        );
        for source in ["https://127.0.0.1", "https://*.127.0.0.1"] {
            let allowlist = parse(source);
            assert_eq!(
                allowlist.serialize(&owner),
                Vec::<String>::new(),
                "{source}"
            );
            assert!(
                !allowlist.allows(&origin("https://127.0.0.1/"), &owner),
                "{source}"
            );
        }
    }

    #[test]
    fn duplicate_header_members_use_the_last_value() {
        let owner =
            DocumentSecurityOrigin::Tuple(StorageOrigin::from_url("https://owner.test/").unwrap());
        let parse = |headers: &[(&str, &str)]| {
            FeatureAllowlist::from_response_headers(
                &headers
                    .iter()
                    .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
                    .collect::<Vec<_>>(),
                PolicyFeature::Fullscreen,
            )
        };
        assert!(
            !parse(&[("Permissions-Policy", "fullscreen=*, fullscreen=()")])
                .unwrap()
                .allows(&owner, &owner)
        );
        assert!(
            !parse(&[
                ("Permissions-Policy", "fullscreen=*"),
                ("Permissions-Policy", "fullscreen=()"),
            ])
            .unwrap()
            .allows(&owner, &owner)
        );
        assert!(
            parse(&[("Permissions-Policy", "fullscreen=(), fullscreen=*")])
                .unwrap()
                .allows(&owner, &owner)
        );
        assert!(parse(&[("Permissions-Policy", "fullscreen=*, fullscreen=?0")]).is_none());
        assert!(parse(&[("Permissions-Policy", "fullscreen=*, fullscreen;note=*")]).is_none());
    }

    #[test]
    fn malformed_structured_parameters_and_strings_do_not_enable_wildcard() {
        let owner =
            DocumentSecurityOrigin::Tuple(StorageOrigin::from_url("https://owner.test/").unwrap());
        let child =
            DocumentSecurityOrigin::Tuple(StorageOrigin::from_url("https://child.test/").unwrap());
        for header in [
            "fullscreen=(*;bad=?)",
            "fullscreen=(* ?)",
            "fullscreen=*;bad=?",
            "fullscreen=(*);bad=?",
            "fullscreen=(*;bad=\"unterminated)",
            r#"fullscreen=("bad\q" *)"#,
            r#"fullscreen=("bad"junk *)"#,
        ] {
            let headers = [("Permissions-Policy".to_owned(), header.to_owned())];
            assert!(
                FeatureAllowlist::from_response_headers(&headers, PolicyFeature::Fullscreen)
                    .is_none(),
                "{header}"
            );
            let policy = DocumentPolicy::from_response_headers(&headers);
            assert!(
                !policy.allows(PolicyFeature::Fullscreen, &child, &owner),
                "{header}"
            );
        }
        let valid = FeatureAllowlist::from_response_headers(
            &[(
                "Permissions-Policy".to_owned(),
                "fullscreen=(*;note=ok);report-to=endpoint".to_owned(),
            )],
            PolicyFeature::Fullscreen,
        )
        .unwrap();
        assert!(valid.allows(&child, &owner));
    }

    #[test]
    fn malformed_dictionary_member_invalidates_the_whole_header() {
        let owner =
            DocumentSecurityOrigin::Tuple(StorageOrigin::from_url("https://owner.test/").unwrap());
        let child =
            DocumentSecurityOrigin::Tuple(StorageOrigin::from_url("https://child.test/").unwrap());
        for header in [
            "geolocation=*, x=?",
            "Geolocation=*",
            "geolocation =*",
            r#"geolocation=*, x="bad\q""#,
        ] {
            let headers = [("Permissions-Policy".to_owned(), header.to_owned())];
            assert!(
                FeatureAllowlist::from_response_headers(&headers, PolicyFeature::Geolocation)
                    .is_none(),
                "{header}"
            );
            let policy = DocumentPolicy::from_response_headers(&headers);
            assert!(policy.allows(PolicyFeature::Geolocation, &owner, &owner));
            assert!(!policy.allows(PolicyFeature::Geolocation, &child, &owner));
        }
        let headers = [
            ("Permissions-Policy".to_owned(), "geolocation=*".to_owned()),
            ("Permissions-Policy".to_owned(), "x=?".to_owned()),
        ];
        assert!(
            FeatureAllowlist::from_response_headers(&headers, PolicyFeature::Geolocation).is_none()
        );
        let valid = [(
            "Permissions-Policy".to_owned(),
            "geolocation=*, unknown=(\"hello world\";note=?1);report-to=endpoint".to_owned(),
        )];
        assert!(
            FeatureAllowlist::from_response_headers(&valid, PolicyFeature::Geolocation)
                .unwrap()
                .allows(&child, &owner)
        );
    }

    #[test]
    fn document_policy_separates_features_and_serializes_only_allowed_origins() {
        let owner =
            DocumentSecurityOrigin::Tuple(StorageOrigin::from_url("https://owner.test/").unwrap());
        let child =
            DocumentSecurityOrigin::Tuple(StorageOrigin::from_url("https://child.test/").unwrap());
        let policy = DocumentPolicy::from_response_headers(&[(
            "Permissions-Policy".to_owned(),
            "fullscreen=(), geolocation=(self \"https://child.test\"), cross-origin-isolated=()"
                .to_owned(),
        )]);
        assert!(!policy.allows(PolicyFeature::Fullscreen, &owner, &owner));
        assert!(policy.allows(PolicyFeature::Geolocation, &owner, &owner));
        assert!(policy.allows(PolicyFeature::Geolocation, &child, &owner));
        assert!(!policy.allows(PolicyFeature::CrossOriginIsolated, &owner, &owner));
        assert_eq!(
            policy.allowlist(PolicyFeature::Fullscreen, &owner),
            Vec::<String>::new()
        );
        assert_eq!(
            policy.allowlist(PolicyFeature::Geolocation, &owner),
            vec!["https://owner.test", "https://child.test"]
        );
        assert_eq!(PolicyFeature::from_name("unknown"), None);
        assert_eq!(
            PolicyFeature::from_name("geolocation"),
            Some(PolicyFeature::Geolocation)
        );

        let default = DocumentPolicy::default();
        assert!(!default.allows(PolicyFeature::Geolocation, &child, &owner));
        assert!(default.permits_delegation(PolicyFeature::Geolocation, &child, &owner));
        assert!(default.allows(PolicyFeature::CrossOriginIsolated, &owner, &owner));
        assert!(!default.allows(PolicyFeature::CrossOriginIsolated, &child, &owner));
        assert!(default.allows(PolicyFeature::StorageAccess, &child, &owner));
        assert_eq!(
            default.allowlist(PolicyFeature::StorageAccess, &owner),
            vec!["*"]
        );
        assert!(container_allows(
            PolicyFeature::StorageAccess,
            None,
            false,
            &child,
            &owner,
            None
        ));
    }

    #[test]
    fn iframe_src_delegation_stops_after_cross_origin_navigation() {
        let owner =
            DocumentSecurityOrigin::Tuple(StorageOrigin::from_url("https://owner.test/").unwrap());
        let declared = DocumentSecurityOrigin::Tuple(
            StorageOrigin::from_url("https://declared.test/").unwrap(),
        );
        let navigated = DocumentSecurityOrigin::Tuple(
            StorageOrigin::from_url("https://navigated.test/").unwrap(),
        );
        assert!(container_allows(
            PolicyFeature::Fullscreen,
            Some("fullscreen"),
            false,
            &declared,
            &owner,
            Some(&declared)
        ));
        assert!(!container_allows(
            PolicyFeature::Fullscreen,
            Some("fullscreen"),
            false,
            &navigated,
            &owner,
            Some(&declared)
        ));
        assert!(container_allows(
            PolicyFeature::Fullscreen,
            None,
            true,
            &navigated,
            &owner,
            Some(&declared)
        ));
        assert!(!container_allows(
            PolicyFeature::Geolocation,
            None,
            true,
            &navigated,
            &owner,
            Some(&declared)
        ));
        assert!(container_allows(
            PolicyFeature::Fullscreen,
            Some("fullscreen 'none' *"),
            false,
            &navigated,
            &owner,
            Some(&declared)
        ));
        assert!(!container_allows(
            PolicyFeature::Fullscreen,
            Some("fullscreen *; fullscreen 'none'"),
            false,
            &navigated,
            &owner,
            Some(&declared)
        ));
        assert!(container_allows(
            PolicyFeature::Fullscreen,
            Some("fullscreen 'none'; fullscreen *"),
            false,
            &navigated,
            &owner,
            Some(&declared)
        ));
        assert!(!container_allows(
            PolicyFeature::Fullscreen,
            Some("fullscreen https://*.example.com:*"),
            false,
            &DocumentSecurityOrigin::Tuple(
                StorageOrigin::from_url("https://a.example.com:8443/").unwrap()
            ),
            &owner,
            Some(&declared)
        ));
        assert!(!container_allows(
            PolicyFeature::Fullscreen,
            Some("fullscreen https://*.example.com:*"),
            false,
            &DocumentSecurityOrigin::Tuple(
                StorageOrigin::from_url("https://example.com:8443/").unwrap()
            ),
            &owner,
            Some(&declared)
        ));
    }

    #[test]
    fn iframe_allow_url_tokens_serialize_origin_before_source_matching() {
        // PP §9.3 drops the iframe URL path before PP §4.7 matches its source.
        let owner =
            DocumentSecurityOrigin::Tuple(StorageOrigin::from_url("https://owner.test/").unwrap());
        let child =
            DocumentSecurityOrigin::Tuple(StorageOrigin::from_url("https://child.test/").unwrap());
        assert!(container_allows(
            PolicyFeature::Geolocation,
            Some("geolocation https://child.test/"),
            false,
            &child,
            &owner,
            None
        ));
        assert!(container_allows(
            PolicyFeature::Geolocation,
            Some("geolocation https://child.test/maps"),
            false,
            &child,
            &owner,
            None
        ));
        assert!(!container_allows(
            PolicyFeature::Geolocation,
            Some("geolocation https://other.test/maps"),
            false,
            &child,
            &owner,
            None
        ));
        assert!(!container_allows(
            PolicyFeature::Geolocation,
            Some("geolocation https://*"),
            false,
            &child,
            &owner,
            None
        ));
        assert!(!container_allows(
            PolicyFeature::Geolocation,
            Some("geolocation https://child.test:8443/maps"),
            false,
            &child,
            &owner,
            None
        ));
        assert!(container_allows(
            PolicyFeature::Geolocation,
            Some("geolocation http://child.test/"),
            false,
            &child,
            &owner,
            None
        ));
        assert!(container_allows(
            PolicyFeature::Geolocation,
            Some("geolocation http://child.test/maps"),
            false,
            &child,
            &owner,
            None
        ));
        assert!(container_allows(
            PolicyFeature::Geolocation,
            Some("geolocation http://child.test:80/maps"),
            false,
            &child,
            &owner,
            None
        ));
        let grandchild = DocumentSecurityOrigin::Tuple(
            StorageOrigin::from_url("https://sub.child.test/").unwrap(),
        );
        assert!(!container_allows(
            PolicyFeature::Geolocation,
            Some("geolocation https://*.child.test/maps"),
            false,
            &grandchild,
            &owner,
            None
        ));
        assert!(container_allows(
            PolicyFeature::Geolocation,
            Some("geolocation http://child.test:80/"),
            false,
            &child,
            &owner,
            None
        ));
        assert!(container_allows(
            PolicyFeature::Geolocation,
            Some("geolocation http://child.test:443/"),
            false,
            &child,
            &owner,
            None
        ));
        assert!(container_allows(
            PolicyFeature::Geolocation,
            Some("geolocation http://child.test:443/maps"),
            false,
            &child,
            &owner,
            None
        ));
        for allowed in [
            "geolocation https://child.test/?query",
            "geolocation https://child.test/maps?query",
            "geolocation https://child.test/#fragment",
        ] {
            assert!(
                container_allows(
                    PolicyFeature::Geolocation,
                    Some(allowed),
                    false,
                    &child,
                    &owner,
                    None
                ),
                "{allowed}"
            );
        }
        for invalid in [
            "geolocation https://child.test\\maps",
            "geolocation https://user@child.test/maps",
            "geolocation https://:password@child.test/maps",
            "geolocation https://@child.test/maps",
        ] {
            assert!(
                !container_allows(
                    PolicyFeature::Geolocation,
                    Some(invalid),
                    false,
                    &child,
                    &owner,
                    None
                ),
                "{invalid}"
            );
        }
        let evil =
            DocumentSecurityOrigin::Tuple(StorageOrigin::from_url("https://evil.test/").unwrap());
        assert!(!container_allows(
            PolicyFeature::Geolocation,
            Some("geolocation https://child.test@evil.test/maps"),
            false,
            &evil,
            &owner,
            None
        ));
    }
}
