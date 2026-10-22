//! Observable iframe policy derived only from its owner and current attributes.

use super::{
    DocumentPolicy, DocumentSecurityOrigin, HostState, IframeSandboxPolicy, JsNativeError,
    JsResult, NodeHandle, PolicyFeature, StorageOrigin, owner_document_for_node,
};
use std::collections::HashSet;

/// An owned, short-lived prediction; it never reads the loaded child Document.
pub(super) struct FramePolicy {
    pub(super) origin: DocumentSecurityOrigin,
    inherited: HashSet<PolicyFeature>,
}

impl FramePolicy {
    pub(super) fn query(
        &self,
        operation: &str,
        feature: Option<PolicyFeature>,
        target: Option<&DocumentSecurityOrigin>,
    ) -> JsResult<serde_json::Value> {
        Ok(match operation {
            "features" => serde_json::json!(PolicyFeature::ALL.map(PolicyFeature::name)),
            "allowedFeatures" => serde_json::json!(
                PolicyFeature::ALL
                    .into_iter()
                    .filter(|feature| self.allows(*feature, &self.origin))
                    .map(PolicyFeature::name)
                    .collect::<Vec<_>>()
            ),
            "allowsFeature" => serde_json::json!(
                feature
                    .zip(target)
                    .is_some_and(|(feature, target)| self.allows(feature, target))
            ),
            "getAllowlistForFeature" => serde_json::json!(
                feature
                    .map(|feature| self.allowlist(feature))
                    .unwrap_or_default()
            ),
            _ => {
                return Err(JsNativeError::typ()
                    .with_message("Unknown PermissionsPolicy operation")
                    .into());
            }
        })
    }

    pub(super) fn allows(&self, feature: PolicyFeature, origin: &DocumentSecurityOrigin) -> bool {
        self.inherited.contains(&feature)
            && DocumentPolicy::default().allows(feature, origin, &self.origin)
    }

    pub(super) fn allowlist(&self, feature: PolicyFeature) -> Vec<String> {
        if self.allows(feature, &self.origin) {
            DocumentPolicy::default().allowlist(feature, &self.origin)
        } else {
            Vec::new()
        }
    }
}

impl HostState {
    pub(super) fn frame_policy(&self, frame: &NodeHandle) -> JsResult<FramePolicy> {
        let owner = owner_document_for_node(frame)
            .or_else(|| self.node_lifetime_owner(frame.identity()))
            .ok_or_else(|| {
                JsNativeError::typ().with_message("PermissionsPolicy frame has no owner Document")
            })?;
        let owner_id = owner.identity();
        if !frame.has_tag_name("iframe") || !self.document_is_active(owner_id) {
            return Err(JsNativeError::typ()
                .with_message("PermissionsPolicy owner is not an active iframe")
                .into());
        }
        let origin = self.frame_policy_origin(frame, &owner)?;
        let inherited =
            self.inherited_permissions_for_iframe(frame, owner_id, &origin, Some(&origin));
        Ok(FramePolicy { origin, inherited })
    }

    fn frame_policy_origin(
        &self,
        frame: &NodeHandle,
        owner: &NodeHandle,
    ) -> JsResult<DocumentSecurityOrigin> {
        // Real Document identities start at 1. Zero is only a temporary opaque
        // prediction, never installed in Document state or compared across frames.
        let opaque = DocumentSecurityOrigin::Opaque(0);
        let owner_sandbox = self.sandbox_policy_for_document(owner);
        let frame_sandbox = IframeSandboxPolicy::from_iframe(frame);
        if (owner_sandbox.active && !owner_sandbox.allow_same_origin)
            || (frame_sandbox.active && !frame_sandbox.allow_same_origin)
        {
            return Ok(opaque);
        }
        let owner_origin = self
            .document_security_origins
            .get(&owner.identity())
            .cloned()
            .ok_or_else(|| {
                JsNativeError::typ().with_message("Frame owner origin is unavailable")
            })?;
        if frame.has_attribute("srcdoc") {
            return Ok(owner_origin);
        }
        let Some(src) = frame.get_attribute("src") else {
            return Ok(owner_origin);
        };
        let base = self
            .base_url_for_document(owner.identity())
            .and_then(|base| url::Url::parse(&base.to_string()).ok());
        let Ok(url) = url::Url::options().base_url(base.as_ref()).parse(&src) else {
            return Ok(owner_origin);
        };
        Ok(StorageOrigin::from_url(&url.origin().ascii_serialization())
            .map(DocumentSecurityOrigin::Tuple)
            .unwrap_or(opaque))
    }
}
