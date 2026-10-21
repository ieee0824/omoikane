//! Shared native state for retained, asynchronous viewport scrolling.
//!
//! [`NativeScrollHandle`] is the ownership boundary between the GUI and the
//! JavaScript/page owner. Its mutex contains owned numbers and bounded queues;
//! it never contains DOM nodes, Boa values, garbage-collected handles, layout,
//! or raster objects. Callers must release the mutex before running script,
//! layout, painting, or event delivery.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard, Weak};

const MAX_PENDING: usize = 1024;

// Diagnostic only: no protocol lock is held while emitting the bounded trace.
fn trace_policy_revocation(before: u64, after: u64, caller: &std::panic::Location<'_>) {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    static COUNT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    if !*ENABLED.get_or_init(|| std::env::var_os("OMOIKANE_TRACE_NATIVE_POLICY").is_some())
        || COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed) >= 4096
    {
        return;
    }
    crate::diagnostics::input_probe(serde_json::json!({
        "event": "policy_revoke",
        "previous_policy_epoch": before,
        "policy_epoch": after,
        "caller_file": caller.file(),
        "caller_line": caller.line(),
    }));
}

/// A finite CSS-pixel offset or delta.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Offset {
    /// Horizontal component.
    pub x: f64,
    /// Vertical component.
    pub y: f64,
}

impl Offset {
    fn finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite()
    }

    fn plus(self, other: Self) -> Self {
        Self {
            x: self.x + other.x,
            y: self.y + other.y,
        }
    }

    /// Match the canonical HostState storage boundary after each default.
    /// f32 retains subpixel wheel input while preventing GUI/owner drift from
    /// grouping several f64 deltas before a single canonical commit.
    fn canonical(self) -> Self {
        Self {
            x: f64::from(self.x as f32),
            y: f64::from(self.y as f32),
        }
    }
}

/// Inclusive valid offsets in CSS pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bounds {
    /// Smallest valid offset on each axis.
    pub min: Offset,
    /// Largest valid offset on each axis.
    pub max: Offset,
}

impl Bounds {
    fn valid(self) -> bool {
        self.min.finite()
            && self.max.finite()
            && self.min.x <= self.max.x
            && self.min.y <= self.max.y
    }

    fn contains(self, offset: Offset) -> bool {
        offset.finite()
            && offset.x >= self.min.x
            && offset.x <= self.max.x
            && offset.y >= self.min.y
            && offset.y <= self.max.y
    }

    fn clamp(self, offset: Offset) -> Offset {
        Offset {
            x: offset.x.clamp(self.min.x, self.max.x),
            y: offset.y.clamp(self.min.y, self.max.y),
        }
    }
}

/// Numeric identity of one scroll-target incarnation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TargetKey {
    /// Document generation; it must change when a Document is replaced.
    pub document: u64,
    /// Page-owner generation; it must change when an owner is restarted.
    pub owner_generation: u64,
    /// Viewport generation; it must change before resize or scale replacement.
    pub viewport_epoch: u64,
    /// Native scroll-node identity within the owner generation.
    pub scroll_node: u64,
}

/// Monotonic order of retained-scene publications.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct PublicationSequence(u64);

impl PublicationSequence {
    /// Constructs a publication sequence from its numeric protocol value.
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// Returns the numeric protocol value.
    pub const fn get(self) -> u64 {
        self.0
    }

    fn initial() -> Self {
        Self(0)
    }

    fn next(self) -> Option<Self> {
        self.0.checked_add(1).map(Self)
    }
}

/// Monotonic order of canonical fallback defaults.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct FallbackTicket(u64);

impl FallbackTicket {
    /// Constructs a fallback ticket from its numeric protocol value.
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// Returns the numeric protocol value.
    pub const fn get(self) -> u64 {
        self.0
    }

    fn next(self) -> Option<Self> {
        self.0.checked_add(1).map(Self)
    }
}

/// Owned geometry and acknowledgements accompanying a retained scene.
#[derive(Clone, Copy, Debug)]
pub struct Scene {
    /// Scroll-target incarnation that produced the scene.
    pub key: TargetKey,
    /// Publication order, independent of paint revision.
    pub publication: PublicationSequence,
    /// Native policy epoch captured before scene extraction.
    pub policy_epoch: u64,
    /// Input-acceptance generation captured before scene extraction.
    pub acceptance_epoch: u64,
    /// Latest accepted default committed by the canonical page owner.
    pub acknowledged: u64,
    /// Latest canonical fallback completed by the page owner.
    pub completed_fallback: FallbackTicket,
    /// Canonical scroll offset represented by the scene.
    pub offset: Offset,
    /// Valid canonical scroll offsets.
    pub extent: Bounds,
    /// Offsets fully covered by retained pixels.
    pub coverage: Bounds,
    /// Trusted owner policy result for this scene.
    pub eligible: bool,
}

impl Scene {
    fn valid(self) -> bool {
        self.extent.valid()
            && self.coverage.valid()
            && self.extent.contains(self.offset)
            && self.coverage.contains(self.offset)
    }
}

/// One GUI-accepted default action, retained until notification is claimed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AcceptedScroll {
    /// Target incarnation that accepted the input.
    pub key: TargetKey,
    /// FIFO input sequence within the target incarnation.
    pub sequence: u64,
    /// Whole delta accepted by the GUI.
    pub delta: Offset,
    /// Visual position immediately after accepting the delta.
    pub visual_offset: Offset,
}

impl AcceptedScroll {
    /// Returns whether `offset` is the exact canonical position immediately
    /// before this accepted default.
    ///
    /// This validates the payload in the forward direction. Callers never
    /// reconstruct the earlier position by subtracting from the post-default
    /// offset, which would lose clamp and canonical-f32 information.
    pub fn matches_pre_default_offset(self, offset: Offset) -> bool {
        offset.finite()
            && self.delta.finite()
            && self.visual_offset.finite()
            && offset.plus(self.delta).canonical() == self.visual_offset
    }
}

/// Acceptance token revalidated immediately before presenting composed pixels.
///
/// Revalidation failure never removes `input` from the owner FIFO. The owner
/// still applies its default and delivers exactly one no-default notification.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AcceptedPresentation {
    /// Immutable FIFO input payload.
    input: AcceptedScroll,
    /// Policy epoch at acceptance.
    policy_epoch: u64,
    /// Publication owning these composed pixels, updated when a newer scene
    /// rebases the presentation without changing its accepted owner input.
    publication: PublicationSequence,
    /// Exact visual position represented by the composed pixels.
    presentation_offset: Offset,
    /// Canonical visual position sampled immediately before the default.
    pre_default_offset: Offset,
}

