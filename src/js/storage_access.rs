//! Document Storage Access decisions shared by the API and DOM cookie sink.

use super::*;

#[derive(Clone, Copy, PartialEq, Eq)]
enum StorageAccessStatus {
    Granted,
    Blocked,
    Inactive,
}

impl StorageAccessStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Granted => "granted",
            Self::Blocked => "blocked",
            Self::Inactive => "inactive",
        }
    }
}

struct StorageAccessContext {
    top_level_site: crate::http::SchemefulSite,
    embedded_site: crate::http::SchemefulSite,
    cookie_site: crate::http::Url,
    same_site: bool,
    sandbox_allows_request: bool,
}

#[derive(Clone, Copy)]
pub(super) enum StorageAccessPermission {
    Prompt,
    Granted,
    Denied,
}

struct TrustedStorageAccessInputGuard {
    host: Rc<RefCell<HostState>>,
    previous: bool,
}

impl Drop for TrustedStorageAccessInputGuard {
    fn drop(&mut self) {
        self.host.borrow_mut().trusted_storage_access_input = self.previous;
    }
}

impl JsRuntime {
    /// Marks synchronous input dispatch and its microtasks as trusted by the
    /// embedder. Page-exposed dispatch helpers cannot set this native flag.
    pub(crate) fn with_trusted_storage_access_input<T>(
        &mut self,
        dispatch: impl FnOnce(&mut Self) -> JsResult<T>,
    ) -> JsResult<T> {
        let previous = std::mem::replace(
            &mut self.host_state.borrow_mut().trusted_storage_access_input,
            true,
        );
        let _guard = TrustedStorageAccessInputGuard {
            host: Rc::clone(&self.host_state),
            previous,
        };
        dispatch(self)
    }
}

impl HostState {
    pub(super) fn set_storage_access_permission(
        &mut self,
        document_id: usize,
        permission: StorageAccessPermission,
    ) -> JsResult<()> {
        let context = self
            .storage_access_context(document_id)
            .ok_or_else(|| JsNativeError::typ().with_message("No active storage-access context"))?;
        let key = (context.top_level_site, context.embedded_site);
        self.storage_access_grants.remove(&key);
        self.storage_access_denials.remove(&key);
        match permission {
            StorageAccessPermission::Prompt => {}
            StorageAccessPermission::Granted => {
                self.storage_access_grants.insert(key);
            }
            StorageAccessPermission::Denied => {
                self.storage_access_denials.insert(key);
            }
        }
        Ok(())
    }

    /// Resolves every iframe ancestor, so an A → B → A frame stays third-party.
    fn storage_access_context(&self, document_id: usize) -> Option<StorageAccessContext> {
        if !self.document_is_active(document_id) {
            return None;
        }
        let embedded_site = self
            .document_origins
            .get(&document_id)?
            .as_ref()?
            .schemeful_site();
        let sandbox_allows_request = self
            .document_sandbox
            .get(&document_id)
            .is_none_or(|policy| policy.allow_storage_access_by_user_activation);
        let mut current = document_id;
        let mut same_site = true;
        let mut cross_site_ancestor = None;
        for _ in 0..=self.iframe_documents.len() {
            let Some((&iframe_id, _)) = self
                .iframe_documents
                .iter()
                .find(|(_, entry)| entry.document.identity() == current)
            else {
                let top_level_origin = self.document_origins.get(&current)?.as_ref()?;
                let top_level_site = top_level_origin.schemeful_site();
                let cookie_site =
                    cross_site_ancestor.or_else(|| top_level_origin.serialize().parse().ok())?;
                return Some(StorageAccessContext {
                    top_level_site,
                    embedded_site,
                    cookie_site,
                    same_site,
                    sandbox_allows_request,
                });
            };
            let iframe = self.get_node(iframe_id)?;
            let parent = owner_document_for_node(&iframe)?;
            let parent_id = parent.identity();
            let parent_origin = self.document_origins.get(&parent_id)?.as_ref()?;
            let parent_site = parent_origin.schemeful_site();
            if parent_site != embedded_site && cross_site_ancestor.is_none() {
                cross_site_ancestor = parent_origin.serialize().parse().ok();
            }
            same_site &= parent_site == embedded_site;
            current = parent_id;
        }
        None
    }

    fn storage_access_status(&self, document_id: usize) -> StorageAccessStatus {
        if !self.document_is_active(document_id) {
            return StorageAccessStatus::Inactive;
        }
        if !document_is_secure_context(self, document_id) {
            return StorageAccessStatus::Blocked;
        }
        let Some(context) = self.storage_access_context(document_id) else {
            return StorageAccessStatus::Blocked;
        };
        if context.same_site {
            return StorageAccessStatus::Granted;
        }
        if self.storage_access_active_documents.contains(&document_id)
            && self
                .storage_access_grants
                .contains(&(context.top_level_site, context.embedded_site))
        {
            StorageAccessStatus::Granted
        } else {
            StorageAccessStatus::Blocked
        }
    }

