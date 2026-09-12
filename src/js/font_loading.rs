//! Document-owned Font Loading API data, shared by style, layout and paint.
use super::*;
use crate::font::{Font, FontStyle, FontWeight, WebFontRegistry};

#[derive(Default)]
pub(super) struct FontStore {
    pub(super) maps: Option<JsValue>,
    next_id: u64,
    faces: HashMap<u64, Face>,
    members: HashMap<usize, Vec<u64>>,
    css: HashMap<usize, Vec<(String, u64)>>,
}

struct Face {
    owner: usize,
    family: String,
    weight: FontWeight,
    style: FontStyle,
    sources: Vec<(String, String)>,
    base: Option<crate::http::Url>,
    bytes: Option<Vec<u8>>,
    font: Option<Arc<Font>>,
    failure: Option<String>,
    documents: HashSet<usize>,
}

impl FontStore {
    pub(super) fn append_to(&self, document: usize, fonts: &mut WebFontRegistry) {
        if let Some(members) = self.members.get(&document) {
            for id in members {
                if let Some(face) = self.faces.get(id)
                    && let Some(font) = &face.font
                {
                    fonts.push_shared(&face.family, face.weight, face.style, font.clone());
                }
            }
        }
        if let Some(members) = self.css.get(&document) {
            for (_, id) in members {
                if let Some(face) = self.faces.get(id)
                    && let Some(font) = &face.font
                {
                    fonts.push_shared(&face.family, face.weight, face.style, font.clone());
                }
            }
        }
    }

    pub(super) fn remove_document(&mut self, document: usize) {
        self.members.remove(&document);
        self.css.remove(&document);
        self.faces.retain(|_, face| {
            face.documents.remove(&document);
            face.owner != document || !face.documents.is_empty()
        });
    }
}

pub(super) fn sync_stylesheets(
    state: &mut HostState,
    document: &NodeHandle,
    rules: Vec<crate::css::FontFaceRule>,
) {
    let previous = state
        .font_loading
        .css
        .remove(&document.identity())
        .unwrap_or_default();
    let mut remaining = previous;
    let mut current = Vec::new();
    let base = crate::paint::stylesheet::extract_document_base_url(
        document,
        state.base_url_for_document(document.identity()).as_ref(),
    );
    for rule in rules {
        // A CSS-connected FontFace survives descriptor edits. The source is
        // the identity boundary: changing or removing `src` disconnects the
        // old face and creates a new one. Searching the remaining ordered list
        // also keeps distinct rules with the same source distinct.
        let key = rule.src_url.clone();
        let reused = remaining
            .iter()
            .position(|(candidate, _)| candidate == &key)
            .map(|position| remaining.remove(position).1);
        let id = reused.unwrap_or_else(|| {
            state.font_loading.next_id += 1;
            let id = state.font_loading.next_id;
            state.font_loading.faces.insert(
                id,
                Face {
                    owner: document.identity(),
                    family: rule.font_family.clone(),
                    weight: FontWeight::parse(rule.font_weight.as_deref().unwrap_or("normal")),
                    style: FontStyle::parse(rule.font_style.as_deref().unwrap_or("normal")),
                    sources: vec![("url".into(), rule.src_url.clone())],
                    base: base.clone(),
                    bytes: None,
                    font: None,
                    failure: None,
                    documents: HashSet::new(),
                },
            );
            id
        });
        if let Some(face) = state.font_loading.faces.get_mut(&id) {
            face.family = rule.font_family;
            face.weight = FontWeight::parse(rule.font_weight.as_deref().unwrap_or("normal"));
            face.style = FontStyle::parse(rule.font_style.as_deref().unwrap_or("normal"));
            face.base = base.clone();
        }
        // Existing document rendering loads CSS fonts before layout. Retain
        // that behavior, but keep the resource in the document's face registry
        // so JS and subsequent style rebuilds reuse the same parsed font.
        let _ = load_face(state, document, id);
        current.push((key, id));
    }
    state.font_loading.css.insert(document.identity(), current);
}

fn error(message: impl ToString) -> JsError {
    JsNativeError::typ()
        .with_message(message.to_string())
        .into()
}