impl AcceptedPresentation {
    /// Returns the immutable accepted input retained for owner delivery.
    pub const fn input(self) -> AcceptedScroll {
        self.input
    }

    /// Returns the atomic visual position immediately before this default.
    pub const fn pre_default_offset(self) -> Offset {
        self.pre_default_offset
    }

    /// Returns the exact visual position that retained pixels must represent.
    ///
    /// A newer scene can rebase this independently of the immutable owner FIFO
    /// input when its canonical offset changes before that input materializes.
    pub const fn visual_offset(self) -> Offset {
        self.presentation_offset
    }
}

/// Reason the native GUI path selected the canonical fallback path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FallbackReason {
    /// No retained scene is installed.
    NoScene,
    /// Native policy does not currently authorize asynchronous defaults.
    Ineligible,
    /// The supplied delta is non-finite.
    InvalidDelta,
    /// The whole delta would cross the scroll extent.
    ExtentBoundary,
    /// The whole delta would cross retained-pixel coverage.
    MissingCoverage,
    /// The bounded accepted/notification queue is full.
    PendingLimit,
    /// The accepted-input sequence cannot advance without wrapping.
    SequenceExhausted,
    /// A previous canonical fallback has not completed.
    FallbackInFlight,
    /// The fallback-ticket sequence cannot advance without wrapping.
    FallbackTicketExhausted,
}

/// Ordering state attached to a canonical fallback.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FallbackOrder {
    /// Ordered fallback ticket.
    Ticket(FallbackTicket),
    /// Ticket space is exhausted for this target incarnation.
    Exhausted,
}

/// A whole wheel delta that must run on the canonical page-owner path.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fallback {
    /// Target incarnation that issued the fallback.
    pub key: TargetKey,
    /// Ordering ticket, or permanent exhaustion.
    pub order: FallbackOrder,
    /// Why the GUI could not accept the default.
    pub reason: FallbackReason,
    /// Latest accepted notification that must be delivered first.
    pub after_sequence: Option<u64>,
    /// Whole delta for the canonical default action.
    pub delta: Offset,
}

/// Result of one GUI scroll attempt.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ScrollDecision {
    /// The GUI applied the default and queued this immutable payload.
    Accepted(AcceptedPresentation),
    /// The page owner must perform the whole default after its barrier.
    Fallback(Fallback),
}

/// Rejection from the canonical owner synchronization protocol.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OwnerRejection {
    /// The caller belongs to another target incarnation.
    WrongTarget,
    /// The input or notification was already consumed.
    Duplicate,
    /// An input sequence skipped its predecessor.
    SequenceGap,
    /// Earlier notifications still block this operation.
    BarrierPending,
    /// The fallback ticket was already completed.
    FallbackDuplicate,
    /// A fallback ticket skipped its predecessor.
    FallbackGap,
    /// The fallback-ticket sequence is exhausted.
    FallbackTicketExhausted,
    /// An offset, delta, extent, or visual offset is invalid.
    InvalidGeometry,
    /// A notification payload did not match the accepted FIFO entry.
    PayloadMismatch,
    /// The independent notification backlog is full.
    Capacity,
    /// A programmatic-scroll completion token is obsolete.
    StaleMutation,
}

/// Snapshot used by a trusted scene publisher before extraction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PublicationGate {
    /// Current target incarnation.
    pub key: TargetKey,
    /// Policy epoch that the resulting scene must carry.
    pub policy_epoch: u64,
    /// Input-acceptance generation that the resulting scene must carry.
    pub acceptance_epoch: u64,
    /// Whether a new GUI default may start from this publication.
    pub eligible: bool,
}

/// Result of reconciling one queued GUI publication with the live shared gate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PublicationReconciliation {
    /// The publication still owns the live eligible gate.
    Current,
    /// A different key, epoch, or retained-scene publication owns the gate.
    StaleUpdate,
    /// The expected gate is current but cannot accept new GUI defaults.
    StaleAcceptance,
    /// The expected gate was current, but its retained-scene association failed.
    InvalidCurrentAuthority,
}

/// Result of applying zero or more accepted defaults at an owner boundary.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Materialization {
    /// Canonical offset the owner must commit with its normal setter.
    pub offset: Offset,
    /// Number of defaults newly applied by this operation.
    pub applied_count: usize,
    /// Latest default-applied sequence after this operation.
    pub default_applied_sequence: u64,
}

/// One FIFO notification claimed before entering author code.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OwnerNotification {
    /// Immutable payload originally accepted by the GUI.
    pub input: AcceptedScroll,
    /// Canonical offset to commit before dispatch; it is unchanged when a
    /// geometry getter already materialized this default.
    pub offset: Offset,
}

/// Owner counters copied into publication and scheduling decisions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OwnerSnapshot {
    /// Latest accepted default committed by the canonical setter.
    pub default_applied_sequence: u64,
    /// Latest accepted notification claimed before author delivery.
    pub notification_delivered_sequence: u64,
    /// Latest canonical fallback completed by the owner.
    pub completed_fallback: FallbackTicket,
}

/// Token spanning the non-script native preparation and completion of one
/// programmatic viewport scroll.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProgrammaticScrollToken {
    key: TargetKey,
    mutation: u64,
}

/// Prepared state for a programmatic scroll setter.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProgrammaticScrollPreparation {
    /// Token completed after the canonical setter runs. `None` means numeric
    /// token space exhausted and publication remains permanently ineligible.
    pub token: Option<ProgrammaticScrollToken>,
    /// Accepted defaults that must be committed before the requested setter.
    pub materialization: Result<Materialization, OwnerRejection>,
}

#[derive(Clone)]
struct OwnerLedger {
    key: TargetKey,
    applied: u64,
    completed_fallback: FallbackTicket,
}

impl OwnerLedger {
    fn new(key: TargetKey) -> Self {
        Self {
            key,
            applied: 0,
            completed_fallback: FallbackTicket::default(),
        }
    }

