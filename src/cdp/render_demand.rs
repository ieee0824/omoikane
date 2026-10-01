//! Read-only rendering requirements exposed to native hosts.

use super::CdpSession;
use crate::js::render_demand::RuntimePaintKey;
use std::time::Duration;

/// When a host should provide the next page rendering opportunity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NextRendering {
    /// Animation or background work requires one opportunity each frame.
    EveryFrame,
    /// Offer an opportunity after this remaining page-time delay.
    After(Duration),
    /// Only external input, resize or navigation can create work.
    Idle,
}

/// An owned snapshot of the inputs that affect the presented page pixels.
///
/// Includes document generation; style, layout, scroll and paint generations;
/// layout and visual viewports; scrolling, focus and selection state; and host
/// visibility, shared visited-history generation and visible animated-image
/// frame indices. DOM, hover,
/// text-control caret/value and child-document changes
/// invalidate the included generations. Hosts capture this after a successful
/// paint and retain it without borrowing the session.
#[derive(Debug, Clone, PartialEq)]
pub struct PaintStateKey {
    document_generation: u64,
    runtime: RuntimePaintKey,
    hidden: bool,
    frozen: bool,
}

/// The page's paint and scheduling needs after a rendering opportunity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RenderDemand {
    /// The supplied last-presented key differs from the current paint inputs.
    pub needs_paint: bool,
    /// The next opportunity required by the page, independent of painting.
    pub next: NextRendering,
}

impl CdpSession {
    /// Returns the monotonic generation of the active document. Native hosts
    /// use this to rebase their clock after a navigation installs a new runtime.
    pub fn document_generation(&self) -> u64 {
        self.document_generation
    }

    /// Advances page, worker and worklet clocks and runs due tasks without
    /// delivering rAF callbacks. Native hosts call this before dispatching input
    /// after sleeping; script timers then use the input's actual page time.
    /// Task-triggered navigation is committed before returning.
    pub fn advance_tasks(&mut self, elapsed_ms: u64) -> Result<(), super::JsonRpcError> {
        self.runtime.tick(elapsed_ms).map_err(super::js_error)?;
        self.drive_navigation_requests()
    }

    /// Captures current native paint inputs without running JavaScript,
    /// flushing styles, reading a clock, fetching resources or changing state.
    /// Capture after painting, since painting can refresh derived style state.
    pub fn paint_state_key(&self) -> PaintStateKey {
        PaintStateKey {
            document_generation: self.document_generation,
            runtime: self.runtime.paint_state_key(),
            hidden: self.host_hidden,
            frozen: self.lifecycle_frozen,
        }
    }

    /// Queries current render needs without running jobs or mutating caches.
    /// `last_presented` must be the key captured after the last successful paint;
    /// `None` requests an initial paint. Delays use the page's explicit virtual
    /// clock, so hosts must account for elapsed real time when scheduling them.
    pub fn render_demand(&self, last_presented: Option<&PaintStateKey>) -> RenderDemand {
        RenderDemand {
            needs_paint: last_presented != Some(&self.paint_state_key()),
            next: self.runtime.next_rendering(),
        }
    }
}