pub(super) fn register(context: &mut Context, host: &Rc<RefCell<HostState>>) -> JsResult<()> {
    let existing = host.borrow().font_loading.maps.clone();
    let maps = if let Some(maps) = existing {
        maps
    } else {
        let maps = context.eval(Source::from_bytes(
            "[new WeakMap(),new WeakMap(),new WeakMap()]",
        ))?;
        host.borrow_mut().font_loading.maps = Some(maps.clone());
        maps
    };
    context.register_global_property(
        js_string!("__omoikane_font_maps"),
        maps,
        boa_engine::property::Attribute::all(),
    )
}

fn parse_sources(source: &str) -> JsResult<Vec<(String, String)>> {
    use crate::css::CssToken as T;
    let tokens: Vec<_> = crate::css::tokenize(source)
        .map_err(error)?
        .into_iter()
        .filter(|token| !matches!(token, T::Whitespace))
        .collect();
    let mut index = 0;
    let mut sources = Vec::new();
    while index < tokens.len() {
        let (kind, value) = match &tokens[index] {
            T::Url(value) => {
                index += 1;
                ("url".to_string(), value.clone())
            }
            T::Ident(kind) if matches!(kind.to_ascii_lowercase().as_str(), "url" | "local") => {
                let kind = kind.to_ascii_lowercase();
                index += 1;
                if tokens.get(index) != Some(&T::ParenOpen) {
                    return Err(error("invalid font source"));
                }
                index += 1;
                let mut parts = Vec::new();
                while let Some(token) = tokens.get(index) {
                    match token {
                        T::String(value) if parts.is_empty() => {
                            parts.push(value.clone());
                            index += 1;
                            break;
                        }
                        T::Ident(value) if kind == "local" => parts.push(value.clone()),
                        T::ParenClose => break,
                        _ => return Err(error("invalid font source")),
                    }
                    index += 1;
                }
                if tokens.get(index) != Some(&T::ParenClose) {
                    return Err(error("invalid font source"));
                }
                index += 1;
                (kind, parts.join(" "))
            }
            _ => return Err(error("font source requires url() or local()")),
        };
        if value.is_empty() {
            return Err(error("empty font source"));
        }
        while let Some(T::Ident(hint)) = tokens.get(index) {
            if kind != "url" || !matches!(hint.to_ascii_lowercase().as_str(), "format" | "tech") {
                return Err(error("invalid font source hint"));
            }
            index += 1;
            if tokens.get(index) != Some(&T::ParenOpen) {
                return Err(error("invalid font hint"));
            }
            index += 1;
            let start = index;
            while matches!(
                tokens.get(index),
                Some(T::Ident(_) | T::String(_) | T::Comma)
            ) {
                index += 1;
            }
            if index == start || tokens.get(index) != Some(&T::ParenClose) {
                return Err(error("invalid font hint"));
            }
            index += 1;
        }
        sources.push((kind, value));
        if index < tokens.len() {
            if tokens[index] != T::Comma || index + 1 == tokens.len() {
                return Err(error("invalid font source list"));
            }
            index += 1;
        }
    }
    if sources.is_empty() {
        return Err(error("empty font source list"));
    }
    Ok(sources)
}

fn parse_font_query(input: &str) -> JsResult<serde_json::Value> {
    use crate::css::{CssToken as T, Value};
    let tokens = crate::css::tokenize(input).map_err(error)?;
    if tokens
        .iter()
        .any(|token| matches!(token, T::Semicolon | T::CurlyOpen | T::CurlyClose))
    {
        return Err(error("invalid font shorthand"));
    }
    let declarations = crate::css::parse_style_attribute(&format!("font:{input}"));
    let values: HashMap<_, _> = declarations
        .into_iter()
        .map(|d| (d.name, d.value))
        .collect();
    let Some(Value::Keyword(family)) = values.get("font-family") else {
        return Err(error("invalid font shorthand"));
    };
    if values.len() != 7
        || matches!(
            family.as_str(),
            "inherit" | "initial" | "unset" | "revert" | "revert-layer"
        )
    {
        return Err(error("invalid font shorthand"));
    }
    let mut families = Vec::new();
    let family_tokens = crate::css::tokenize(family).map_err(error)?;
    for tokens in family_tokens.split(|token| *token == T::Comma) {
        let parts: Vec<_> = tokens
            .iter()
            .filter_map(|token| match token {
                T::Ident(value) | T::String(value) => Some(value.as_str()),
                _ => None,
            })
            .collect();
        families.push(parts.join(" "));
    }
    let weight = match values.get("font-weight") {
        Some(Value::Number(weight)) => *weight,
        Some(Value::Keyword(weight)) if weight == "bold" || weight == "bolder" => 700.0,
        Some(Value::Keyword(weight)) if weight == "lighter" => 100.0,
        _ => 400.0,
    };
    let style = match values.get("font-style") {
        Some(Value::Keyword(style)) => style.as_str(),
        _ => "normal",
    };
    Ok(serde_json::json!({"families":families,"weight":weight,"style":style}))
}