    fn apply(
        &mut self,
        input: AcceptedScroll,
        current: Offset,
        extent: Bounds,
    ) -> Result<Offset, OwnerRejection> {
        if input.key != self.key {
            return Err(OwnerRejection::WrongTarget);
        }
        if input.sequence <= self.applied {
            return Err(OwnerRejection::Duplicate);
        }
        if self.applied.checked_add(1) != Some(input.sequence) {
            return Err(OwnerRejection::SequenceGap);
        }
        let desired = current.plus(input.delta).canonical();
        if !extent.valid() || !current.finite() || !input.delta.finite() || !desired.finite() {
            return Err(OwnerRejection::InvalidGeometry);
        }
        self.applied = input.sequence;
        Ok(extent.clamp(desired))
    }

    fn fallback_ready(&self, fallback: Fallback) -> Result<(), OwnerRejection> {
        if fallback.key != self.key {
            return Err(OwnerRejection::WrongTarget);
        }
        if fallback
            .after_sequence
            .is_some_and(|sequence| sequence > self.applied)
        {
            return Err(OwnerRejection::BarrierPending);
        }
        let FallbackOrder::Ticket(ticket) = fallback.order else {
            return Err(OwnerRejection::FallbackTicketExhausted);
        };
        if ticket <= self.completed_fallback {
            return Err(OwnerRejection::FallbackDuplicate);
        }
        if self.completed_fallback.next() != Some(ticket) {
            return Err(OwnerRejection::FallbackGap);
        }
        Ok(())
    }

    fn complete_fallback(&mut self, fallback: Fallback) -> Result<FallbackTicket, OwnerRejection> {
        self.fallback_ready(fallback)?;
        let FallbackOrder::Ticket(ticket) = fallback.order else {
            return Err(OwnerRejection::FallbackTicketExhausted);
        };
        self.completed_fallback = ticket;
        Ok(ticket)
    }

    fn completed_fallback(&self) -> FallbackTicket {
        self.completed_fallback
    }
}

#[derive(Clone)]
struct OwnerInputs {
    key: TargetKey,
    ledger: OwnerLedger,
    applied: u64,
    notified: u64,
    materialized: VecDeque<AcceptedScroll>,
}

impl OwnerInputs {
    fn new(key: TargetKey) -> Self {
        Self {
            key,
            ledger: OwnerLedger::new(key),
            applied: 0,
            notified: 0,
            materialized: VecDeque::new(),
        }
    }

    fn materialize(
        &mut self,
        input: AcceptedScroll,
        current: Offset,
        extent: Bounds,
    ) -> Result<Offset, OwnerRejection> {
        if !input.visual_offset.finite() {
            return Err(OwnerRejection::InvalidGeometry);
        }
        if self.materialized.len() == MAX_PENDING {
            return Err(OwnerRejection::Capacity);
        }
        let next = self.ledger.apply(input, current, extent)?;
        self.applied = input.sequence;
        self.materialized.push_back(input);
        Ok(next)
    }

    fn notification(
        &mut self,
        input: AcceptedScroll,
        current: Offset,
        extent: Bounds,
    ) -> Result<Offset, OwnerRejection> {
        if input.key != self.key {
            return Err(OwnerRejection::WrongTarget);
        }
        if input.sequence <= self.notified {
            return Err(OwnerRejection::Duplicate);
        }
        if self.notified.checked_add(1) != Some(input.sequence) {
            return Err(OwnerRejection::SequenceGap);
        }
        let next = if input.sequence > self.applied {
            self.materialize(input, current, extent)?
        } else {
            current
        };
        if self.materialized.front() != Some(&input) {
            return Err(OwnerRejection::PayloadMismatch);
        }
        self.materialized.pop_front();
        self.notified = input.sequence;
        Ok(next)
    }

    fn fallback_ready(&self, fallback: Fallback) -> Result<(), OwnerRejection> {
        if fallback.key != self.key {
            return Err(OwnerRejection::WrongTarget);
        }
        if fallback
            .after_sequence
            .is_some_and(|sequence| sequence > self.notified)
        {
            return Err(OwnerRejection::BarrierPending);
        }
        self.ledger.fallback_ready(fallback)
    }
}

struct ScrollState {
    key: TargetKey,
    scene: Option<Scene>,
    visual_offset: Offset,
    pending: VecDeque<AcceptedScroll>,
    last_sequence: u64,
    last_fallback_ticket: FallbackTicket,
    fallback_barrier: Option<u64>,
    fallback_exhausted: bool,
    policy_epoch: u64,
    policy_exhausted: bool,
    acceptance_epoch: u64,
    acceptance_exhausted: bool,
    accepting: bool,
    eligible: bool,
}

impl ScrollState {
    fn new(key: TargetKey) -> Self {
        Self {
            key,
            scene: None,
            visual_offset: Offset::default(),
            pending: VecDeque::new(),
            last_sequence: 0,
            last_fallback_ticket: FallbackTicket::default(),
            fallback_barrier: None,
            fallback_exhausted: false,
            policy_epoch: 0,
            policy_exhausted: false,
            acceptance_epoch: 0,
            acceptance_exhausted: false,
            accepting: false,
            eligible: false,
        }
    }

    fn revoke(&mut self) {
        self.accepting = false;
        self.eligible = false;
        match self.policy_epoch.checked_add(1) {
            Some(epoch) => self.policy_epoch = epoch,
            None => self.policy_exhausted = true,
        }
    }

    fn activate(&mut self, key: TargetKey) {
        if key != self.key {
            *self = Self::new(key);
        }
    }

    fn policy_epoch(&self) -> u64 {
        self.policy_epoch
    }

    fn install(&mut self, scene: Scene) -> bool {
        if scene.key != self.key
            || !scene.valid()
            || self.acceptance_exhausted
            || scene.acceptance_epoch != self.acceptance_epoch
            || scene.acknowledged > self.last_sequence
            || scene.completed_fallback > self.last_fallback_ticket
            || (scene.completed_fallback > FallbackTicket::default()
                && self
                    .fallback_barrier
                    .is_some_and(|barrier| scene.acknowledged < barrier))
            || self.scene.is_some_and(|old| {
                scene.publication <= old.publication
                    || scene.acknowledged < old.acknowledged
                    || scene.completed_fallback < old.completed_fallback
            })
        {
            return false;
        }
        while self
            .pending
            .front()
            .is_some_and(|input| input.sequence <= scene.acknowledged)
        {
            self.pending.pop_front();
        }
        let mut offset = scene.offset;
        for input in &self.pending {
            offset = scene.extent.clamp(offset.plus(input.delta).canonical());
        }
        let covered = scene.coverage.contains(offset);
        self.visual_offset = if covered { offset } else { scene.offset };
        let fallback_complete =
            !self.fallback_exhausted && scene.completed_fallback == self.last_fallback_ticket;
        self.eligible = covered
            && scene.eligible
            && fallback_complete
            && !self.policy_exhausted
            && !self.acceptance_exhausted
            && scene.policy_epoch == self.policy_epoch;
        self.accepting = self.eligible;
        if fallback_complete {
            self.fallback_barrier = None;
        }
        self.scene = Some(scene);
        true
    }

