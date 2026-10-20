//! Embedder-owned resource scheduling shared by document resource loaders.
use super::JsRuntime;
use crate::resource_worker::ResourceQueue;

impl JsRuntime {
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