fn used_fonts(state: &mut HostState, document: &NodeHandle) -> Vec<serde_json::Value> {
    state.ensure_style_resolver(document);
    let Some(resolver) = state
        .document_styles
        .get_mut(&document.identity())
        .and_then(|entry| entry.resolver.as_mut())
    else {
        return Vec::new();
    };
    let mut stack = vec![(document.clone(), String::new())];
    let mut samples: HashMap<String, HashSet<char>> = HashMap::new();
    while let Some((node, mut font)) = stack.pop() {
        if node.node_type() == NodeType::Element {
            let style = resolver.computed_style(&node);
            if matches!(style.get("display"), Some(ComputedValue::Keyword(value)) if value == "none")
            {
                continue;
            }
            let fragment = crate::layout::FragmentStyle::from_computed(&style);
            font = format!(
                "{} {} 16px {}",
                fragment.font_style.as_deref().unwrap_or("normal"),
                fragment.font_weight.as_deref().unwrap_or("normal"),
                fragment.font_family.as_deref().unwrap_or("serif")
            );
            if node.tag_name().as_deref() == Some("input") {
                let text = node
                    .get_attribute("value")
                    .or_else(|| node.get_attribute("placeholder"))
                    .unwrap_or_default();
                samples
                    .entry(font.clone())
                    .or_default()
                    .extend(text.chars());
            }
        } else if node.node_type() == NodeType::Text && !font.is_empty() {
            samples
                .entry(font.clone())
                .or_default()
                .extend(node.data().unwrap_or_default().chars());
        }
        for child in node.child_nodes() {
            stack.push((child, font.clone()));
        }
    }
    samples
        .into_iter()
        .map(|(font, characters)| {
            serde_json::json!({
                "font":font,"text":characters.into_iter().collect::<String>()
            })
        })
        .collect()
}

pub(super) fn queue_task(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let callback = args.first().cloned().unwrap_or_default();
    if !callback.is_callable() {
        return Err(error("font task requires a callback"));
    }
    let payload = bind_timer_payload_to_current_realm(
        context,
        TimerPayload::Callback {
            callback,
            args: Vec::new(),
        },
    );
    with_host_state(|host| {
        let mut state = host.borrow_mut();
        let id = state.csp_document_for_context(context)?.identity();
        state.event_loop.enqueue_font_loading(payload, id);
        Ok(JsValue::undefined())
    })
}