    fn scroll(&mut self, delta: Offset) -> Result<AcceptedScroll, Fallback> {
        if self.fallback_pending() {
            return Err(self.issue_fallback(FallbackReason::FallbackInFlight, delta));
        }
        match self.accept(delta) {
            Ok(input) => Ok(input),
            Err(reason) => Err(self.issue_fallback(reason, delta)),
        }
    }

    fn fallback_pending(&self) -> bool {
        self.fallback_exhausted
            || (self.last_fallback_ticket > FallbackTicket::default()
                && self
                    .scene
                    .is_none_or(|scene| scene.completed_fallback < self.last_fallback_ticket))
    }

    fn issue_fallback(&mut self, reason: FallbackReason, delta: Offset) -> Fallback {
        self.close_acceptance();
        let barrier = self.fallback_barrier.or_else(|| self.barrier_sequence());
        self.fallback_barrier = barrier;
        let (order, reason) = if self.fallback_exhausted {
            (
                FallbackOrder::Exhausted,
                FallbackReason::FallbackTicketExhausted,
            )
        } else if let Some(ticket) = self.last_fallback_ticket.next() {
            self.last_fallback_ticket = ticket;
            (FallbackOrder::Ticket(ticket), reason)
        } else {
            self.fallback_exhausted = true;
            (
                FallbackOrder::Exhausted,
                FallbackReason::FallbackTicketExhausted,
            )
        };
        Fallback {
            key: self.key,
            order,
            reason,
            after_sequence: barrier,
            delta,
        }
    }

    fn accept(&mut self, delta: Offset) -> Result<AcceptedScroll, FallbackReason> {
        let scene = self.scene.ok_or(FallbackReason::NoScene)?;
        if !self.accepting || !self.eligible {
            return Err(FallbackReason::Ineligible);
        }
        if !delta.finite() {
            return Err(FallbackReason::InvalidDelta);
        }
        let next = self.visual_offset.plus(delta).canonical();
        if !next.finite() {
            return Err(FallbackReason::InvalidDelta);
        }
        if !scene.extent.contains(next) {
            return Err(FallbackReason::ExtentBoundary);
        }
        if !scene.coverage.contains(next) {
            return Err(FallbackReason::MissingCoverage);
        }
        if self.pending.len() >= MAX_PENDING {
            return Err(FallbackReason::PendingLimit);
        }
        let sequence = self
            .last_sequence
            .checked_add(1)
            .ok_or(FallbackReason::SequenceExhausted)?;
        let input = AcceptedScroll {
            key: self.key,
            sequence,
            delta,
            visual_offset: next,
        };
        self.pending.push_back(input);
        self.last_sequence = sequence;
        self.visual_offset = next;
        Ok(input)
    }

    fn barrier_sequence(&self) -> Option<u64> {
        self.pending.back().map(|input| input.sequence)
    }

    fn visual_offset(&self) -> Offset {
        self.visual_offset
    }

    fn close_acceptance(&mut self) -> u64 {
        if self.accepting {
            self.accepting = false;
            match self.acceptance_epoch.checked_add(1) {
                Some(epoch) => self.acceptance_epoch = epoch,
                None => self.acceptance_exhausted = true,
            }
        }
        self.acceptance_epoch
    }

    fn rebase_revoked(&mut self, offset: Offset, extent: Bounds) -> Result<(), OwnerRejection> {
        if !extent.valid() || !extent.contains(offset) {
            return Err(OwnerRejection::InvalidGeometry);
        }
        self.visual_offset = offset;
        self.accepting = false;
        self.eligible = false;
        Ok(())
    }
}

struct NativeScrollState {
    gui: ScrollState,
    owner: OwnerInputs,
    accepted: VecDeque<AcceptedScroll>,
    authorized_policy_epoch: Option<u64>,
    blocking_listener_latched: bool,
    stale_presentation: Option<(u64, PublicationSequence)>,
    active_mutation: Option<u64>,
    next_mutation: Option<u64>,
}

impl NativeScrollState {
    fn new(key: TargetKey) -> Self {
        Self {
            gui: ScrollState::new(key),
            owner: OwnerInputs::new(key),
            accepted: VecDeque::new(),
            authorized_policy_epoch: None,
            blocking_listener_latched: false,
            stale_presentation: None,
            active_mutation: None,
            next_mutation: Some(1),
        }
    }

    fn publication_gate(&self) -> PublicationGate {
        PublicationGate {
            key: self.gui.key,
            policy_epoch: self.gui.policy_epoch,
            acceptance_epoch: self.gui.acceptance_epoch,
            eligible: self.gui.accepting
                && self.gui.eligible
                && self.active_mutation.is_none()
                && !self.blocking_listener_latched
                && self.authorized_policy_epoch == Some(self.gui.policy_epoch)
                && !self.gui.policy_exhausted
                && !self.gui.acceptance_exhausted,
        }
    }

    fn activate(&mut self, key: TargetKey) -> bool {
        if key == self.gui.key {
            return false;
        }
        let detached = self.gui.key == TargetKey::default();
        let keep_blocker = self.blocking_listener_latched
            && (detached || key.owner_generation == self.gui.key.owner_generation);
        *self = Self::new(key);
        self.blocking_listener_latched = keep_blocker;
        true
    }

    fn require_owner(&self, key: TargetKey) -> Result<(), OwnerRejection> {
        if key == self.gui.key {
            Ok(())
        } else {
            Err(OwnerRejection::WrongTarget)
        }
    }

    fn revoke(&mut self) -> u64 {
        self.authorized_policy_epoch = None;
        self.gui.revoke();
        self.gui.policy_epoch
    }

    fn revoke_for_blocking_listener(&mut self) -> u64 {
        self.blocking_listener_latched = true;
        self.revoke()
    }