    /// A third-party Document cannot read or write unpartitioned cookies until
    /// this Document activates its top-level-site/embedded-site permission.
    pub(super) fn storage_access_allows_document_cookies(&self, document_id: usize) -> bool {
        let Some(context) = self.storage_access_context(document_id) else {
            return false;
        };
        context.same_site
            || (self.feature_enabled_in_document(document_id, PolicyFeature::StorageAccess)
                && self.storage_access_status(document_id) == StorageAccessStatus::Granted)
    }

    /// Chooses a cross-site ancestor as the cookie context when one exists.
    /// In A → B → A nesting the top-level A alone would make SameSite cookies
    /// appear first-party, even though the inner Document is third-party.
    pub(super) fn storage_access_cookie_site_for_document(
        &self,
        document_id: usize,
        fallback: &crate::http::Url,
    ) -> crate::http::Url {
        self.storage_access_context(document_id)
            .map_or_else(|| fallback.clone(), |context| context.cookie_site)
    }

    /// Partitions shared worker identity by the active top-level site.
    pub(super) fn storage_access_top_level_site(
        &self,
        document_id: usize,
    ) -> Option<crate::http::SchemefulSite> {
        self.storage_access_context(document_id)
            .map(|context| context.top_level_site)
    }

    /// Captures policy once so asynchronous resource loaders do not retain DOM state.
    pub(super) fn document_cookie_request_context(
        &self,
        document_id: usize,
    ) -> crate::http::CookieRequestContext {
        if document_id == self.document.identity() {
            if let Some(context) = &self.worker_cookie_request_context {
                return context.clone();
            }
        }
        let context = self.storage_access_context(document_id);
        // Standalone runtimes can install the main page URL through
        // `set_base_url` without navigating. Resource requests already use
        // that URL, so their cookie site must follow it as well.
        let main_document = document_id == self.document.identity();
        let site = if main_document {
            self.base_url
                .clone()
                .or_else(|| context.as_ref().map(|context| context.cookie_site.clone()))
        } else {
            context.as_ref().map(|context| context.cookie_site.clone())
        };
        let cross_site_ancestry =
            !main_document && context.as_ref().is_none_or(|context| !context.same_site);
        let granted_origin = ((!cross_site_ancestry
            || self.feature_enabled_in_document(document_id, PolicyFeature::StorageAccess))
            && self.storage_access_status(document_id) == StorageAccessStatus::Granted)
            .then(|| {
                self.document_origins
                    .get(&document_id)
                    .and_then(Option::as_ref)
            })
            .flatten()
            .and_then(|origin| origin.serialize().parse().ok());
        crate::http::CookieRequestContext::new(site, granted_origin, cross_site_ancestry)
    }

    /// Shares the creator's browsing session with a Worker, without inheriting
    /// a Document-only Storage Access grant for later Worker requests.
    pub(super) fn inherit_worker_cookie_session(
        &mut self,
        owner: &HostState,
        owner_document_id: usize,
    ) {
        self.worker_cookie_request_context = Some(
            owner
                .document_cookie_request_context(owner_document_id)
                .for_worker_internal(),
        );
        self.cookie_store = Arc::clone(&owner.cookie_store);
        self.cookie_change_sequence = self.cookie_store.lock().unwrap().change_sequence();
        self.http_client = owner.http_client.fork_for_worker();
        self.http_client
            .set_shared_cookie_store(Arc::clone(&self.cookie_store));
    }

    /// Downgrades a shared Worker and all existing Dedicated Worker descendants
    /// when an owner with cross-site ancestry joins its session.
    pub(super) fn restrict_worker_cookie_context(&mut self) {
        let mut pending = self.restrict_one_worker_cookie_context();
        while let Some(child) = pending.pop() {
            pending.extend(child.borrow_mut().restrict_one_worker_cookie_context());
        }
    }

    fn restrict_one_worker_cookie_context(&mut self) -> Vec<Rc<RefCell<HostState>>> {
        if let Some(context) = self.worker_cookie_request_context.take() {
            self.worker_cookie_request_context = Some(context.with_cross_site_ancestry());
        }
        self.workers
            .values()
            .map(|entry| Rc::clone(&entry.borrow().runtime.host_state))
            .collect()
    }

    /// Applies a Document's cookie context to an outgoing subresource request.
    pub(super) fn set_document_cookie_request_context(
        &self,
        document_id: usize,
        request: &mut HttpRequest,
    ) {
        self.document_cookie_request_context(document_id)
            .apply(request);
    }