fn font_bytes(
    state: &mut HostState,
    document: &NodeHandle,
    source: &str,
    base: Option<&crate::http::Url>,
) -> JsResult<Vec<u8>> {
    let reference = if source.starts_with("data:") {
        source.to_owned()
    } else if let Ok(url) = source.parse::<crate::http::Url>() {
        url.to_string()
    } else {
        crate::http::url::resolve_url(base.ok_or_else(|| error("font URL has no base"))?, source)
            .map_err(error)?
            .to_string()
    };
    let policy = state.csp_policy_for_document(document);
    if !policy.allows_reference(ResourceType::Font, &reference) {
        state.record_csp_violation(document, ResourceType::Font, &reference);
        return Err(error("font blocked by font-src"));
    }
    let bytes = if reference.starts_with("data:") {
        crate::http::parse_data_uri(&reference)
            .ok_or_else(|| error("invalid font data URL"))?
            .data
    } else {
        let mut url = reference.parse::<crate::http::Url>().map_err(error)?;
        let origin = state
            .base_url_for_document(document.identity())
            .as_ref()
            .map(CorsOrigin::from_url)
            .unwrap_or_else(CorsOrigin::opaque);
        // Check font-src before each network hop, rather than rejecting only
        // after a forbidden redirect destination has already been requested.
        let mut redirects = 0;
        let response = loop {
            if !policy.allows_url_after_redirects(ResourceType::Font, &url, redirects) {
                state.record_csp_violation(document, ResourceType::Font, url.to_string());
                return Err(error("font redirect blocked by font-src"));
            }
            let fetched = crate::http::cors::fetch(
                &mut state.http_client,
                HttpRequest::new(Method::Get, url.clone()),
                &origin,
                RequestMode::Cors,
                CredentialsMode::SameOrigin,
                RedirectMode::Manual,
                &mut state.cors_preflight_cache,
            )
            .map_err(error)?;
            let response = fetched.response;
            if !matches!(response.status_code(), 301 | 302 | 303 | 307 | 308) {
                break response;
            }
            if redirects == 20 {
                return Err(error("too many font redirects"));
            }
            url = crate::http::url::resolve_url(
                &url,
                response
                    .header("location")
                    .ok_or_else(|| error("font redirect has no location"))?,
            )
            .map_err(error)?;
            redirects += 1;
        };
        if !(200..300).contains(&response.status_code()) {
            return Err(error("font HTTP request failed"));
        }
        response.body().to_vec()
    };
    if bytes.len() > 10_000_000 {
        return Err(error("font exceeds the decoded font size limit"));
    }
    Ok(bytes)
}

fn load_face(state: &mut HostState, document: &NodeHandle, id: u64) -> JsResult<()> {
    if let Some(failure) = state
        .font_loading
        .faces
        .get(&id)
        .and_then(|face| face.failure.as_ref())
    {
        return Err(error(failure));
    }
    let result = load_face_data(state, document, id);
    if let Err(failure) = &result
        && let Some(face) = state.font_loading.faces.get_mut(&id)
    {
        face.failure = Some(failure.to_string());
        face.bytes = None;
    }
    result
}

fn load_face_data(state: &mut HostState, document: &NodeHandle, id: u64) -> JsResult<()> {
    let face = state
        .font_loading
        .faces
        .get(&id)
        .ok_or_else(|| error("unknown font"))?;
    if face.font.is_some() {
        return Ok(());
    }
    let bytes = face.bytes.clone();
    let sources = face.sources.clone();
    let base = face.base.clone();
    let font = if let Some(bytes) = bytes {
        Font::load_from_bytes(bytes).map_err(error)?
    } else {
        let mut loaded = None;
        for (kind, source) in sources {
            let candidate = if kind == "local" {
                crate::font::load_system_font(&source).map_err(error)
            } else {
                font_bytes(state, document, &source, base.as_ref())
                    .and_then(|bytes| Font::load_from_bytes(bytes).map_err(error))
            };
            if let Ok(font) = candidate {
                loaded = Some(font);
                break;
            }
        }
        loaded.ok_or_else(|| error("no font source could be loaded"))?
    };
    let face = state.font_loading.faces.get_mut(&id).unwrap();
    face.bytes = None;
    face.font = Some(Arc::new(font));
    Ok(())
}