    fn scene_matches(
        &self,
        key: TargetKey,
        policy_epoch: u64,
        acceptance_epoch: u64,
        publication: PublicationSequence,
    ) -> bool {
        self.gui.scene.is_some_and(|scene| {
            scene.key == key
                && scene.policy_epoch == policy_epoch
                && scene.acceptance_epoch == acceptance_epoch
                && scene.publication == publication
        })
    }

    fn install_scene(&mut self, mut scene: Scene) -> bool {
        if scene.acknowledged > self.owner.applied
            || scene.completed_fallback > self.owner.ledger.completed_fallback
        {
            return false;
        }
        let clears_stale = self
            .stale_presentation
            .is_some_and(|(sequence, publication)| {
                scene.acknowledged >= sequence && scene.publication > publication
            });
        if self.stale_presentation.is_some() && !clears_stale {
            scene.eligible = false;
        }
        if self.active_mutation.is_some()
            || self.blocking_listener_latched
            || self.authorized_policy_epoch != Some(scene.policy_epoch)
        {
            scene.eligible = false;
        }
        let installed = self.gui.install(scene);
        if installed && clears_stale {
            self.stale_presentation = None;
        }
        installed
    }

    fn scroll(&mut self, delta: Offset) -> ScrollDecision {
        if self.accepted.len() >= MAX_PENDING {
            let mut fallback = self.gui.issue_fallback(FallbackReason::PendingLimit, delta);
            if let Some(sequence) = self.accepted.back().map(|input| input.sequence) {
                fallback.after_sequence = Some(sequence);
                self.gui.fallback_barrier = Some(sequence);
            }
            return ScrollDecision::Fallback(fallback);
        }
        let pre_default_offset = self.gui.visual_offset();
        match self.gui.scroll(delta) {
            Ok(input) => {
                let publication = self
                    .gui
                    .scene
                    .map_or(PublicationSequence::default(), |scene| scene.publication);
                let accepted = AcceptedPresentation {
                    input,
                    policy_epoch: self.gui.policy_epoch,
                    publication,
                    presentation_offset: input.visual_offset,
                    pre_default_offset,
                };
                self.accepted.push_back(input);
                ScrollDecision::Accepted(accepted)
            }
            Err(mut fallback) => {
                if let Some(sequence) = self.accepted.back().map(|input| input.sequence)
                    && fallback
                        .after_sequence
                        .is_none_or(|barrier| barrier < sequence)
                {
                    fallback.after_sequence = Some(sequence);
                    self.gui.fallback_barrier = Some(sequence);
                }
                ScrollDecision::Fallback(fallback)
            }
        }
    }

    fn materialize_pending(
        &mut self,
        key: TargetKey,
        current: Offset,
        extent: Bounds,
    ) -> Result<Materialization, OwnerRejection> {
        self.require_owner(key)?;
        if !current.finite() || !extent.valid() || !extent.contains(current) {
            return Err(OwnerRejection::InvalidGeometry);
        }
        let inputs: Vec<_> = self
            .accepted
            .iter()
            .copied()
            .filter(|input| input.sequence > self.owner.applied)
            .collect();
        let mut owner = self.owner.clone();
        let mut offset = current;
        for input in &inputs {
            offset = owner.materialize(*input, offset, extent)?;
        }
        self.owner = owner;
        Ok(Materialization {
            offset,
            applied_count: inputs.len(),
            default_applied_sequence: self.owner.applied,
        })
    }

    fn begin_programmatic_scroll(
        &mut self,
        key: TargetKey,
        current: Offset,
        extent: Bounds,
    ) -> Result<ProgrammaticScrollPreparation, OwnerRejection> {
        self.require_owner(key)?;
        let materialization = self.materialize_pending(key, current, extent);
        self.revoke();
        let token = self.next_mutation.map(|mutation| {
            self.next_mutation = mutation.checked_add(1);
            self.active_mutation = Some(mutation);
            ProgrammaticScrollToken { key, mutation }
        });
        if self.next_mutation.is_none() {
            self.gui.policy_exhausted = true;
        }
        if let Ok(materialized) = materialization {
            let _ = self.gui.rebase_revoked(materialized.offset, extent);
        }
        Ok(ProgrammaticScrollPreparation {
            token,
            materialization,
        })
    }

    fn finish_programmatic_scroll(
        &mut self,
        token: ProgrammaticScrollToken,
        offset: Offset,
        extent: Bounds,
    ) -> Result<(), OwnerRejection> {
        self.require_owner(token.key)?;
        if self.active_mutation != Some(token.mutation) {
            return Err(OwnerRejection::StaleMutation);
        }
        self.active_mutation = None;
        self.revoke();
        self.gui.rebase_revoked(offset, extent)
    }
}

/// Cloneable opaque handle shared by the GUI and canonical page owner.
///
/// Methods only validate, copy, or update owned protocol state while holding
/// the mutex. Notification dispatch, script, layout, canonical setters, and
/// raster work must happen after a method returns. A page-visible callback or
/// listener count must never call [`Self::authorize_policy`]; authorization is
/// reserved for trusted native CSSOM, target-path, and listener validation.
#[derive(Clone)]
pub struct NativeScrollHandle {
    inner: Arc<Mutex<NativeScrollState>>,
}

/// Weak, owned policy invalidator for shared render-state stores.
///
/// It keeps neither the scroll owner nor DOM/Realm state alive. Callers may
/// hold the storage mutex while revoking: no native-scroll method calls back
/// into storage, script, layout, or a user-provided callback.
#[derive(Clone, Debug)]
pub(crate) struct NativeScrollInvalidator {
    inner: Weak<Mutex<NativeScrollState>>,
}

impl NativeScrollInvalidator {
    pub(crate) fn ptr_eq(&self, other: &Self) -> bool {
        self.inner.ptr_eq(&other.inner)
    }

    pub(crate) fn is_live(&self) -> bool {
        self.inner.strong_count() != 0
    }

    /// Revokes the current incarnation, or returns None when its owner is gone.
    #[track_caller]
    pub(crate) fn revoke_if_live(&self) -> Option<u64> {
        let inner = self.inner.upgrade()?;
        Some(NativeScrollHandle { inner }.revoke_policy())
    }
}

impl Default for NativeScrollHandle {
    fn default() -> Self {
        Self::new(TargetKey::default())
    }
}

