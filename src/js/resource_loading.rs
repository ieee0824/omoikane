//! Embedder-owned resource scheduling shared by document resource loaders.
use super::{HostState, JsRuntime};
use crate::resource_worker::ResourceQueue;

impl JsRuntime {
    /// Runs Fetch and asynchronous XHR transport on the shared resource workers.
    /// Completion callbacks stay in the owning runtime and settle through tasks.
    pub fn set_nonblocking_fetch_loading(&mut self, enabled: bool) {
        self.host_state.borrow_mut().nonblocking_fetch_loading = enabled;
    }

    /// Fetches dynamic external scripts on the shared resource workers.
    /// Script execution and completion events remain on the owning runtime.
    pub fn set_nonblocking_script_loading(&mut self, enabled: bool) {
        self.host_state.borrow_mut().nonblocking_script_loading = enabled;
    }

    /// Fetches and decodes fonts on the document's shared resource workers.
    /// Completion is applied when the host pumps page tasks; the initial paint
    /// can use fallback fonts without waiting for transport or decoding.
    pub fn set_nonblocking_font_loading(&mut self, enabled: bool) {
        self.host_state.borrow_mut().nonblocking_font_loading = enabled;
    }

    /// Uses the supplied worker queue for owned resource I/O and decoding.
    /// Cloning one queue across runtimes shares its concurrency limit. Configure
    /// it before starting resource loads; replacing it does not abort active I/O.
    pub fn set_resource_queue(&mut self, queue: ResourceQueue) {
        self.host_state.borrow_mut().resource_queue = queue;
    }
}

impl HostState {
    /// Copies a creator's scheduling policy and shares its resource concurrency budget.
    pub(super) fn inherit_resource_loading(&mut self, owner: &HostState) {
        self.resource_queue = owner.resource_queue.clone();
        self.nonblocking_image_loading = owner.nonblocking_image_loading;
        self.nonblocking_font_loading = owner.nonblocking_font_loading;
        self.nonblocking_script_loading = owner.nonblocking_script_loading;
        self.nonblocking_fetch_loading = owner.nonblocking_fetch_loading;
    }
}