/// The bootstrap retains this binding privately; it never accepts a page-supplied
/// document identity for policy checks or for registration in another document.
pub(super) fn native(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let operation = string_argument(args.first(), "", context)?;
    let payload = string_argument(args.get(1), "{}", context)?;
    let payload: serde_json::Value = serde_json::from_str(&payload).map_err(error)?;
    if operation == "parse-source" || operation == "parse-font" {
        let input = payload["input"].as_str().unwrap_or_default();
        let result = if operation == "parse-source" {
            serde_json::to_value(parse_sources(input)?).map_err(error)?
        } else {
            parse_font_query(input)?
        };
        return Ok(js_string!(result.to_string().as_str()).into());
    }
    let bytes = body_bytes_argument(args.get(2), context)?;
    with_host_state(|host| {
        let mut state = host.borrow_mut();
        let document = state.csp_document_for_context(context)?;
        let document_id = document.identity();
        if operation == "used" {
            let samples = used_fonts(&mut state, &document);
            return Ok(js_string!(serde_json::to_string(&samples).map_err(error)?.as_str()).into());
        }
        if operation == "css" {
            state.ensure_style_resolver(&document);
            let result: Vec<_> = state
                .font_loading
                .css
                .get(&document_id)
                .into_iter()
                .flatten()
                .filter_map(|(_, id)| {
                    let face = state.font_loading.faces.get(id)?;
                    Some(serde_json::json!({
                        "id": id, "family": face.family,
                        "weight": face.weight.0.to_string(),
                        "style": match face.style {
                            FontStyle::Normal => "normal",
                            FontStyle::Italic => "italic",
                            FontStyle::Oblique => "oblique",
                        },
                        "loaded": face.font.is_some(),
                    }))
                })
                .collect();
            return Ok(js_string!(serde_json::to_string(&result).map_err(error)?.as_str()).into());
        }
        if operation == "create" {
            state.font_loading.next_id += 1;
            let id = state.font_loading.next_id;
            let sources = serde_json::from_value(payload["sources"].clone()).map_err(error)?;
            let base = crate::paint::stylesheet::extract_document_base_url(
                &document,
                state.base_url_for_document(document_id).as_ref(),
            );
            if bytes.as_ref().is_some_and(|bytes| bytes.len() > 10_000_000) {
                return Err(error("font exceeds the decoded font size limit"));
            }
            state.font_loading.faces.insert(
                id,
                Face {
                    owner: document_id,
                    family: payload["family"].as_str().unwrap_or_default().to_owned(),
                    weight: FontWeight::parse(payload["weight"].as_str().unwrap_or("normal")),
                    style: FontStyle::parse(payload["style"].as_str().unwrap_or("normal")),
                    sources,
                    base,
                    bytes,
                    font: None,
                    failure: None,
                    documents: HashSet::new(),
                },
            );
            return Ok(JsValue::from(id as f64));
        }
        let id = payload["id"].as_u64().unwrap_or(0);
        if operation != "clear" {
            let face = state
                .font_loading
                .faces
                .get(&id)
                .ok_or_else(|| error("unknown font"))?;
            if face.owner != document_id
                && (state
                    .document_origins
                    .get(&face.owner)
                    .and_then(Option::as_ref)
                    .is_none()
                    || state.document_origins.get(&face.owner)
                        != state.document_origins.get(&document_id))
            {
                return Err(error("font belongs to a different origin"));
            }
        }
        match operation.as_str() {
            "load" => {
                let owner_id = state.font_loading.faces[&id].owner;
                let owner = state
                    .get_node(owner_id)
                    .ok_or_else(|| error("font document is no longer live"))?;
                load_face(&mut state, &owner, id)?;
            }
            "add" => {
                let members = state.font_loading.members.entry(document_id).or_default();
                if !members.contains(&id) {
                    members.push(id);
                }
                state
                    .font_loading
                    .faces
                    .get_mut(&id)
                    .unwrap()
                    .documents
                    .insert(document_id);
            }
            "delete" => {
                if let Some(members) = state.font_loading.members.get_mut(&document_id) {
                    members.retain(|member| *member != id);
                }
                state
                    .font_loading
                    .faces
                    .get_mut(&id)
                    .unwrap()
                    .documents
                    .remove(&document_id);
            }
            "clear" => {
                state.font_loading.members.remove(&document_id);
            }
            "update" => {
                let face = state.font_loading.faces.get_mut(&id).unwrap();
                face.family = payload["family"].as_str().unwrap_or_default().to_owned();
                face.weight = FontWeight::parse(payload["weight"].as_str().unwrap_or("normal"));
                face.style = FontStyle::parse(payload["style"].as_str().unwrap_or("normal"));
            }
            _ => return Err(error("unknown font operation")),
        }
        if matches!(operation.as_str(), "load" | "update") {
            let members: Vec<_> = state.font_loading.faces[&id]
                .documents
                .iter()
                .copied()
                .collect();
            for member in members {
                if let Some(document) = state.get_node(member) {
                    state.mark_document_style_dirty(&document);
                }
            }
        }
        state.mark_document_style_dirty(&document);
        Ok(JsValue::undefined())
    })
}