impl NativeScrollHandle {
    /// Creates detached native scroll state for `key`.
    pub fn new(key: TargetKey) -> Self {
        Self {
            inner: Arc::new(Mutex::new(NativeScrollState::new(key))),
        }
    }

    /// Returns whether both handles refer to the same shared protocol state.
    pub fn is_same_handle(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }

    /// Creates a weak invalidator without acquiring the native mutex.
    pub(crate) fn weak_invalidator(&self) -> NativeScrollInvalidator {
        NativeScrollInvalidator {
            inner: Arc::downgrade(&self.inner),
        }
    }

    fn lock(&self) -> MutexGuard<'_, NativeScrollState> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Replaces the target incarnation and permanently retires its old queues.
    ///
    /// The page owner must retain the same `key` and pass it to every owner-side
    /// method. This prevents an old owner from consuming a replacement's input.
    pub fn activate(&self, key: TargetKey) -> bool {
        self.lock().activate(key)
    }

    /// Returns the current trusted publication gate.
    pub fn publication_gate(&self) -> PublicationGate {
        self.lock().publication_gate()
    }

    /// Returns whether another retained capture can ever authorize `key`.
    ///
    /// Temporary revocation, closed acceptance, and a missing scene are not
    /// blockers: a fresh owner publication can recover all three. The caller
    /// may use this query to avoid capture only after an owner-scoped latch or
    /// sequence exhaustion makes capture unable to authorize further native
    /// input until activation.
    pub fn retained_capture_possible(&self, key: TargetKey) -> bool {
        let state = self.lock();
        key == state.gui.key
            && !state.blocking_listener_latched
            && !state.gui.policy_exhausted
            && !state.gui.acceptance_exhausted
            && !state.gui.fallback_exhausted
            && state.gui.last_sequence != u64::MAX
    }

    /// Returns whether an exact installed scene owns the live eligible gate.
    pub fn publication_is_current(
        &self,
        expected_key: TargetKey,
        expected_policy_epoch: u64,
        expected_acceptance_epoch: u64,
        expected_publication: PublicationSequence,
    ) -> bool {
        let state = self.lock();
        let gate = state.publication_gate();
        gate.eligible
            && gate.key == expected_key
            && gate.policy_epoch == expected_policy_epoch
            && gate.acceptance_epoch == expected_acceptance_epoch
            && state.scene_matches(
                expected_key,
                expected_policy_epoch,
                expected_acceptance_epoch,
                expected_publication,
            )
    }

    /// Atomically reconciles a queued GUI publication with the live gate.
    ///
    /// The expected key, epochs, and scene-publication sequence are compared
    /// under the same protocol mutex as any resulting close or policy
    /// revocation. A mismatch is therefore a stale update and cannot mutate a
    /// newer owner generation or same-epoch scene.
    #[track_caller]
    pub fn reconcile_publication(
        &self,
        expected_key: TargetKey,
        expected_policy_epoch: u64,
        expected_acceptance_epoch: u64,
        expected_publication: PublicationSequence,
        retained_association_matches: bool,
    ) -> PublicationReconciliation {
        let mut state = self.lock();
        let gate = state.publication_gate();
        let scene_matches = state.scene_matches(
            expected_key,
            expected_policy_epoch,
            expected_acceptance_epoch,
            expected_publication,
        );
        if gate.key != expected_key
            || gate.policy_epoch != expected_policy_epoch
            || gate.acceptance_epoch != expected_acceptance_epoch
            || !scene_matches
        {
            return PublicationReconciliation::StaleUpdate;
        }
        if !retained_association_matches {
            let previous_epoch = state.gui.policy_epoch();
            let epoch = state.revoke();
            drop(state);
            trace_policy_revocation(previous_epoch, epoch, std::panic::Location::caller());
            return PublicationReconciliation::InvalidCurrentAuthority;
        }
        if !gate.eligible {
            state.gui.close_acceptance();
            return PublicationReconciliation::StaleAcceptance;
        }
        PublicationReconciliation::Current
    }

    /// Atomically authorizes and installs a retained scene at GUI commit time.
    ///
    /// Stale key, epoch, or publication candidates are ignored. Only an exact
    /// current candidate may revoke authority when its owned retained-pixel
    /// association or scene metadata is invalid. The outer acknowledgement,
    /// owner acknowledgement bounds, and fallback bounds are checked by the
    /// same mutex-protected installation.
    #[track_caller]
    pub fn commit_scene_publication(
        &self,
        expected_key: TargetKey,
        expected_policy_epoch: u64,
        expected_acceptance_epoch: u64,
        expected_publication: PublicationSequence,
        expected_acknowledged: u64,
        pending_scene: Scene,
        retained_association_matches: bool,
    ) -> PublicationReconciliation {
        let mut state = self.lock();
        let gate = state.publication_gate();
        let stale_gate = gate.key != expected_key
            || gate.policy_epoch != expected_policy_epoch
            || gate.acceptance_epoch != expected_acceptance_epoch;
        let installed_is_current_or_newer = state.gui.scene.is_some_and(|scene| {
            scene.key == expected_key && scene.publication >= expected_publication
        });
        if stale_gate || installed_is_current_or_newer {
            return PublicationReconciliation::StaleUpdate;
        }
        let pending_matches = pending_scene.key == expected_key
            && pending_scene.policy_epoch == expected_policy_epoch
            && pending_scene.acceptance_epoch == expected_acceptance_epoch
            && pending_scene.publication == expected_publication
            && pending_scene.acknowledged == expected_acknowledged
            && pending_scene.eligible;
        if !pending_matches || !retained_association_matches {
            let previous_epoch = state.gui.policy_epoch();
            let epoch = state.revoke();
            drop(state);
            trace_policy_revocation(previous_epoch, epoch, std::panic::Location::caller());
            return PublicationReconciliation::InvalidCurrentAuthority;
        }
        if state.active_mutation.is_some()
            || state.blocking_listener_latched
            || state.gui.policy_exhausted
            || state.gui.acceptance_exhausted
        {
            state.gui.close_acceptance();
            return PublicationReconciliation::StaleAcceptance;
        }
        let previous_authorization = state.authorized_policy_epoch;
        state.authorized_policy_epoch = Some(expected_policy_epoch);
        if !state.install_scene(pending_scene) {
            state.authorized_policy_epoch = previous_authorization;
            let previous_epoch = state.gui.policy_epoch();
            let epoch = state.revoke();
            drop(state);
            trace_policy_revocation(previous_epoch, epoch, std::panic::Location::caller());
            return PublicationReconciliation::InvalidCurrentAuthority;
        }
        if !state.publication_gate().eligible {
            state.gui.close_acceptance();
            return PublicationReconciliation::StaleAcceptance;
        }
        PublicationReconciliation::Current
    }