    fn request_storage_access(
        &mut self,
        document_id: usize,
        user_activation: bool,
    ) -> StorageAccessStatus {
        if !self.document_is_active(document_id) {
            return StorageAccessStatus::Inactive;
        }
        if !document_is_secure_context(self, document_id) {
            return StorageAccessStatus::Blocked;
        }
        if !self.feature_enabled_in_document(document_id, PolicyFeature::StorageAccess) {
            return StorageAccessStatus::Blocked;
        }
        let Some(context) = self.storage_access_context(document_id) else {
            return StorageAccessStatus::Blocked;
        };
        if !context.sandbox_allows_request {
            return StorageAccessStatus::Blocked;
        }
        if context.same_site {
            return StorageAccessStatus::Granted;
        }
        let key = (context.top_level_site, context.embedded_site);
        // A → B → A is still blocked until this nested Document opts in, but
        // its site matches the top-level site so no user gesture is required.
        if key.0 == key.1 {
            self.storage_access_grants.insert(key);
            self.storage_access_active_documents.insert(document_id);
            return StorageAccessStatus::Granted;
        }
        if self.storage_access_grants.contains(&key) {
            self.storage_access_active_documents.insert(document_id);
            return StorageAccessStatus::Granted;
        }
        if self.storage_access_denials.contains(&key) {
            return StorageAccessStatus::Blocked;
        }
        if !user_activation {
            return StorageAccessStatus::Blocked;
        }
        self.storage_access_grants.insert(key);
        self.storage_access_active_documents.insert(document_id);
        StorageAccessStatus::Granted
    }
}

pub(super) fn storage_access_state_native(
    _this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let document_id = parse_node_id(args.first(), context)?;
    let active = with_host_state(|host| Ok(host.borrow().document_is_active(document_id)))?;
    if !active {
        return Ok(JsValue::from(js_string!(
            StorageAccessStatus::Inactive.as_str()
        )));
    }
    ensure_same_origin_document(context, document_id)?;
    with_host_state(|host| {
        let status = host.borrow().storage_access_status(document_id);
        Ok(JsValue::from(js_string!(status.as_str())))
    })
}

/// Storage Access deliberately hides a denied permission as prompt.
pub(super) fn permission_state_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let document = parse_node_id(args.first(), context)?;
    ensure_same_origin_document(context, document)?;
    let granted = with_host_state(|host| {
        let state = host.borrow();
        Ok(state.storage_access_context(document).is_some_and(|site| {
            document_is_secure_context(&state, document)
                && state.feature_enabled_in_document(document, PolicyFeature::StorageAccess)
                && state
                    .storage_access_grants
                    .contains(&(site.top_level_site, site.embedded_site))
        }))
    })?;
    Ok(js_string!(if granted { "granted" } else { "prompt" }).into())
}

/// Recomputes live PermissionStatus objects in each active Window Realm.
pub(super) fn notify_permission_changes(context: &mut Context) {
    let Ok(realms) = with_host_state(|host| {
        let state = host.borrow();
        let mut realms = Vec::new();
        if let Some(realm) = &state.main_realm {
            realms.push(realm.clone());
        }
        realms.extend(
            state
                .iframe_documents
                .values()
                .filter_map(|entry| entry.realm.clone()),
        );
        realms.extend(
            state
                .auxiliary_contexts
                .values()
                .filter_map(|entry| entry.realm.clone()),
        );
        Ok(realms)
    }) else {
        return;
    };
    for realm in realms {
        let previous = context.enter_realm(realm);
        let _ = (|| -> JsResult<()> {
            let callback = context
                .global_object()
                .get(js_string!("__omoikane_permission_changed"), context)?;
            if let Some(callable) = callback.as_callable() {
                callable.call(
                    &JsValue::undefined(),
                    &[js_string!("storage-access").into()],
                    context,
                )?;
            }
            Ok(())
        })();
        context.enter_realm(previous);
    }
}

pub(super) fn storage_access_request_native(
    _this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let document_id = parse_node_id(args.first(), context)?;
    let active = with_host_state(|host| Ok(host.borrow().document_is_active(document_id)))?;
    if !active {
        return Ok(JsValue::from(js_string!(
            StorageAccessStatus::Inactive.as_str()
        )));
    }
    ensure_same_origin_document(context, document_id)?;
    let dispatcher_activation = args.get(1).is_some_and(JsValue::to_boolean);
    let result = with_host_state(|host| {
        let user_activation = dispatcher_activation && host.borrow().trusted_storage_access_input;
        let status = host
            .borrow_mut()
            .request_storage_access(document_id, user_activation);
        Ok(JsValue::from(js_string!(status.as_str())))
    })?;
    notify_permission_changes(context);
    Ok(result)
}
