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
    origin: StorageOrigin,
    same_site: bool,
    sandbox_allows_request: bool,
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
    /// Resolves every iframe ancestor, so an A → B → A frame stays third-party.
    fn storage_access_context(&self, document_id: usize) -> Option<StorageAccessContext> {
        if !self.document_is_active(document_id) {
            return None;
        }
        let origin = self.document_origins.get(&document_id)?.as_ref()?.clone();
        let origin_site = origin.schemeful_site();
        let sandbox_allows_request = self
            .document_sandbox
            .get(&document_id)
            .is_none_or(|policy| policy.allow_storage_access_by_user_activation);
        let mut current = document_id;
        let mut same_site = true;
        for _ in 0..=self.iframe_documents.len() {
            let Some((&iframe_id, _)) = self
                .iframe_documents
                .iter()
                .find(|(_, entry)| entry.document.identity() == current)
            else {
                let top_level_site = self
                    .document_origins
                    .get(&current)?
                    .as_ref()?
                    .schemeful_site();
                return Some(StorageAccessContext {
                    top_level_site,
                    origin,
                    same_site,
                    sandbox_allows_request,
                });
            };
            let iframe = self.get_node(iframe_id)?;
            let parent = owner_document_for_node(&iframe)?;
            let parent_id = parent.identity();
            let parent_site = self
                .document_origins
                .get(&parent_id)?
                .as_ref()?
                .schemeful_site();
            same_site &= parent_site == origin_site;
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
        if context.sandbox_allows_request
            && self
                .storage_access_grants
                .contains(&(context.top_level_site, context.origin))
        {
            StorageAccessStatus::Granted
        } else {
            StorageAccessStatus::Blocked
        }
    }

    /// A third-party Document cannot read or write unpartitioned DOM cookies
    /// until its top-level-site/origin pair has an active Storage Access grant.
    pub(super) fn storage_access_allows_document_cookies(&self, document_id: usize) -> bool {
        let Some(context) = self.storage_access_context(document_id) else {
            return false;
        };
        context.same_site || self.storage_access_status(document_id) == StorageAccessStatus::Granted
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
        let Some(context) = self.storage_access_context(document_id) else {
            return StorageAccessStatus::Blocked;
        };
        if !context.sandbox_allows_request {
            return StorageAccessStatus::Blocked;
        }
        if context.same_site {
            return StorageAccessStatus::Granted;
        }
        let key = (context.top_level_site, context.origin);
        if self.storage_access_grants.contains(&key) {
            return StorageAccessStatus::Granted;
        }
        if !user_activation {
            return StorageAccessStatus::Blocked;
        }
        self.storage_access_grants.insert(key);
        StorageAccessStatus::Granted
    }
}

pub(super) fn storage_access_state_native(
    _this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let document_id = parse_node_id(args.first(), context)?;
    ensure_same_origin_document(context, document_id)?;
    with_host_state(|host| {
        let status = host.borrow().storage_access_status(document_id);
        Ok(JsValue::from(js_string!(status.as_str())))
    })
}

pub(super) fn storage_access_request_native(
    _this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let document_id = parse_node_id(args.first(), context)?;
    ensure_same_origin_document(context, document_id)?;
    let dispatcher_activation = args.get(1).is_some_and(JsValue::to_boolean);
    with_host_state(|host| {
        let user_activation = dispatcher_activation && host.borrow().trusted_storage_access_input;
        let status = host
            .borrow_mut()
            .request_storage_access(document_id, user_activation);
        Ok(JsValue::from(js_string!(status.as_str())))
    })
}