    /// Stops new GUI defaults without invalidating already composed pixels.
    ///
    /// The next trusted scene publication may reopen acceptance. Policy
    /// mutation uses [`Self::revoke_policy`] instead so old pixels also fail
    /// presentation validation.
    pub fn close_acceptance(&self) -> u64 {
        self.lock().gui.close_acceptance()
    }

    /// Authorizes one policy epoch after trusted native validation.
    ///
    /// Removal or count callbacks controlled by page script must not invoke
    /// this method. Authorization never reuses an installed scene: a newer
    /// publication must still carry the authorized epoch.
    pub fn authorize_policy(&self, key: TargetKey, policy_epoch: u64) -> bool {
        let mut state = self.lock();
        if state.require_owner(key).is_err()
            || state.active_mutation.is_some()
            || state.blocking_listener_latched
            || state.gui.policy_exhausted
            || state.gui.acceptance_exhausted
            || state.gui.policy_epoch != policy_epoch
        {
            return false;
        }
        state.authorized_policy_epoch = Some(policy_epoch);
        true
    }

    /// Revokes asynchronous eligibility before a native policy mutation.
    #[track_caller]
    pub fn revoke_policy(&self) -> u64 {
        let mut state = self.lock();
        let previous_epoch = state.gui.policy_epoch;
        let epoch = state.revoke();
        drop(state);
        trace_policy_revocation(previous_epoch, epoch, std::panic::Location::caller());
        epoch
    }

    /// Permanently latches a blocking wheel listener for this target
    /// incarnation and revokes before its listener entry becomes observable.
    ///
    /// There is deliberately no removal/count callback that clears this bit.
    /// Only [`Self::activate`] with a different trusted owner generation resets
    /// it; viewport and scroll-node replacement preserve the latch.
    pub fn revoke_for_blocking_listener(&self) -> u64 {
        self.lock().revoke_for_blocking_listener()
    }

    /// Installs a newer trusted scene snapshot.
    ///
    /// An eligible bit is honored only after [`Self::authorize_policy`] for the
    /// same current epoch. Installing an unauthorized scene cannot re-enable
    /// scrolling, even when page code fabricated a listener removal or count.
    pub fn install_scene(&self, scene: Scene) -> bool {
        self.lock().install_scene(scene)
    }

    /// Applies a GUI default only while an exact cached publication is current.
    ///
    /// The exact scene check and acceptance happen under one protocol lock, so
    /// a newer same-epoch publication cannot race between a GUI precheck and
    /// default acceptance. A stale cached publication returns `None` without
    /// changing the newer scene, FIFO, or fallback ledger.
    pub fn scroll_for_publication(
        &self,
        expected_key: TargetKey,
        expected_policy_epoch: u64,
        expected_acceptance_epoch: u64,
        expected_publication: PublicationSequence,
        delta: Offset,
    ) -> Option<ScrollDecision> {
        let mut state = self.lock();
        let gate = state.publication_gate();
        if !gate.eligible
            || gate.key != expected_key
            || gate.policy_epoch != expected_policy_epoch
            || gate.acceptance_epoch != expected_acceptance_epoch
            || !state.scene_matches(
                expected_key,
                expected_policy_epoch,
                expected_acceptance_epoch,
                expected_publication,
            )
        {
            return None;
        }
        Some(state.scroll(delta))
    }

    /// Applies a whole GUI default or issues an ordered canonical fallback.
    ///
    /// Accepted payloads remain in the shared FIFO until
    /// [`Self::claim_notification`] succeeds. Cursor, target, phase, modifier,
    /// and Realm metadata must be attached by the trusted GUI transport before
    /// the resulting notification is dispatched as an author event.
    pub fn scroll(&self, delta: Offset) -> ScrollDecision {
        self.lock().scroll(delta)
    }

    /// Rebinds the latest accepted presentation to an exact newer scene.
    ///
    /// This does not consume owner notifications or change default-applied
    /// counters. The GUI uses the returned live visual offset to recompose from
    /// the newly committed retained pixels.
    pub fn rebind_presentation_to_publication(
        &self,
        accepted: AcceptedPresentation,
        publication: PublicationSequence,
    ) -> Option<AcceptedPresentation> {
        let state = self.lock();
        let gate = state.publication_gate();
        let current = state.gui.scene?;
        if !gate.eligible
            || accepted.input.key != gate.key
            || accepted.policy_epoch != gate.policy_epoch
            || current.publication != publication
            || current.key != gate.key
            || current.policy_epoch != gate.policy_epoch
            || current.acceptance_epoch != gate.acceptance_epoch
            || accepted.input.sequence != state.gui.last_sequence
            || current.acknowledged >= accepted.input.sequence
            || accepted.publication >= publication
        {
            return None;
        }
        Some(AcceptedPresentation {
            publication,
            presentation_offset: state.gui.visual_offset,
            ..accepted
        })
    }

    /// Revalidates an accepted token immediately before presenting composed
    /// pixels. A failed check leaves its input in the FIFO and forces the next
    /// wheel through an ordered fallback until a newer acknowledging scene is
    /// installed.
    pub fn validate_for_presentation(&self, accepted: AcceptedPresentation) -> bool {
        let mut state = self.lock();
        if accepted.input.key != state.gui.key {
            return false;
        }
        let current_publication = state.gui.scene.map(|scene| scene.publication);
        if current_publication.is_some_and(|publication| publication > accepted.publication) {
            // Any presentation from an older publication of this owner is
            // harmlessly late, including after a hard policy transition. Its
            // replacement may already be composed from newer pixels, so it
            // must neither present nor poison the current eligible gate.
            return false;
        }
        let policy_valid = accepted.policy_epoch == state.gui.policy_epoch
            && state.authorized_policy_epoch == Some(state.gui.policy_epoch)
            && !state.blocking_listener_latched
            && state.active_mutation.is_none()
            && state.gui.eligible;
        let authority_valid = policy_valid && current_publication == Some(accepted.publication);
        if authority_valid && accepted.input.sequence < state.gui.last_sequence {
            // A later input from the same authorized scene supersedes this
            // frame. Dropping the older composed pixels must not poison the
            // cumulative latest token or disturb either FIFO entry.
            return false;
        }
        let valid = authority_valid
            && accepted.input.sequence == state.gui.last_sequence
            && state.gui.visual_offset == accepted.presentation_offset;
        if !valid {
            state.gui.eligible = false;
            state.gui.close_acceptance();
            let publication = state
                .gui
                .scene
                .map_or(accepted.publication, |scene| scene.publication);
            state.stale_presentation = Some((state.gui.last_sequence, publication));
        }
        valid
    }

