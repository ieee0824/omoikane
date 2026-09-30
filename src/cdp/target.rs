//! `Target` domain: browser context creation and disposal.

use super::*;

impl CdpSession {
    pub(super) fn target_create_browser_context(&mut self) -> Value {
        let browser_context_id = format!("context-{}", self.next_browser_context_id);
        self.next_browser_context_id += 1;
        self.browser_context_ids.push(browser_context_id.clone());
        json!({ "browserContextId": browser_context_id })
    }

    pub(super) fn target_get_browser_contexts(&self) -> Value {
        json!({ "browserContextIds": self.browser_context_ids })
    }

    pub(super) fn target_dispose_browser_context(
        &mut self,
        params: &Value,
    ) -> Result<Value, JsonRpcError> {
        let browser_context_id = require_string(params, "browserContextId")?;
        let original_len = self.browser_context_ids.len();
        self.browser_context_ids
            .retain(|current| current != &browser_context_id);

        if self.browser_context_ids.len() == original_len {
            return Err(JsonRpcError {
                code: -32000,
                message: format!("Unknown browser context: {browser_context_id}"),
            });
        }

        Ok(json!({}))
    }
}