    /// Returns the GUI's current visual offset without materializing defaults.
    pub fn visual_offset(&self) -> Offset {
        self.lock().gui.visual_offset
    }

    /// Returns whether `key` has an accepted default not yet applied by its
    /// canonical owner.
    ///
    /// A false result is a valid read-side linearization point: callers may
    /// skip layout and extent calculation for that geometry read. A wrong or
    /// replaced owner key also returns false and cannot inspect new-owner work.
    pub fn needs_owner_materialization(&self, key: TargetKey) -> bool {
        let state = self.lock();
        key == state.gui.key
            && state
                .accepted
                .back()
                .is_some_and(|input| input.sequence > state.owner.applied)
    }

    /// Returns the latest unacknowledged accepted sequence, if any.
    pub fn accepted_barrier(&self) -> Option<u64> {
        self.lock().gui.barrier_sequence()
    }

    /// Materializes every accepted default for `key` without consuming any
    /// notification. The caller must commit the returned offset with the
    /// canonical scroll setter before returning geometry to author code.
    pub fn materialize_pending(
        &self,
        key: TargetKey,
        current: Offset,
        extent: Bounds,
    ) -> Result<Materialization, OwnerRejection> {
        self.lock().materialize_pending(key, current, extent)
    }

    /// Claims one immutable accepted payload before entering its author
    /// callback. The caller commits `offset`, then dispatches a notification
    /// whose default action is disabled; it must not perform a second default.
    pub fn claim_notification(
        &self,
        key: TargetKey,
        current: Offset,
        extent: Bounds,
    ) -> Result<Option<OwnerNotification>, OwnerRejection> {
        let mut state = self.lock();
        state.require_owner(key)?;
        let Some(input) = state.accepted.front().copied() else {
            return Ok(None);
        };
        let mut owner = state.owner.clone();
        let offset = owner.notification(input, current, extent)?;
        state.owner = owner;
        state.accepted.pop_front();
        Ok(Some(OwnerNotification { input, offset }))
    }

    /// Claims the next notification only when its immutable GUI payload is
    /// exactly `expected`.
    ///
    /// The comparison happens while the protocol mutex is held and before the
    /// owner ledger or FIFO is changed. A stale, duplicated, or reordered page
    /// command therefore cannot consume the real head notification.
    pub fn claim_notification_for(
        &self,
        key: TargetKey,
        expected: AcceptedScroll,
        current: Offset,
        extent: Bounds,
    ) -> Result<OwnerNotification, OwnerRejection> {
        let mut state = self.lock();
        state.require_owner(key)?;
        let Some(input) = state.accepted.front().copied() else {
            return Err(OwnerRejection::Duplicate);
        };
        if input != expected {
            return Err(OwnerRejection::PayloadMismatch);
        }
        let mut owner = state.owner.clone();
        let offset = owner.notification(input, current, extent)?;
        state.owner = owner;
        state.accepted.pop_front();
        Ok(OwnerNotification { input, offset })
    }

    /// Returns separate default-applied and notification-delivered counters.
    pub fn owner_sequences(&self, key: TargetKey) -> Result<(u64, u64), OwnerRejection> {
        let state = self.lock();
        state.require_owner(key)?;
        Ok((state.owner.applied, state.owner.notified))
    }

    /// Returns the owner snapshot used for scene acknowledgement and barriers.
    pub fn owner_snapshot(&self, key: TargetKey) -> Result<OwnerSnapshot, OwnerRejection> {
        let state = self.lock();
        state.require_owner(key)?;
        Ok(OwnerSnapshot {
            default_applied_sequence: state.owner.applied,
            notification_delivered_sequence: state.owner.notified,
            completed_fallback: state.owner.ledger.completed_fallback,
        })
    }

    /// Returns the number of accepted notifications still in the shared FIFO.
    pub fn notification_backlog(&self, key: TargetKey) -> Result<usize, OwnerRejection> {
        let state = self.lock();
        state.require_owner(key)?;
        Ok(state.accepted.len())
    }

    /// Checks the delivered-notification barrier before a canonical fallback.
    pub fn fallback_ready(&self, fallback: Fallback) -> Result<(), OwnerRejection> {
        self.lock().owner.fallback_ready(fallback)
    }

    /// Records a fallback only after its canonical default has completed.
    pub fn complete_fallback(&self, fallback: Fallback) -> Result<FallbackTicket, OwnerRejection> {
        let mut state = self.lock();
        state.owner.fallback_ready(fallback)?;
        state.owner.ledger.complete_fallback(fallback)
    }

    /// Materializes older accepted defaults and revokes policy before a
    /// programmatic canonical setter mutates the viewport.
    ///
    /// The caller commits a successful `materialization`, runs its normal
    /// setter with no native-scroll hook recursion, then calls
    /// [`Self::finish_programmatic_scroll`] for a returned token with the actual
    /// clamped offset. A missing token leaves the protocol fail-closed.
    pub fn prepare_programmatic_scroll(
        &self,
        key: TargetKey,
        current: Offset,
        extent: Bounds,
    ) -> Result<ProgrammaticScrollPreparation, OwnerRejection> {
        self.lock().begin_programmatic_scroll(key, current, extent)
    }

    /// Finishes a prepared programmatic setter and leaves policy revoked until
    /// trusted validation authorizes a new scene. Obsolete owner tokens are
    /// inert and cannot rebase a replacement target.
    pub fn finish_programmatic_scroll(
        &self,
        token: ProgrammaticScrollToken,
        actual_offset: Offset,
        extent: Bounds,
    ) -> Result<(), OwnerRejection> {
        self.lock()
            .finish_programmatic_scroll(token, actual_offset, extent)
    }
}

#[cfg(test)]
mod tests;
