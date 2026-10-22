use super::*;

fn offset(y: f64) -> Offset {
    Offset { x: 0.0, y }
}

fn key() -> TargetKey {
    TargetKey {
        document: 7,
        owner_generation: 1,
        viewport_epoch: 2,
        scroll_node: 1,
    }
}

fn scene_for(
    key: TargetKey,
    publication: u64,
    acknowledged: u64,
    completed_fallback: u64,
    y: f64,
) -> Scene {
    Scene {
        key,
        publication: PublicationSequence(publication),
        policy_epoch: 0,
        acceptance_epoch: 0,
        acknowledged,
        completed_fallback: FallbackTicket(completed_fallback),
        offset: offset(y),
        extent: Bounds {
            min: offset(0.0),
            max: offset(1000.0),
        },
        coverage: Bounds {
            min: offset(0.0),
            max: offset(400.0),
        },
        eligible: true,
    }
}

fn scene(publication: u64, acknowledged: u64, y: f64) -> Scene {
    scene_for(key(), publication, acknowledged, 0, y)
}

fn completed_scene(publication: u64, acknowledged: u64, completed_fallback: u64, y: f64) -> Scene {
    scene_for(key(), publication, acknowledged, completed_fallback, y)
}

fn ready() -> ScrollState {
    let mut state = ScrollState::new(key());
    assert!(state.install(scene(1, 0, 0.0)));
    state
}

fn assert_ordered_fallback(
    fallback: Fallback,
    reason: FallbackReason,
    ticket: u64,
    after_sequence: Option<u64>,
    delta: Offset,
) {
    assert_eq!(fallback.key, key());
    assert_eq!(
        fallback.order,
        FallbackOrder::Ticket(FallbackTicket(ticket))
    );
    assert_eq!(fallback.reason, reason);
    assert_eq!(fallback.after_sequence, after_sequence);
    assert_eq!(fallback.delta, delta);
}

#[test]
fn visual_moves_before_any_owner_ack_and_rebases_only_unacknowledged_deltas() {
    let mut state = ready();
    assert_eq!(state.scroll(offset(20.25)).unwrap().sequence, 1);
    assert_eq!(state.scroll(offset(10.5)).unwrap().sequence, 2);
    assert_eq!(state.visual_offset(), offset(30.75));
    assert_eq!(state.barrier_sequence(), Some(2));
    assert!(state.install(scene(2, 1, 20.25)));
    assert_eq!(state.visual_offset(), offset(30.75));
    assert!(state.install(scene(3, 2, 30.75)));
    assert_eq!(state.visual_offset(), offset(30.75));
    assert_eq!(state.barrier_sequence(), None);
}

#[test]
fn stale_future_and_regressing_acks_leave_presentation_unchanged() {
    let mut state = ready();
    state.scroll(offset(30.0)).unwrap();
    assert!(!state.install(scene(2, 2, 30.0)));
    assert!(state.install(scene(2, 1, 30.0)));
    assert!(!state.install(scene(2, 1, 99.0)));
    assert!(!state.install(scene(3, 0, 0.0)));
    assert!(!state.install(completed_scene(3, 1, 1, 30.0)));
    assert_eq!(state.visual_offset(), offset(30.0));
}

#[test]
fn reverse_deltas_and_authoritative_programmatic_offset_rebase_in_order() {
    let mut state = ready();
    state.scroll(offset(100.0)).unwrap();
    state.scroll(offset(-20.0)).unwrap();
    assert!(state.install(scene(2, 1, 200.0)));
    assert_eq!(state.visual_offset(), offset(180.0));
}

#[test]
fn coverage_and_extent_fallback_never_consume_part_of_a_wheel() {
    let mut coverage = ready();
    let fallback = coverage.scroll(offset(401.0)).unwrap_err();
    assert_ordered_fallback(
        fallback,
        FallbackReason::MissingCoverage,
        1,
        None,
        offset(401.0),
    );
    assert_eq!(coverage.visual_offset(), offset(0.0));

    let mut extent = ready();
    let fallback = extent.scroll(offset(-1.0)).unwrap_err();
    assert_ordered_fallback(
        fallback,
        FallbackReason::ExtentBoundary,
        1,
        None,
        offset(-1.0),
    );
    assert_eq!(extent.visual_offset(), offset(0.0));
}

#[test]
fn fallback_latches_until_every_canonical_default_is_published() {
    let mut gui = ready();
    let accepted = gui.scroll(offset(20.0)).unwrap();
    let first = gui.scroll(offset(500.0)).unwrap_err();
    assert_ordered_fallback(
        first,
        FallbackReason::MissingCoverage,
        1,
        Some(1),
        offset(500.0),
    );
    assert_eq!(gui.visual_offset(), offset(20.0));

    let second = gui.scroll(offset(10.0)).unwrap_err();
    assert_ordered_fallback(
        second,
        FallbackReason::FallbackInFlight,
        2,
        Some(1),
        offset(10.0),
    );
    assert_eq!(gui.visual_offset(), offset(20.0));

    let extent = scene(1, 0, 0.0).extent;
    let mut owner = OwnerLedger::new(key());
    assert_eq!(
        owner.fallback_ready(first),
        Err(OwnerRejection::BarrierPending)
    );
    let mut canonical = owner.apply(accepted, offset(0.0), extent).unwrap();
    canonical = extent.clamp(canonical.plus(first.delta));
    assert_eq!(owner.complete_fallback(first), Ok(FallbackTicket(1)));

    let mut intermediate = completed_scene(2, 1, 1, canonical.y);
    intermediate.acceptance_epoch = gui.acceptance_epoch;
    intermediate.coverage.max = offset(1000.0);
    assert!(gui.install(intermediate));
    assert!(!gui.eligible);
    assert_eq!(gui.visual_offset(), offset(520.0));

    canonical = extent.clamp(canonical.plus(second.delta));
    assert_eq!(owner.complete_fallback(second), Ok(FallbackTicket(2)));
    let mut complete = completed_scene(3, 1, 2, canonical.y);
    complete.acceptance_epoch = gui.acceptance_epoch;
    complete.coverage.max = offset(1000.0);
    assert!(gui.install(complete));
    assert!(gui.eligible);
    assert_eq!(gui.visual_offset(), offset(530.0));
    assert_eq!(gui.scroll(offset(5.0)).unwrap().sequence, 2);
}

#[test]
fn fallback_completion_cannot_claim_an_unacknowledged_accept_barrier() {
    let mut state = ready();
    state.scroll(offset(25.0)).unwrap();
    let fallback = state.scroll(offset(500.0)).unwrap_err();
    assert_ordered_fallback(
        fallback,
        FallbackReason::MissingCoverage,
        1,
        Some(1),
        offset(500.0),
    );
    let mut impossible = completed_scene(2, 0, 1, 525.0);
    impossible.acceptance_epoch = state.acceptance_epoch;
    impossible.coverage.max = offset(1000.0);
    assert!(!state.install(impossible));
    assert_eq!(state.visual_offset(), offset(25.0));
}

#[test]
fn listener_revocation_requires_canonical_completion_before_async_resumes() {
    let mut state = ready();
    state.revoke();
    assert!(state.install(scene(2, 0, 0.0)));
    let fallback = state.scroll(offset(10.0)).unwrap_err();
    assert_ordered_fallback(fallback, FallbackReason::Ineligible, 1, None, offset(10.0));
    let mut owner = OwnerLedger::new(key());
    assert_eq!(owner.complete_fallback(fallback), Ok(FallbackTicket(1)));
    let mut fresh = completed_scene(3, 0, 1, 10.0);
    fresh.policy_epoch = state.policy_epoch();
    fresh.acceptance_epoch = state.acceptance_epoch;
    assert!(state.install(fresh));
    assert_eq!(state.scroll(offset(1.0)).unwrap().sequence, 1);
}

#[test]
fn policy_epoch_exhaustion_cannot_be_reenabled_by_a_scene() {
    let mut state = ready();
    state.policy_epoch = u64::MAX;
    state.revoke();
    let mut next = scene(2, 0, 0.0);
    next.policy_epoch = u64::MAX;
    assert!(state.install(next));
    let fallback = state.scroll(offset(1.0)).unwrap_err();
    assert_eq!(fallback.reason, FallbackReason::Ineligible);
    assert_eq!(fallback.order, FallbackOrder::Ticket(FallbackTicket(1)));
}

#[test]
fn acceptance_epoch_exhaustion_stays_closed_until_owner_identity_changes() {
    let mut state = ready();
    state.acceptance_epoch = u64::MAX;
    state.close_acceptance();
    assert!(state.acceptance_exhausted);
    let mut stale = scene(2, 0, 0.0);
    stale.acceptance_epoch = u64::MAX;
    assert!(!state.install(stale));
    assert_eq!(
        state.scroll(offset(1.0)).unwrap_err().reason,
        FallbackReason::Ineligible
    );

    let next = TargetKey {
        owner_generation: key().owner_generation + 1,
        ..key()
    };
    state.activate(next);
    assert_eq!(state.acceptance_epoch, 0);
    assert!(!state.acceptance_exhausted);
    assert!(state.install(scene_for(next, 1, 0, 0, 0.0)));
}

#[test]
fn navigation_and_viewport_changes_discard_pending_and_fallback_lineage() {
    for next in [
        TargetKey {
            document: 8,
            owner_generation: 2,
            ..key()
        },
        TargetKey {
            viewport_epoch: 3,
            owner_generation: 2,
            ..key()
        },
    ] {
        let mut state = ready();
        state.scroll(offset(20.0)).unwrap();
        state.scroll(offset(500.0)).unwrap_err();
        state.activate(next);
        assert_eq!(state.barrier_sequence(), None);
        assert!(!state.install(scene(2, 1, 20.0)));
        let fallback = state.scroll(offset(1.0)).unwrap_err();
        assert_eq!(fallback.key, next);
        assert_eq!(fallback.order, FallbackOrder::Ticket(FallbackTicket(1)));
        assert_eq!(fallback.reason, FallbackReason::NoScene);
    }
}

#[test]
fn missing_rebase_coverage_retains_pending_until_canonical_rebase() {
    let mut state = ready();
    let accepted = state.scroll(offset(300.0)).unwrap();
    let mut next = scene(2, 0, 0.0);
    next.coverage.max = offset(200.0);
    assert!(state.install(next));
    assert_eq!(state.visual_offset(), offset(0.0));
    assert_eq!(state.barrier_sequence(), Some(1));
    let fallback = state.scroll(offset(1.0)).unwrap_err();
    assert_ordered_fallback(
        fallback,
        FallbackReason::Ineligible,
        1,
        Some(1),
        offset(1.0),
    );

    let extent = scene(1, 0, 0.0).extent;
    let mut owner = OwnerLedger::new(key());
    let canonical = owner.apply(accepted, offset(0.0), extent).unwrap();
    let canonical = extent.clamp(canonical.plus(fallback.delta));
    owner.complete_fallback(fallback).unwrap();
    let mut complete = completed_scene(3, 1, 1, canonical.y);
    complete.acceptance_epoch = state.acceptance_epoch;
    complete.coverage.max = offset(1000.0);
    assert!(state.install(complete));
    assert_eq!(state.visual_offset(), offset(301.0));
    assert!(state.eligible);
}

#[test]
fn shrink_clamps_pending_once_and_ack_does_not_replay_it() {
    let mut state = ready();
    state.scroll(offset(200.0)).unwrap();
    let mut next = scene(2, 0, 0.0);
    next.extent.max = offset(100.0);
    assert!(state.install(next));
    assert_eq!(state.visual_offset(), offset(100.0));
    let mut ack = scene(3, 1, 100.0);
    ack.extent.max = offset(100.0);
    assert!(state.install(ack));
    assert_eq!(state.visual_offset(), offset(100.0));
    assert_eq!(state.barrier_sequence(), None);
}

#[test]
fn untrusted_numbers_and_input_sequence_exhaustion_use_fallback_tickets() {
    let mut state = ready();
    let mut bad = scene(2, 0, 0.0);
    bad.coverage.max.y = f64::INFINITY;
    assert!(!state.install(bad));
    let invalid = state.scroll(offset(f64::NAN)).unwrap_err();
    assert_eq!(invalid.order, FallbackOrder::Ticket(FallbackTicket(1)));
    assert_eq!(invalid.reason, FallbackReason::InvalidDelta);
    assert_eq!(state.visual_offset(), offset(0.0));

    let mut state = ready();
    for _ in 0..MAX_PENDING {
        state.scroll(offset(0.25)).unwrap();
    }
    let bounded = state.scroll(offset(0.25)).unwrap_err();
    assert_ordered_fallback(
        bounded,
        FallbackReason::PendingLimit,
        1,
        Some(MAX_PENDING as u64),
        offset(0.25),
    );
    assert_eq!(state.visual_offset(), offset(256.0));

    let mut state = ready();
    state.last_sequence = u64::MAX;
    let exhausted = state.scroll(offset(1.0)).unwrap_err();
    assert_eq!(exhausted.reason, FallbackReason::SequenceExhausted);
    assert_eq!(exhausted.order, FallbackOrder::Ticket(FallbackTicket(1)));
}

#[test]
fn fallback_ticket_exhaustion_is_permanent_until_identity_changes() {
    let mut state = ready();
    state.last_fallback_ticket = FallbackTicket(u64::MAX);
    state.scene.as_mut().unwrap().completed_fallback = FallbackTicket(u64::MAX);
    let exhausted = state.scroll(offset(500.0)).unwrap_err();
    assert_eq!(exhausted.order, FallbackOrder::Exhausted);
    assert_eq!(exhausted.reason, FallbackReason::FallbackTicketExhausted);
    let again = state.scroll(offset(1.0)).unwrap_err();
    assert_eq!(again.order, FallbackOrder::Exhausted);
    assert_eq!(again.reason, FallbackReason::FallbackTicketExhausted);

    let next_key = TargetKey {
        owner_generation: 2,
        ..key()
    };
    state.activate(next_key);
    assert!(state.install(scene_for(next_key, 1, 0, 0, 0.0)));
    assert_eq!(state.scroll(offset(1.0)).unwrap().sequence, 1);
}

#[test]
fn repeating_the_current_identity_does_not_reset_pending_sequences() {
    let mut state = ready();
    state.scroll(offset(10.0)).unwrap();
    state.activate(key());
    assert_eq!(state.barrier_sequence(), Some(1));
    assert_eq!(state.scroll(offset(2.0)).unwrap().sequence, 2);
}

#[test]
fn delayed_owner_can_ack_after_visual_state_has_already_moved() {
    use std::sync::mpsc;
    let (release, blocked) = mpsc::channel();
    let (acknowledge, ack) = mpsc::channel();
    let owner = std::thread::spawn(move || {
        blocked.recv().unwrap();
        acknowledge.send(scene(2, 1, 15.5)).unwrap();
    });
    let mut state = ready();
    state.scroll(offset(15.5)).unwrap();
    assert_eq!(state.visual_offset(), offset(15.5));
    assert!(matches!(ack.try_recv(), Err(mpsc::TryRecvError::Empty)));
    release.send(()).unwrap();
    assert!(state.install(ack.recv().unwrap()));
    owner.join().unwrap();
    assert_eq!(state.visual_offset(), offset(15.5));
    assert_eq!(state.barrier_sequence(), None);
}

#[test]
fn revocation_before_gui_lock_prevents_a_scroll_using_old_policy() {
    use std::sync::{Arc, Mutex, mpsc};
    let state = Arc::new(Mutex::new(ready()));
    let owner_state = Arc::clone(&state);
    let (registered, registration) = mpsc::channel();
    let owner = std::thread::spawn(move || {
        owner_state.lock().unwrap().revoke();
        registered.send(()).unwrap();
    });
    registration.recv().unwrap();
    let mut gui = state.lock().unwrap();
    assert!(gui.install(scene(2, 0, 0.0)));
    let fallback = gui.scroll(offset(10.0)).unwrap_err();
    assert_eq!(fallback.reason, FallbackReason::Ineligible);
    assert_eq!(fallback.order, FallbackOrder::Ticket(FallbackTicket(1)));
    drop(gui);
    owner.join().unwrap();
}

#[test]
fn owner_enforces_accepted_barrier_and_fallback_ticket_fifo() {
    let mut gui = ready();
    let input = gui.scroll(offset(20.0)).unwrap();
    let first = gui.scroll(offset(500.0)).unwrap_err();
    let second = gui.scroll(offset(10.0)).unwrap_err();
    let mut owner = OwnerLedger::new(key());
    assert_eq!(
        owner.fallback_ready(first),
        Err(OwnerRejection::BarrierPending)
    );
    assert_eq!(
        owner.fallback_ready(second),
        Err(OwnerRejection::BarrierPending)
    );
    let extent = scene(1, 0, 0.0).extent;
    let current = owner.apply(input, offset(0.0), extent).unwrap();
    assert_eq!(current, offset(20.0));
    assert_eq!(
        owner.fallback_ready(second),
        Err(OwnerRejection::FallbackGap)
    );
    assert_eq!(owner.complete_fallback(first), Ok(FallbackTicket(1)));
    assert_eq!(
        owner.complete_fallback(first),
        Err(OwnerRejection::FallbackDuplicate)
    );
    assert_eq!(owner.complete_fallback(second), Ok(FallbackTicket(2)));
    assert_eq!(owner.completed_fallback(), FallbackTicket(2));
}

#[test]
fn owner_rejects_gaps_old_lineages_and_exhausted_fallback_orders() {
    let mut gui = ready();
    let first = gui.scroll(offset(10.0)).unwrap();
    let second = gui.scroll(offset(20.0)).unwrap();
    let extent = scene(1, 0, 0.0).extent;
    let mut owner = OwnerLedger::new(key());
    assert_eq!(
        owner.apply(second, offset(0.0), extent),
        Err(OwnerRejection::SequenceGap)
    );
    let mut old = first;
    old.key.owner_generation = 0;
    assert_eq!(
        owner.apply(old, offset(0.0), extent),
        Err(OwnerRejection::WrongTarget)
    );
    let next = owner.apply(first, offset(0.0), extent).unwrap();
    assert_eq!(owner.apply(second, next, extent).unwrap(), offset(30.0));

    let exhausted = Fallback {
        key: key(),
        order: FallbackOrder::Exhausted,
        reason: FallbackReason::FallbackTicketExhausted,
        after_sequence: Some(2),
        delta: offset(1.0),
    };
    assert_eq!(
        owner.fallback_ready(exhausted),
        Err(OwnerRejection::FallbackTicketExhausted)
    );
}

#[test]
fn ack_only_publications_advance_independently_of_pixel_changes() {
    let mut state = ready();
    let input = state.scroll(offset(10.0)).unwrap();
    let mut acknowledgement = scene(2, input.sequence, 10.0);
    acknowledgement.publication = PublicationSequence::initial()
        .next()
        .unwrap()
        .next()
        .unwrap();
    assert!(state.install(acknowledgement));
    assert_eq!(state.barrier_sequence(), None);
    assert!(!state.install(scene(1, 0, 0.0)));
    assert_eq!(state.visual_offset(), offset(10.0));
    assert_eq!(PublicationSequence(u64::MAX).next(), None);
}

mod owner_sync_contract {
    use super::*;

    fn owner_key() -> TargetKey {
        TargetKey {
            document: 1,
            owner_generation: 1,
            viewport_epoch: 1,
            scroll_node: 1,
        }
    }

    fn owner_offset(y: f64) -> Offset {
        Offset { x: 0.0, y }
    }

    fn extent() -> Bounds {
        Bounds {
            min: owner_offset(0.0),
            max: Offset {
                x: 1000.0,
                y: 1000.0,
            },
        }
    }

    fn input(sequence: u64, y: f64) -> AcceptedScroll {
        AcceptedScroll {
            key: owner_key(),
            sequence,
            delta: owner_offset(y),
            visual_offset: owner_offset(y),
        }
    }

    #[test]
    fn geometry_getter_materializes_two_defaults_but_each_notification_still_arrives_once() {
        let mut owner = OwnerInputs::new(owner_key());
        let first = input(1, 10.25);
        let second = input(2, 20.5);
        let mut position = owner
            .materialize(first, owner_offset(0.0), extent())
            .unwrap();
        position = owner.materialize(second, position, extent()).unwrap();
        assert_eq!(position, owner_offset(30.75));
        assert_eq!((owner.applied, owner.notified), (2, 0));
        position = owner.notification(first, position, extent()).unwrap();
        position = owner.notification(second, position, extent()).unwrap();
        assert_eq!(position, owner_offset(30.75));
        assert_eq!((owner.applied, owner.notified), (2, 2));
        assert!(owner.materialized.is_empty());
        assert_eq!(
            owner.notification(first, position, extent()),
            Err(OwnerRejection::Duplicate)
        );
    }

    #[test]
    fn notification_without_an_earlier_getter_applies_the_default_once() {
        let mut owner = OwnerInputs::new(owner_key());
        let event = input(1, 20.5);
        let position = owner
            .notification(event, owner_offset(0.0), extent())
            .unwrap();
        assert_eq!(position, owner_offset(20.5));
        assert_eq!((owner.applied, owner.notified), (1, 1));
        assert_eq!(
            owner.notification(event, position, extent()),
            Err(OwnerRejection::Duplicate)
        );
    }

    #[test]
    fn canonical_fallback_waits_for_notifications_after_defaults_materialize() {
        let mut gui = ScrollState::new(owner_key());
        let ticket = gui.scroll(owner_offset(1.0)).unwrap_err().order;
        let fallback = Fallback {
            key: owner_key(),
            order: ticket,
            reason: FallbackReason::MissingCoverage,
            after_sequence: Some(1),
            delta: owner_offset(200.0),
        };
        let mut owner = OwnerInputs::new(owner_key());
        let event = input(1, 10.0);
        let position = owner
            .materialize(event, owner_offset(0.0), extent())
            .unwrap();
        assert_eq!(
            owner.fallback_ready(fallback),
            Err(OwnerRejection::BarrierPending)
        );
        owner.notification(event, position, extent()).unwrap();
        assert!(owner.fallback_ready(fallback).is_ok());
    }

    #[test]
    fn out_of_order_or_wrong_payload_does_not_consume_real_notification() {
        let mut owner = OwnerInputs::new(owner_key());
        let event = input(1, 10.0);
        let position = owner
            .materialize(event, owner_offset(0.0), extent())
            .unwrap();
        assert_eq!(
            owner.notification(input(2, 5.0), position, extent()),
            Err(OwnerRejection::SequenceGap)
        );
        assert_eq!(
            owner.notification(input(1, 99.0), position, extent()),
            Err(OwnerRejection::PayloadMismatch)
        );
        assert_eq!((owner.applied, owner.notified), (1, 0));
        assert_eq!(owner.materialized.len(), 1);
        assert_eq!(owner.notification(event, position, extent()), Ok(position));
    }

    #[test]
    fn wrong_target_and_invalid_geometry_leave_counters_unchanged() {
        let mut owner = OwnerInputs::new(owner_key());
        let mut event = input(1, 5.0);
        event.key.document = 99;
        assert_eq!(
            owner.notification(event, owner_offset(0.0), extent()),
            Err(OwnerRejection::WrongTarget)
        );
        assert_eq!(
            owner.materialize(input(1, f64::NAN), owner_offset(0.0), extent()),
            Err(OwnerRejection::InvalidGeometry)
        );
        let mut invalid_visual = input(1, 5.0);
        invalid_visual.visual_offset.y = f64::INFINITY;
        assert_eq!(
            owner.materialize(invalid_visual, owner_offset(0.0), extent()),
            Err(OwnerRejection::InvalidGeometry)
        );
        assert_eq!(
            (owner.applied, owner.notified, owner.materialized.len()),
            (0, 0, 0)
        );
    }

    #[test]
    fn notification_capacity_is_bounded_independently_of_default_acknowledgment() {
        let mut owner = OwnerInputs::new(owner_key());
        let wide = Bounds {
            min: owner_offset(0.0),
            max: owner_offset(2000.0),
        };
        let mut position = owner_offset(0.0);
        for sequence in 1..=MAX_PENDING as u64 {
            position = owner
                .materialize(input(sequence, 0.25), position, wide)
                .unwrap();
        }
        assert_eq!(
            owner.materialize(input(1025, 0.25), position, wide),
            Err(OwnerRejection::Capacity)
        );
        owner.notification(input(1, 0.25), position, wide).unwrap();
        assert!(owner.materialize(input(1025, 0.25), position, wide).is_ok());
    }

    #[test]
    fn scripted_scroll_between_getter_and_notification_is_not_overwritten() {
        let mut owner = OwnerInputs::new(owner_key());
        let event = input(1, 10.0);
        owner
            .materialize(event, owner_offset(0.0), extent())
            .unwrap();
        assert_eq!(
            owner.notification(event, owner_offset(500.0), extent()),
            Ok(owner_offset(500.0))
        );
    }
}

mod shared_handle_contract {
    use super::*;

    fn shared_key(generation: u64) -> TargetKey {
        TargetKey {
            document: generation,
            owner_generation: generation,
            viewport_epoch: 1,
            scroll_node: 1,
        }
    }

    fn shared_offset(y: f64) -> Offset {
        Offset { x: 0.0, y }
    }

    fn shared_extent() -> Bounds {
        Bounds {
            min: shared_offset(0.0),
            max: shared_offset(1000.0),
        }
    }

    fn shared_scene(
        key: TargetKey,
        publication: u64,
        epoch: u64,
        acknowledged: u64,
        offset: f64,
    ) -> Scene {
        Scene {
            key,
            publication: PublicationSequence::new(publication),
            policy_epoch: epoch,
            acceptance_epoch: 0,
            acknowledged,
            completed_fallback: FallbackTicket::default(),
            offset: shared_offset(offset),
            extent: shared_extent(),
            coverage: Bounds {
                min: shared_offset(0.0),
                max: shared_offset(900.0),
            },
            eligible: true,
        }
    }

    fn ready_handle() -> (NativeScrollHandle, TargetKey) {
        let key = shared_key(1);
        let handle = NativeScrollHandle::new(key);
        let gate = handle.publication_gate();
        assert!(handle.authorize_policy(key, gate.policy_epoch));
        assert!(handle.install_scene(shared_scene(key, 1, gate.policy_epoch, 0, 0.0)));
        (handle, key)
    }

    #[test]
    fn retained_capture_skips_only_owner_permanent_blockers() {
        let cases: [fn(&mut NativeScrollState); 4] = [
            |state| state.gui.policy_exhausted = true,
            |state| state.gui.acceptance_exhausted = true,
            |state| state.gui.fallback_exhausted = true,
            |state| state.gui.last_sequence = u64::MAX,
        ];
        for block in cases {
            let handle = NativeScrollHandle::new(shared_key(1));
            let key = handle.publication_gate().key;
            assert!(handle.retained_capture_possible(key));
            block(&mut handle.lock());
            assert!(!handle.retained_capture_possible(key));
        }

        let (closed, closed_key) = ready_handle();
        closed.close_acceptance();
        assert!(closed.retained_capture_possible(closed_key));

        let (handle, key) = ready_handle();
        handle.revoke_policy();
        assert!(handle.retained_capture_possible(key));
        assert!(!handle.retained_capture_possible(shared_key(2)));

        // A listener latch differs from recoverable close/revoke. It follows
        // the owner generation through same-owner retargeting.
        handle.revoke_for_blocking_listener();
        assert!(!handle.retained_capture_possible(key));
        assert!(!handle.activate(key));
        assert!(!handle.retained_capture_possible(key));

        let same_owner_retarget = TargetKey {
            viewport_epoch: key.viewport_epoch + 1,
            scroll_node: key.scroll_node + 1,
            ..key
        };
        assert!(handle.activate(same_owner_retarget));
        assert!(!handle.retained_capture_possible(same_owner_retarget));

        let replacement = shared_key(2);
        assert!(handle.activate(replacement));
        assert!(handle.retained_capture_possible(replacement));
        assert!(!handle.retained_capture_possible(key));
    }

    #[test]
    fn each_acceptance_owns_its_atomic_pre_default_offset() {
        let (handle, _) = ready_handle();
        let first = match handle.scroll(shared_offset(10.25)) {
            ScrollDecision::Accepted(accepted) => accepted,
            decision => panic!("unexpected {decision:?}"),
        };
        let second = match handle.scroll(shared_offset(20.5)) {
            ScrollDecision::Accepted(accepted) => accepted,
            decision => panic!("unexpected {decision:?}"),
        };

        assert_eq!(first.pre_default_offset(), shared_offset(0.0));
        assert_eq!(second.pre_default_offset(), shared_offset(10.25));
        assert!(
            first
                .input()
                .matches_pre_default_offset(first.pre_default_offset())
        );
        assert!(
            second
                .input()
                .matches_pre_default_offset(second.pre_default_offset())
        );
        assert!(
            !second
                .input()
                .matches_pre_default_offset(shared_offset(0.0))
        );
    }

    #[test]
    fn closing_acceptance_preserves_latest_pixels_and_issues_ordered_fallback() {
        let (handle, key) = ready_handle();
        let accepted = match handle.scroll(shared_offset(10.0)) {
            ScrollDecision::Accepted(accepted) => accepted,
            decision => panic!("unexpected {decision:?}"),
        };
        let before = handle.publication_gate();

        handle.close_acceptance();

        let closed = handle.publication_gate();
        assert_eq!(
            (closed.key, closed.policy_epoch),
            (before.key, before.policy_epoch)
        );
        assert_eq!(closed.acceptance_epoch, before.acceptance_epoch + 1);
        assert!(!closed.eligible);
        assert!(handle.validate_for_presentation(accepted));
        let fallback = match handle.scroll(shared_offset(5.0)) {
            ScrollDecision::Fallback(fallback) => fallback,
            decision => panic!("unexpected {decision:?}"),
        };
        assert_eq!(fallback.reason, FallbackReason::Ineligible);
        assert_eq!(fallback.after_sequence, Some(accepted.input().sequence));
        assert_eq!(
            handle.publication_gate().acceptance_epoch,
            closed.acceptance_epoch
        );
        for _ in 0..3 {
            assert!(handle.validate_for_presentation(accepted));
        }
        assert_eq!(handle.notification_backlog(key), Ok(1));
    }

    #[test]
    fn fresh_scene_reopens_acceptance_without_rolling_back_closed_pixels() {
        let (handle, key) = ready_handle();
        let accepted = match handle.scroll(shared_offset(10.0)) {
            ScrollDecision::Accepted(accepted) => accepted,
            decision => panic!("unexpected {decision:?}"),
        };
        let before = handle.publication_gate();
        handle.close_acceptance();
        assert!(handle.validate_for_presentation(accepted));
        let closed = handle.publication_gate();
        assert_eq!(closed.acceptance_epoch, before.acceptance_epoch + 1);
        handle.close_acceptance();
        assert_eq!(handle.publication_gate(), closed);

        assert!(handle.authorize_policy(key, before.policy_epoch));
        let mut stale = shared_scene(key, 2, before.policy_epoch, 0, 0.0);
        stale.acceptance_epoch = before.acceptance_epoch;
        assert!(!handle.install_scene(stale));
        let mut fresh = stale;
        fresh.acceptance_epoch = closed.acceptance_epoch;
        assert!(handle.install_scene(fresh));

        let current = handle.publication_gate();
        let rebound = handle
            .rebind_presentation_to_publication(accepted, PublicationSequence::new(2))
            .expect("closed P1 pixels can be rebound to the fresh P2 scene");
        assert!(current.eligible);
        assert!(!handle.validate_for_presentation(accepted));
        assert_eq!(handle.publication_gate(), current);
        assert!(handle.validate_for_presentation(rebound));
        let next = match handle.scroll(shared_offset(5.0)) {
            ScrollDecision::Accepted(accepted) => accepted,
            decision => panic!("unexpected {decision:?}"),
        };
        assert!(!handle.validate_for_presentation(rebound));
        assert!(handle.validate_for_presentation(next));
        let reopened = handle.publication_gate();
        handle.close_acceptance();
        assert_eq!(
            handle.publication_gate().acceptance_epoch,
            reopened.acceptance_epoch + 1
        );
    }

    #[test]
    fn full_policy_revoke_invalidates_pixels_that_input_close_preserved() {
        let (handle, _) = ready_handle();
        let accepted = match handle.scroll(shared_offset(10.0)) {
            ScrollDecision::Accepted(accepted) => accepted,
            decision => panic!("unexpected {decision:?}"),
        };
        let epoch = handle.publication_gate().policy_epoch;
        handle.close_acceptance();
        assert!(handle.validate_for_presentation(accepted));

        let revoked_epoch = handle.revoke_policy();

        assert!(revoked_epoch > epoch);
        assert!(!handle.publication_gate().eligible);
        assert!(!handle.validate_for_presentation(accepted));
    }

    #[test]
    fn invalid_latest_token_closes_the_publication_gate_and_new_input() {
        let (handle, _) = ready_handle();
        let accepted = match handle.scroll(shared_offset(10.0)) {
            ScrollDecision::Accepted(accepted) => accepted,
            decision => panic!("unexpected {decision:?}"),
        };
        let mut invalid = accepted;
        invalid.presentation_offset.y += 1.0;
        let before = handle.publication_gate();

        assert!(!handle.validate_for_presentation(invalid));
        let closed = handle.publication_gate();
        assert!(!closed.eligible);
        assert_eq!(closed.acceptance_epoch, before.acceptance_epoch + 1);
        assert!(matches!(
            handle.scroll(shared_offset(1.0)),
            ScrollDecision::Fallback(Fallback {
                reason: FallbackReason::Ineligible,
                ..
            })
        ));
        assert_eq!(handle.publication_gate(), closed);
        assert_eq!(handle.notification_backlog(accepted.input().key), Ok(1));
    }

    #[test]
    fn shared_fifo_keeps_default_and_notification_progress_separate() {
        let (handle, key) = ready_handle();
        assert!(matches!(
            handle.scroll(shared_offset(10.25)),
            ScrollDecision::Accepted(_)
        ));
        assert!(matches!(
            handle.scroll(shared_offset(20.5)),
            ScrollDecision::Accepted(_)
        ));
        let materialized = handle
            .materialize_pending(key, shared_offset(0.0), shared_extent())
            .unwrap();
        assert_eq!(materialized.offset, shared_offset(30.75));
        assert_eq!(handle.owner_sequences(key), Ok((2, 0)));
        assert_eq!(handle.notification_backlog(key), Ok(2));

        let first = handle
            .claim_notification(key, materialized.offset, shared_extent())
            .unwrap()
            .unwrap();
        let second = handle
            .claim_notification(key, first.offset, shared_extent())
            .unwrap()
            .unwrap();
        assert_eq!(second.offset, shared_offset(30.75));
        assert_eq!(handle.owner_sequences(key), Ok((2, 2)));
        assert_eq!(handle.notification_backlog(key), Ok(0));
    }

    #[test]
    fn exact_claim_rejects_reordered_page_commands_without_consuming_the_head() {
        let (handle, key) = ready_handle();
        let first = match handle.scroll(shared_offset(10.0)) {
            ScrollDecision::Accepted(accepted) => accepted.input(),
            decision => panic!("unexpected {decision:?}"),
        };
        let second = match handle.scroll(shared_offset(20.0)) {
            ScrollDecision::Accepted(accepted) => accepted.input(),
            decision => panic!("unexpected {decision:?}"),
        };

        assert_eq!(
            handle.claim_notification_for(key, second, shared_offset(0.0), shared_extent(),),
            Err(OwnerRejection::PayloadMismatch)
        );
        assert_eq!(handle.owner_sequences(key), Ok((0, 0)));
        assert_eq!(handle.notification_backlog(key), Ok(2));

        let first_notification = handle
            .claim_notification_for(key, first, shared_offset(0.0), shared_extent())
            .unwrap();
        assert_eq!(first_notification.offset, shared_offset(10.0));
        let second_notification = handle
            .claim_notification_for(key, second, first_notification.offset, shared_extent())
            .unwrap();
        assert_eq!(second_notification.offset, shared_offset(30.0));
        assert_eq!(handle.owner_sequences(key), Ok((2, 2)));
        assert_eq!(handle.notification_backlog(key), Ok(0));
    }

    #[test]
    fn materialization_probe_tracks_only_unapplied_defaults_for_the_exact_owner() {
        let key = shared_key(1);
        let handle = NativeScrollHandle::new(key);
        assert!(!handle.needs_owner_materialization(key));

        let gate = handle.publication_gate();
        assert!(handle.authorize_policy(key, gate.policy_epoch));
        assert!(handle.install_scene(shared_scene(key, 1, gate.policy_epoch, 0, 0.0)));
        assert!(!handle.needs_owner_materialization(key));

        assert!(matches!(
            handle.scroll(shared_offset(10.0)),
            ScrollDecision::Accepted(_)
        ));
        assert!(handle.needs_owner_materialization(key));
        let materialized = handle
            .materialize_pending(key, shared_offset(0.0), shared_extent())
            .unwrap();
        assert_eq!(materialized.default_applied_sequence, 1);
        assert!(!handle.needs_owner_materialization(key));
        assert_eq!(handle.owner_sequences(key), Ok((1, 0)));
        assert_eq!(handle.notification_backlog(key), Ok(1));

        let replacement = shared_key(2);
        assert!(handle.activate(replacement));
        assert!(!handle.needs_owner_materialization(key));
        assert!(!handle.needs_owner_materialization(replacement));
    }

    #[test]
    fn blocking_listener_latches_for_the_owner_generation() {
        let (handle, key) = ready_handle();
        let epoch = handle.revoke_for_blocking_listener();
        assert!(handle.install_scene(shared_scene(key, 2, epoch, 0, 0.0)));
        assert!(matches!(
            handle.scroll(shared_offset(1.0)),
            ScrollDecision::Fallback(Fallback {
                reason: FallbackReason::Ineligible,
                ..
            })
        ));
        assert!(!handle.authorize_policy(key, epoch));
        let mut blocked = shared_scene(key, 3, epoch, 0, 0.0);
        blocked.acceptance_epoch = handle.publication_gate().acceptance_epoch;
        assert!(handle.install_scene(blocked));
        assert!(matches!(
            handle.scroll(shared_offset(1.0)),
            ScrollDecision::Fallback(_)
        ));
        let viewport_replacement = TargetKey {
            viewport_epoch: key.viewport_epoch + 1,
            ..key
        };
        assert!(handle.activate(viewport_replacement));
        let replacement_gate = handle.publication_gate();
        assert!(!handle.authorize_policy(viewport_replacement, replacement_gate.policy_epoch));
    }

    #[test]
    fn programmatic_setter_materializes_revokes_and_does_not_replay_default() {
        let (handle, key) = ready_handle();
        let accepted = match handle.scroll(shared_offset(10.0)) {
            ScrollDecision::Accepted(input) => input,
            other => panic!("expected accepted input, got {other:?}"),
        };
        let prepared = handle
            .prepare_programmatic_scroll(key, shared_offset(0.0), shared_extent())
            .unwrap();
        assert_eq!(
            prepared.materialization.unwrap().offset,
            shared_offset(10.0)
        );
        assert!(!handle.publication_gate().eligible);
        handle
            .finish_programmatic_scroll(
                prepared.token.unwrap(),
                shared_offset(500.0),
                shared_extent(),
            )
            .unwrap();
        let notification = handle
            .claim_notification(key, shared_offset(500.0), shared_extent())
            .unwrap()
            .unwrap();
        assert_eq!(notification.input, accepted.input());
        assert_eq!(notification.offset, shared_offset(500.0));
        assert_eq!(handle.owner_sequences(key), Ok((1, 1)));
    }

    #[test]
    fn replacement_target_rejects_old_owner_and_completion_token() {
        let (handle, old_key) = ready_handle();
        let prepared = handle
            .prepare_programmatic_scroll(old_key, shared_offset(0.0), shared_extent())
            .unwrap();
        let new_key = shared_key(2);
        assert!(handle.activate(new_key));
        assert_eq!(
            handle.materialize_pending(old_key, shared_offset(0.0), shared_extent()),
            Err(OwnerRejection::WrongTarget)
        );
        assert_eq!(
            handle.finish_programmatic_scroll(
                prepared.token.unwrap(),
                shared_offset(200.0),
                shared_extent()
            ),
            Err(OwnerRejection::WrongTarget)
        );
        assert_eq!(handle.visual_offset(), shared_offset(0.0));
        assert_eq!(handle.notification_backlog(new_key), Ok(0));
    }

    #[test]
    fn handle_is_send_sync_and_clones_share_the_same_fifo() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<NativeScrollHandle>();
        let (handle, key) = ready_handle();
        let gui = handle.clone();
        let accepted = std::thread::spawn(move || gui.scroll(shared_offset(5.0)))
            .join()
            .unwrap();
        assert!(matches!(accepted, ScrollDecision::Accepted(_)));
        assert_eq!(handle.notification_backlog(key), Ok(1));
    }

    #[test]
    fn f32_canonical_boundary_preserves_fractional_steps_without_owner_drift() {
        let (handle, key) = ready_handle();
        let mut expected = 0.0_f32;
        for _ in 0..100 {
            expected = (f64::from(expected) + 0.1) as f32;
            assert!(matches!(
                handle.scroll(shared_offset(0.1)),
                ScrollDecision::Accepted(_)
            ));
        }
        let materialized = handle
            .materialize_pending(key, shared_offset(0.0), shared_extent())
            .unwrap();
        assert_eq!(materialized.offset.y, f64::from(expected));
        assert_eq!(handle.visual_offset(), materialized.offset);
        assert_ne!(materialized.offset.y.fract(), 0.0);
    }

    #[test]
    fn presentation_revalidation_never_rolls_back_the_owner_fifo() {
        let (handle, key) = ready_handle();
        let accepted = match handle.scroll(shared_offset(10.0)) {
            ScrollDecision::Accepted(accepted) => accepted,
            other => panic!("expected accepted input, got {other:?}"),
        };
        assert!(handle.validate_for_presentation(accepted));
        handle.revoke_for_blocking_listener();
        assert!(!handle.validate_for_presentation(accepted));
        assert_eq!(handle.notification_backlog(key), Ok(1));
        let fallback = match handle.scroll(shared_offset(1.0)) {
            ScrollDecision::Fallback(fallback) => fallback,
            other => panic!("expected fallback, got {other:?}"),
        };
        assert_eq!(fallback.after_sequence, Some(1));
        let notification = handle
            .claim_notification(key, shared_offset(0.0), shared_extent())
            .unwrap()
            .unwrap();
        assert_eq!(notification.input, accepted.input());
    }

    #[test]
    fn newer_accept_benignly_supersedes_an_older_composed_frame() {
        let (handle, key) = ready_handle();
        let first = match handle.scroll(shared_offset(10.0)) {
            ScrollDecision::Accepted(accepted) => accepted,
            other => panic!("expected first accepted input, got {other:?}"),
        };
        let second = match handle.scroll(shared_offset(5.0)) {
            ScrollDecision::Accepted(accepted) => accepted,
            other => panic!("expected second accepted input, got {other:?}"),
        };
        assert!(!handle.validate_for_presentation(first));
        assert!(handle.validate_for_presentation(second));
        assert_eq!(handle.notification_backlog(key), Ok(2));
        assert_eq!(handle.owner_sequences(key), Ok((0, 0)));
    }

    #[test]
    fn newer_equivalent_publication_keeps_latest_pixels_until_the_next_accept() {
        let (handle, key) = ready_handle();
        let p1 = match handle.scroll(shared_offset(10.0)) {
            ScrollDecision::Accepted(accepted) => accepted,
            other => panic!("expected P1 accepted input, got {other:?}"),
        };
        let epoch = handle.publication_gate().policy_epoch;
        assert!(handle.install_scene(shared_scene(key, 2, epoch, 0, 0.0)));
        let gate = handle.publication_gate();
        let owner = handle.owner_sequences(key).unwrap();
        let backlog = handle.notification_backlog(key).unwrap();
        let rebound = handle
            .rebind_presentation_to_publication(p1, PublicationSequence::new(2))
            .expect("latest accepted pixels can be rebound to P2");
        assert!(!handle.validate_for_presentation(p1));
        assert_eq!(handle.publication_gate(), gate);
        assert!(handle.validate_for_presentation(rebound));
        assert_eq!(handle.owner_sequences(key), Ok(owner));
        assert_eq!(handle.notification_backlog(key), Ok(backlog));
        let p2 = match handle.scroll(shared_offset(5.0)) {
            ScrollDecision::Accepted(accepted) => accepted,
            other => panic!("expected P2 accepted input, got {other:?}"),
        };
        assert!(!handle.validate_for_presentation(p1));
        assert_eq!(p2.publication, PublicationSequence::new(2));
        assert!(handle.validate_for_presentation(p2));
        assert_eq!(handle.notification_backlog(key), Ok(2));
    }

    #[test]
    fn rebind_uses_replayed_live_offset_without_rewriting_the_owner_fifo() {
        let (handle, key) = ready_handle();
        let p1 = match handle.scroll(shared_offset(10.0)) {
            ScrollDecision::Accepted(accepted) => accepted,
            other => panic!("expected P1 accepted input, got {other:?}"),
        };
        let original_input = p1.input();
        let epoch = handle.publication_gate().policy_epoch;
        assert!(handle.install_scene(shared_scene(key, 2, epoch, 0, 40.0)));
        assert_eq!(handle.visual_offset(), shared_offset(50.0));

        let rebound = handle
            .rebind_presentation_to_publication(p1, PublicationSequence::new(2))
            .expect("P1 input replays over the P2 canonical offset");
        assert_eq!(rebound.visual_offset(), shared_offset(50.0));
        assert_eq!(rebound.input(), original_input);
        assert_eq!(rebound.input().visual_offset, shared_offset(10.0));
        assert!(handle.validate_for_presentation(rebound));
        assert_eq!(handle.owner_sequences(key), Ok((0, 0)));
        assert_eq!(handle.notification_backlog(key), Ok(1));

        let materialized = handle
            .materialize_pending(key, shared_offset(40.0), shared_extent())
            .unwrap();
        assert_eq!(materialized.offset, shared_offset(50.0));
        let notification = handle
            .claim_notification(key, materialized.offset, shared_extent())
            .unwrap()
            .expect("one retained FIFO notification");
        assert_eq!(notification.input, original_input);
        assert!(
            handle
                .claim_notification(key, notification.offset, shared_extent())
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn rebind_uses_the_new_scene_clamp_without_rewriting_the_owner_fifo() {
        let (handle, key) = ready_handle();
        let p1 = match handle.scroll(shared_offset(10.0)) {
            ScrollDecision::Accepted(accepted) => accepted,
            other => panic!("expected P1 accepted input, got {other:?}"),
        };
        let original_input = p1.input();
        let epoch = handle.publication_gate().policy_epoch;
        let mut scene = shared_scene(key, 2, epoch, 0, 40.0);
        scene.extent.max = shared_offset(45.0);
        scene.coverage.max = shared_offset(45.0);
        assert!(handle.install_scene(scene));
        assert_eq!(handle.visual_offset(), shared_offset(45.0));

        let rebound = handle
            .rebind_presentation_to_publication(p1, PublicationSequence::new(2))
            .expect("P1 input clamps against the P2 extent");
        assert_eq!(rebound.visual_offset(), shared_offset(45.0));
        assert_eq!(rebound.input(), original_input);
        assert_eq!(rebound.input().visual_offset, shared_offset(10.0));
        assert!(handle.validate_for_presentation(rebound));
        assert_eq!(handle.owner_sequences(key), Ok((0, 0)));
        assert_eq!(handle.notification_backlog(key), Ok(1));

        let extent = scene.extent;
        let materialized = handle
            .materialize_pending(key, shared_offset(40.0), extent)
            .unwrap();
        assert_eq!(materialized.offset, shared_offset(45.0));
        let notification = handle
            .claim_notification(key, materialized.offset, extent)
            .unwrap()
            .expect("one retained FIFO notification");
        assert_eq!(notification.input, original_input);
        assert!(
            handle
                .claim_notification(key, notification.offset, extent)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn newer_publication_with_a_different_visual_offset_rejects_old_pixels() {
        let (handle, key) = ready_handle();
        let accepted = match handle.scroll(shared_offset(10.0)) {
            ScrollDecision::Accepted(accepted) => accepted,
            decision => panic!("unexpected {decision:?}"),
        };
        handle
            .materialize_pending(key, shared_offset(0.0), shared_extent())
            .unwrap();
        let epoch = handle.publication_gate().policy_epoch;
        assert!(handle.install_scene(shared_scene(key, 2, epoch, 1, 20.0)));

        let current = handle.publication_gate();
        assert!(!handle.validate_for_presentation(accepted));
        assert_eq!(handle.publication_gate(), current);
        assert!(current.eligible);
        assert_eq!(handle.notification_backlog(key), Ok(1));
    }

    #[test]
    fn late_old_publication_after_hard_policy_change_is_drop_only() {
        let (handle, key) = ready_handle();
        let p1 = match handle.scroll(shared_offset(10.0)) {
            ScrollDecision::Accepted(accepted) => accepted,
            other => panic!("expected P1 accepted input, got {other:?}"),
        };
        handle.revoke_policy();
        let fresh = handle.publication_gate();
        assert!(handle.authorize_policy(key, fresh.policy_epoch));
        assert!(handle.install_scene(shared_scene(key, 2, fresh.policy_epoch, 0, 10.0,)));
        let p2_gate = handle.publication_gate();
        assert!(!handle.validate_for_presentation(p1));
        assert_eq!(handle.publication_gate(), p2_gate);
        assert!(p2_gate.eligible);
        assert!(matches!(
            handle.scroll(shared_offset(5.0)),
            ScrollDecision::Accepted(_)
        ));
    }

    #[test]
    fn scene_ack_does_not_remove_the_notification_fallback_barrier() {
        let (handle, key) = ready_handle();
        let accepted = match handle.scroll(shared_offset(10.0)) {
            ScrollDecision::Accepted(accepted) => accepted,
            other => panic!("expected accepted input, got {other:?}"),
        };
        let materialized = handle
            .materialize_pending(key, shared_offset(0.0), shared_extent())
            .unwrap();
        let epoch = handle.publication_gate().policy_epoch;
        assert!(handle.install_scene(shared_scene(key, 2, epoch, 1, 10.0)));
        assert_eq!(handle.accepted_barrier(), None);
        let fallback = match handle.scroll(shared_offset(950.0)) {
            ScrollDecision::Fallback(fallback) => fallback,
            other => panic!("expected fallback, got {other:?}"),
        };
        assert_eq!(fallback.after_sequence, Some(accepted.input().sequence));
        assert_eq!(
            handle.fallback_ready(fallback),
            Err(OwnerRejection::BarrierPending)
        );
        handle
            .claim_notification(key, materialized.offset, shared_extent())
            .unwrap()
            .unwrap();
        assert!(handle.fallback_ready(fallback).is_ok());
    }

    #[test]
    fn scene_cannot_acknowledge_owner_work_that_did_not_complete() {
        let (handle, key) = ready_handle();
        let epoch = handle.publication_gate().policy_epoch;
        assert!(matches!(
            handle.scroll(shared_offset(10.0)),
            ScrollDecision::Accepted(_)
        ));

        assert!(!handle.install_scene(shared_scene(key, 2, epoch, 1, 10.0)));
        assert_eq!(handle.accepted_barrier(), Some(1));

        let materialized = handle
            .materialize_pending(key, shared_offset(0.0), shared_extent())
            .unwrap();
        assert_eq!(materialized.default_applied_sequence, 1);
        assert!(handle.install_scene(shared_scene(key, 2, epoch, 1, 10.0)));

        let mut forged_fallback = shared_scene(key, 3, epoch, 1, 10.0);
        forged_fallback.completed_fallback = FallbackTicket::new(1);
        assert!(!handle.install_scene(forged_fallback));
    }

    #[test]
    fn zero_delta_inputs_remain_one_event_per_fifo_entry() {
        let (handle, key) = ready_handle();
        let first = match handle.scroll(shared_offset(0.0)) {
            ScrollDecision::Accepted(accepted) => accepted,
            other => panic!("expected first accepted input, got {other:?}"),
        };
        let second = match handle.scroll(shared_offset(0.0)) {
            ScrollDecision::Accepted(accepted) => accepted,
            other => panic!("expected second accepted input, got {other:?}"),
        };
        assert_eq!((first.input().sequence, second.input().sequence), (1, 2));
        assert_eq!(handle.notification_backlog(key), Ok(2));
        assert_eq!(
            handle
                .claim_notification(key, shared_offset(0.0), shared_extent())
                .unwrap()
                .unwrap()
                .input,
            first.input()
        );
        assert_eq!(
            handle
                .claim_notification(key, shared_offset(0.0), shared_extent())
                .unwrap()
                .unwrap()
                .input,
            second.input()
        );
    }

    #[test]
    fn bootstrap_revokes_immediately_before_blocking_listener_store_mutation() {
        let source = super::super::super::DOM_BOOTSTRAP;
        let hook = source
            .find("nativeWillAddBlockingWheelListener();")
            .expect("blocking listener hook");
        let push = source[hook..]
            .find("list.push(entry);")
            .map(|offset| hook + offset)
            .expect("listener insertion after hook");
        let publish = source[hook..push].find("authorize_policy");
        assert!(hook < push);
        assert_eq!(publish, None);
    }
}

mod runtime_hook_contract {
    use super::super::super::JsRuntime;
    use super::*;

    fn runtime_key() -> TargetKey {
        TargetKey {
            document: 44,
            owner_generation: 9,
            viewport_epoch: 3,
            scroll_node: 1,
        }
    }

    fn ready_runtime() -> (JsRuntime, NativeScrollHandle, TargetKey) {
        ready_runtime_with_setup("")
    }

    fn ready_runtime_with_setup(setup: &str) -> (JsRuntime, NativeScrollHandle, TargetKey) {
        let mut runtime = JsRuntime::new().unwrap();
        runtime.set_viewport(200.0, 100.0);
        runtime
            .eval("document.body.setAttribute('style', 'height: 1000px')")
            .unwrap();
        if !setup.is_empty() {
            runtime.eval(setup).unwrap();
        }
        let _ = runtime.hit_test(1.0, 1.0);
        let (offset, extent) = runtime.host_state.borrow_mut().native_scroll_geometry();
        assert!(extent.max.y >= 10.0);

        let key = runtime_key();
        let handle = runtime.activate_native_scroll(key);
        let gate = handle.publication_gate();
        assert!(handle.authorize_policy(key, gate.policy_epoch));
        assert!(handle.install_scene(Scene {
            key,
            publication: PublicationSequence::new(1),
            policy_epoch: gate.policy_epoch,
            acceptance_epoch: gate.acceptance_epoch,
            acknowledged: 0,
            completed_fallback: FallbackTicket::default(),
            offset,
            extent,
            coverage: extent,
            eligible: true,
        }));
        (runtime, handle, key)
    }

    fn accept_vertical(handle: &NativeScrollHandle, delta: f64) -> AcceptedPresentation {
        match handle.scroll(Offset { x: 0.0, y: delta }) {
            ScrollDecision::Accepted(accepted) => accepted,
            other => panic!("expected accepted native scroll, got {other:?}"),
        }
    }

    #[test]
    fn canvas_pixels_preserve_committed_scene_for_unchanged_backing_dimensions() {
        let (mut runtime, handle, key) = ready_runtime_with_setup(
            "globalThis.testCanvas=document.createElement('canvas');             testCanvas.width=2;testCanvas.height=2;document.body.appendChild(testCanvas);             globalThis.testContext=testCanvas.getContext('2d');             testContext.fillStyle='red';testContext.fillRect(0,0,2,2)",
        );
        let before = handle.publication_gate();
        let paint_before = runtime.render_generations().paint;
        let accepted = accept_vertical(&handle, 10.0);
        runtime
            .eval(
                "testContext.fillStyle='blue';for(let i=0;i<20;i++){testContext.fillRect(0,0,2,2)}",
            )
            .unwrap();
        assert_eq!(
            runtime
                .eval("Array.from(testContext.getImageData(0,0,1,1).data).join(',')")
                .unwrap()
                .as_string()
                .unwrap()
                .to_std_string_escaped(),
            "0,0,255,255",
        );
        assert!(runtime.render_generations().paint > paint_before);
        assert_eq!(handle.publication_gate(), before);
        assert!(handle.validate_for_presentation(accepted));
        assert_eq!(handle.owner_sequences(key), Ok((0, 0)));
        assert_eq!(runtime.eval("scrollY").unwrap().as_number(), Some(10.0));
        assert_eq!(runtime.eval("scrollY").unwrap().as_number(), Some(10.0));
        assert_eq!(handle.owner_sequences(key), Ok((1, 0)));
        let (offset, extent) = runtime.host_state.borrow_mut().native_scroll_geometry();
        let notification = handle
            .claim_notification(key, offset, extent)
            .unwrap()
            .unwrap();
        assert_eq!(notification.input, accepted.input());
        assert_eq!(notification.offset, Offset { x: 0.0, y: 10.0 });
        assert_eq!(handle.claim_notification(key, offset, extent), Ok(None));
        assert_eq!(handle.owner_sequences(key), Ok((1, 1)));
    }

    #[test]
    fn canvas_backing_dimension_changes_and_first_commit_still_revoke() {
        for (setup, mutation) in [
            (
                "globalThis.testCanvas=document.createElement('canvas');                 testCanvas.width=2;testCanvas.height=2;document.body.appendChild(testCanvas);                 testCanvas.getContext('2d').fillRect(0,0,2,2)",
                "testCanvas.width=3",
            ),
            (
                "globalThis.testCanvas=document.createElement('canvas');document.body.appendChild(testCanvas)",
                "testCanvas.getContext('2d').fillRect(0,0,2,2)",
            ),
        ] {
            let (mut runtime, handle, key) = ready_runtime_with_setup(setup);
            let canvas_id = runtime
                .host_state
                .borrow()
                .nodes
                .values()
                .find(|node| node.has_tag_name("canvas"))
                .unwrap()
                .identity();
            if mutation.starts_with("testCanvas.getContext") {
                assert_eq!(crate::canvas::backing_dimensions(canvas_id), None);
            } else {
                assert_eq!(crate::canvas::backing_dimensions(canvas_id), Some((2, 2)));
            }
            let gate = handle.publication_gate();
            let accepted = accept_vertical(&handle, 10.0);
            runtime.eval(mutation).unwrap();
            assert!(
                handle.publication_gate().policy_epoch > gate.policy_epoch,
                "{mutation}"
            );
            assert!(!handle.validate_for_presentation(accepted), "{mutation}");
            assert_eq!(handle.notification_backlog(key), Ok(1));
            assert!(!handle.authorize_policy(key, gate.policy_epoch));
        }
    }

    #[test]
    fn canvas_native_backing_resize_revokes_without_attribute_mutation() {
        use super::super::super::{activate_host_state, canvas_commit_native};
        use boa_engine::{JsValue, object::builtins::JsUint8Array};

        let (mut runtime, handle, key) = ready_runtime_with_setup(
            "globalThis.testCanvas=document.createElement('canvas');             testCanvas.width=2;testCanvas.height=2;document.body.appendChild(testCanvas);             testCanvas.getContext('2d').fillRect(0,0,2,2)",
        );
        let canvas_id = runtime
            .host_state
            .borrow()
            .nodes
            .values()
            .find(|node| node.has_tag_name("canvas"))
            .unwrap()
            .identity();
        let gate = handle.publication_gate();
        let accepted = accept_vertical(&handle, 10.0);
        let pixels = JsUint8Array::from_iter([255u8; 24], &mut runtime.context).unwrap();
        {
            let _active = activate_host_state(runtime.host_state.clone());
            canvas_commit_native(
                &JsValue::undefined(),
                &[
                    JsValue::from(canvas_id as f64),
                    3.into(),
                    2.into(),
                    pixels.into(),
                ],
                &mut runtime.context,
            )
            .unwrap();
        }
        assert_eq!(
            runtime.eval("testCanvas.width").unwrap().as_number(),
            Some(2.0)
        );
        assert_eq!(crate::canvas::backing_dimensions(canvas_id), Some((3, 2)));
        assert!(handle.publication_gate().policy_epoch > gate.policy_epoch);
        assert!(!handle.validate_for_presentation(accepted));
        assert_eq!(handle.notification_backlog(key), Ok(1));
        assert!(!handle.authorize_policy(key, gate.policy_epoch));
    }

    #[test]
    fn detached_dom_mutations_preserve_scene_and_pending_native_default() {
        let (mut runtime, handle, key) = ready_runtime();
        let before = handle.publication_gate();
        let accepted = accept_vertical(&handle, 10.0);
        runtime.eval(
            "globalThis.detached=document.createElement('section');             detached.setAttribute('class','offscreen');             detached.setAttributeNS('urn:detached','d:mode','value');             detached.textContent='offscreen';             detached.innerHTML='<span>offscreen</span>';             const child=document.createElement('b');             detached.appendChild(child);             detached.insertBefore(document.createElement('i'),child);             detached.removeChild(child);             const canvas=document.createElement('canvas');             canvas.width=3;canvas.height=3;             canvas.getContext('2d').fillRect(0,0,3,3);"
        ).unwrap();
        assert_eq!(handle.publication_gate(), before);
        assert!(handle.validate_for_presentation(accepted));
        assert_eq!(handle.owner_sequences(key), Ok((0, 0)));
        assert_eq!(runtime.eval("scrollY").unwrap().as_number(), Some(10.0));
        assert_eq!(runtime.eval("scrollY").unwrap().as_number(), Some(10.0));
        assert_eq!(handle.owner_sequences(key), Ok((1, 0)));
    }

    #[test]
    fn live_dom_mutations_keep_the_committed_scene_scrollable() {
        for operation in [
            "document.body.appendChild(detached)",
            "detached.appendChild(live)",
            "detached.insertBefore(live,null)",
            "document.body.removeChild(live)",
            "live.setAttribute('class','changed')",
            "live.textContent='changed'",
            "live.innerHTML='<b>changed</b>'",
        ] {
            let (mut runtime, handle, _) = ready_runtime_with_setup(
                "globalThis.detached=document.createElement('section');                 globalThis.live=document.createElement('div');                 live.textContent='live';document.body.appendChild(live);",
            );
            let before = handle.publication_gate();
            runtime.eval(operation).unwrap();
            let after = handle.publication_gate();
            assert_eq!(after, before, "{operation}");
            assert!(matches!(
                handle.scroll(Offset { x: 0.0, y: 10.0 }),
                ScrollDecision::Accepted(_)
            ));
        }
    }

    #[test]
    fn connected_shadow_mutations_keep_the_committed_scene_scrollable() {
        let (mut runtime, handle, _) = ready_runtime_with_setup(
            "const host=document.createElement('div');document.body.appendChild(host);             globalThis.shadow=host.attachShadow({mode:'open'});             shadow.innerHTML='<div>live</div>';",
        );
        let before = handle.publication_gate();
        runtime
            .eval("shadow.firstElementChild.setAttribute('class','changed')")
            .unwrap();
        assert_eq!(handle.publication_gate(), before);
    }

    #[test]
    fn private_blocking_listener_hook_revokes_before_author_listener_is_visible() {
        let mut runtime = JsRuntime::new().unwrap();
        let key = runtime_key();
        let handle = runtime.activate_native_scroll(key);
        let initial = handle.publication_gate();
        assert!(handle.authorize_policy(key, initial.policy_epoch));

        runtime
            .eval("globalThis.addEventListener('wheel', () => {}, { passive: true })")
            .unwrap();
        assert_eq!(handle.publication_gate().policy_epoch, initial.policy_epoch);

        runtime
            .eval(
                "globalThis.__blocking = () => {}; \
                 globalThis.addEventListener('wheel', globalThis.__blocking, { passive: false })",
            )
            .unwrap();
        let revoked = handle.publication_gate();
        assert!(revoked.policy_epoch > initial.policy_epoch);
        assert!(!revoked.eligible);

        runtime
            .eval("globalThis.removeEventListener('wheel', globalThis.__blocking)")
            .unwrap();
        assert!(!handle.authorize_policy(key, revoked.policy_epoch));
    }

    #[test]
    fn cached_geometry_and_window_reads_skip_extent_without_pending_defaults() {
        let mut runtime = JsRuntime::new().unwrap();
        let _ = runtime.hit_test(1.0, 1.0);
        let (before_extent, before_adjusted) = {
            let state = runtime.host_state.borrow();
            (
                state.window_scroll_extent_calls,
                state.adjusted_layout_builds,
            )
        };

        let _ = runtime.hit_test(1.0, 1.0);
        runtime
            .eval("void window.scrollX; void visualViewport.offsetTop")
            .unwrap();

        let state = runtime.host_state.borrow();
        assert_eq!(state.window_scroll_extent_calls, before_extent);
        assert_eq!(state.adjusted_layout_builds, before_adjusted);
    }

    #[test]
    fn javascript_programmatic_setter_uses_the_native_mutation_gate() {
        let mut runtime = JsRuntime::new().unwrap();
        let key = runtime_key();
        let handle = runtime.activate_native_scroll(key);
        let before = handle.publication_gate().policy_epoch;
        runtime.eval("globalThis.scrollTo(0, 0)").unwrap();
        let after = handle.publication_gate();
        assert!(after.policy_epoch >= before + 2);
        assert!(!after.eligible);
    }

    #[test]
    fn viewport_change_keeps_owner_key_and_fifo_but_rejects_old_presentation() {
        let (mut runtime, handle, key) = ready_runtime();
        let accepted = accept_vertical(&handle, 10.0);
        let before = handle.publication_gate();

        runtime.set_viewport(201.0, 100.0);

        let after = handle.publication_gate();
        assert_eq!(after.key, key);
        assert!(after.policy_epoch > before.policy_epoch);
        assert!(!after.eligible);
        assert_eq!(handle.owner_sequences(key), Ok((0, 0)));
        assert_eq!(handle.notification_backlog(key), Ok(1));
        assert!(!handle.validate_for_presentation(accepted));
    }

    #[test]
    fn no_op_layout_and_visual_viewport_updates_keep_policy_epoch() {
        let (mut runtime, handle, key) = ready_runtime();
        runtime.set_viewport(201.0, 100.0);
        let layout_epoch = handle.publication_gate().policy_epoch;
        runtime.set_viewport(201.0, 100.0);
        assert_eq!(handle.publication_gate().policy_epoch, layout_epoch);

        runtime.set_visual_viewport(180.0, 90.0, 1.0, 2.0, 1.5);
        let visual_epoch = handle.publication_gate().policy_epoch;
        runtime.set_visual_viewport(180.0, 90.0, 1.0, 2.0, 1.5);
        let after = handle.publication_gate();
        assert_eq!(after.key, key);
        assert_eq!(after.policy_epoch, visual_epoch);
    }

    #[test]
    fn accepted_default_commit_preserves_policy_and_notification_fifo() {
        let (mut runtime, handle, key) = ready_runtime();
        let accepted = accept_vertical(&handle, 10.0);
        let before = handle.publication_gate();

        runtime.eval("void window.scrollY").unwrap();

        let after = handle.publication_gate();
        assert_eq!(after.key, key);
        assert_eq!(after.policy_epoch, before.policy_epoch);
        assert!(after.eligible);
        assert_eq!(runtime.host_state.borrow().window_scroll.1, 10.0);
        assert_eq!(handle.owner_sequences(key), Ok((1, 0)));
        assert_eq!(handle.notification_backlog(key), Ok(1));
        assert!(handle.validate_for_presentation(accepted));
    }

    #[test]
    fn live_dom_mutation_keeps_an_accepted_presentation_and_notification() {
        let (mut runtime, handle, key) = ready_runtime();
        let accepted = accept_vertical(&handle, 10.0);
        let before = handle.publication_gate();

        runtime
            .eval("document.body.setAttribute('data-native-scroll', 'changed')")
            .unwrap();

        let after = handle.publication_gate();
        assert_eq!(after.key, key);
        assert_eq!(after.policy_epoch, before.policy_epoch);
        assert!(after.eligible);
        assert_eq!(handle.owner_sequences(key), Ok((0, 0)));
        assert_eq!(handle.notification_backlog(key), Ok(1));
        assert!(handle.validate_for_presentation(accepted));
    }
}

mod render_invalidation_source_contract {
    const HOST: &str = include_str!("../mod.rs");
    const DOCUMENT_WRITE: &str = include_str!("../document_write.rs");
    const FONT_LOADING: &str = include_str!("../font_loading.rs");
    const FORM_VALIDATION: &str = include_str!("../form_validation.rs");
    const HTML_PARSING: &str = include_str!("../html_parsing.rs");
    const IMAGE_LOADING: &str = include_str!("../image_loading.rs");
    const MEDIA_ENVIRONMENT: &str = include_str!("../media_environment.rs");
    const RENDER_DEMAND: &str = include_str!("../render_demand.rs");

    fn section<'a>(source: &'a str, start: &str, end: &str) -> &'a str {
        let start = source
            .find(start)
            .unwrap_or_else(|| panic!("missing {start}"));
        let tail = &source[start..];
        let end = tail.find(end).unwrap_or_else(|| panic!("missing {end}"));
        &tail[..end]
    }

    fn assert_before(source: &str, first: &str, second: &str) {
        let first = source
            .find(first)
            .unwrap_or_else(|| panic!("missing first marker: {first}"));
        let second = source
            .find(second)
            .unwrap_or_else(|| panic!("missing second marker: {second}"));
        assert!(first < second, "{first} must precede {second}");
    }

    #[test]
    fn owned_helper_records_active_mutations_without_revoking_scroll() {
        let helper = section(
            HOST,
            "fn with_node_render_mutation",
            "fn with_canvas_render_mutation",
        );
        assert!(helper.contains("(state.native_scroll.clone(), affects)"));
        assert!(helper.contains("trace_render_mutation("));
        assert!(!helper.contains("native_scroll.revoke_policy();"));
        assert_before(helper, "trace_render_mutation(", "f(state)");

        let attribute = section(
            HOST,
            "fn set_attribute_native",
            "fn set_attribute_ns_native",
        );
        let last_conversion = attribute.rfind(".to_string(context)?").unwrap();
        let mutation = attribute
            .find("with_node_render_mutation(&[node_id]")
            .unwrap();
        assert!(last_conversion < mutation);

        for function in [
            "set_content_visibility_focus_native",
            "set_content_visibility_selection_native",
            "set_form_associated_custom_native",
            "set_element_scroll_native",
            "set_popover_open_native",
            "set_modal_dialog_native",
            "append_child_native",
            "commit_history_api_url_native",
            "commit_fragment_url_native",
            "register_property_native",
            "set_attribute_native",
            "set_attribute_ns_native",
            "set_checked_native",
            "set_option_selected_native",
            "set_text_control_state_native",
            "set_adopted_stylesheets_native",
            "canvas_commit_native",
            "set_text_content_native",
            "set_inner_html_native",
            "remove_child_native",
            "insert_before_native_impl",
            "set_user_action_target_native",
            "remove_attribute_native",
            "remove_attribute_ns_native",
            "attach_shadow_native",
            "document_reset_native",
            "document_write_native",
            "document_close_native",
        ] {
            let marker = format!("fn {function}");
            let start = HOST
                .find(&marker)
                .unwrap_or_else(|| panic!("missing {function}"));
            let body = &HOST[start..HOST.len().min(start + 8_000)];
            let boundary = match function {
                "append_child_native" | "remove_child_native" => {
                    "with_node_render_mutation(&[parent_id, child_id]"
                }
                "insert_before_native_impl" => "with_node_render_mutation(&[parent_id, new_id]",
                "set_attribute_native" | "set_attribute_ns_native" => {
                    "with_node_render_mutation(&[node_id]"
                }
                "canvas_commit_native" => "with_canvas_render_mutation(id, width, height",
                "set_text_content_native" | "set_inner_html_native" => {
                    "with_node_render_mutation(&[id]"
                }
                _ => "with_render_mutation",
            };
            assert!(
                body.contains(boundary),
                "{function} must use its active-target render mutation boundary"
            );
        }
    }

    #[test]
    fn canvas_helper_softens_only_existing_same_sized_backings() {
        let helper = section(
            HOST,
            "fn with_canvas_render_mutation",
            "fn trace_render_mutation",
        );
        assert!(helper.contains("node.has_tag_name(\"canvas\")"));
        assert!(
            helper.contains("crate::canvas::backing_dimensions(node_id) == Some((width, height))")
        );
        assert!(helper.contains("if affects_active_document && !preserves_layout"));
        assert!(
            helper.contains(
                "trace_render_mutation(caller, &native_scroll, affects_active_document);"
            )
        );
        assert_before(helper, "native_scroll.revoke_policy();", "f(state)");
    }

    #[test]
    fn viewport_replacement_revokes_in_place_without_activation() {
        let layout = section(HOST, "pub fn set_viewport", "pub fn set_visual_viewport");
        assert_before(
            layout,
            "native_scroll.revoke_policy();",
            "state.viewport = Rect",
        );
        assert!(!layout.contains("activate_native_scroll"));
        assert!(!layout.contains("native_scroll.activate("));

        let visual = section(HOST, "pub fn set_visual_viewport", "pub fn set_base_url");
        assert_before(
            visual,
            "if previous == next && !follows_layout",
            "native_scroll.revoke_policy();",
        );
        assert_before(
            visual,
            "native_scroll.revoke_policy();",
            "state.visual_viewport = next",
        );
        assert!(!visual.contains("activate_native_scroll"));
        assert!(!visual.contains("native_scroll.activate("));
    }

    #[test]
    fn accepted_root_commit_is_the_only_non_revoking_canonical_setter() {
        let materialize = section(
            HOST,
            "fn materialize_native_scroll_defaults",
            "fn prepare_native_programmatic_scroll",
        );
        assert!(materialize.contains("set_window_scroll_from_native_default"));
        let prepare = section(
            HOST,
            "fn prepare_native_programmatic_scroll",
            "fn finish_native_programmatic_scroll",
        );
        assert!(prepare.contains("set_window_scroll_from_native_default"));
        let setters = section(
            HOST,
            "fn set_window_scroll(&mut self",
            "fn element_scroll_offset",
        );
        assert_before(setters, "if revoke", "self.window_scroll = next");
        let paint = section(
            HOST,
            "fn invalidate_paint_cache",
            "fn invalidate_layout_root",
        );
        assert!(!paint.contains("revoke_policy"));
    }

    #[test]
    fn internal_scroll_routes_close_policy_without_native_hook_recursion() {
        let resnap = section(HOST, "fn resnap_after_layout", "fn scroll_document_to");
        assert!(resnap.contains("self.set_window_scroll(position.0, position.1)"));
        assert!(resnap.contains("self.set_element_scroll(&node, position.0, position.1)"));
        assert!(!resnap.contains("prepare_native_programmatic_scroll"));

        let bootstrap = super::super::super::DOM_BOOTSTRAP;
        let into_view = section(
            bootstrap,
            "scrollIntoView(options = true)",
            "focus(options)",
        );
        assert!(into_view.contains("(viewport ? view : container).scrollBy"));
        let focus = section(
            bootstrap,
            "commit(id, previousId",
            "function focusAcrossDocuments",
        );
        assert!(focus.contains("view.scrollBy"));
        let window_scroll = section(
            bootstrap,
            "function applyWindowScroll",
            "let queueIntersectionObserverChecks",
        );
        assert!(window_scroll.contains("__omoikane_set_window_scroll"));
    }

    #[test]
    fn external_render_owners_apply_their_declared_scene_boundaries() {
        let media = section(
            MEDIA_ENVIRONMENT,
            "pub fn set_media_environment",
            "fn media_document_ids",
        );
        assert_before(
            media,
            "if state.media_environment == environment",
            "native_scroll.revoke_policy();",
        );
        assert_before(
            media,
            "native_scroll.revoke_policy();",
            "state.media_environment = environment;",
        );

        let font = section(FONT_LOADING, "pub(super) fn native", "\n}");
        assert_before(
            font,
            "body_bytes_argument",
            "native_scroll.revoke_policy();",
        );
        assert_before(
            font,
            "let document = state.csp_document_for_context(context)?;",
            "native_scroll.revoke_policy();",
        );
        assert_before(
            font,
            "ok_or_else(|| error(\"unknown font\"))?",
            "native_scroll.revoke_policy();",
        );
        assert_before(
            font,
            "font belongs to a different origin",
            "native_scroll.revoke_policy();",
        );
        assert_before(
            font,
            "font document is no longer live",
            "native_scroll.revoke_policy();",
        );
        assert_before(
            font,
            "native_scroll.revoke_policy();",
            "match operation.as_str()",
        );

        let validation = section(
            FORM_VALIDATION,
            "pub(super) fn flush",
            "pub(super) fn register",
        );
        // Derived validity changes preserve the committed scene; author callbacks
        // still use their dedicated listener/navigation/scroll boundaries.
        assert!(!validation.contains("native_scroll.revoke_policy();"));
        assert_before(validation, "serde_json::from_str", "node.set_css_validity");
        assert_before(
            validation,
            "node.set_css_validity",
            "state.invalidate_document_style_cache(document)",
        );

        let parser = section(
            HTML_PARSING,
            "pub(super) fn fragment_native",
            "pub(super) fn document_native",
        );
        assert_before(
            parser,
            "state.node_is_in_active_document(&parsing_context)",
            "native_scroll.revoke_policy();",
        );
        assert_before(
            parser,
            "native_scroll.revoke_policy();",
            "parsing_context.attach_shadow",
        );

        let write = section(DOCUMENT_WRITE, "pub(super) fn write", "fn prepare_script");
        assert_before(
            write,
            "blocking_script.is_some()",
            "native_scroll.revoke_policy();",
        );
        assert_before(
            write,
            "native_scroll.revoke_policy();",
            ".parser.advance(eof)",
        );
        assert_eq!(write.matches("native_scroll.revoke_policy();").count(), 1);

        let image = section(
            IMAGE_LOADING,
            "pub(super) fn poll_image_resource_loads",
            "let waiting =",
        );
        assert!(image.contains("effects.changed"));
        assert!(image.contains("state.invalidate_paint_cache()"));
        assert!(!image.contains("native_scroll.revoke_policy();"));
    }

    #[test]
    fn smooth_scroll_clock_revokes_before_every_clock_advance() {
        let helper = section(
            RENDER_DEMAND,
            "fn revoke_native_scroll_for_render_clock_advance",
            "pub(crate) fn enable_live_css_animations",
        );
        assert!(helper.contains("!state.smooth_scrolls.is_empty()"));
        assert!(!helper.contains("visible_image_playbacks"));
        assert!(!helper.contains("has_running_animations"));
        assert_before(
            helper,
            "state.native_scroll.clone()",
            "native_scroll.revoke_policy();",
        );

        let enable = section(
            RENDER_DEMAND,
            "pub(crate) fn enable_live_css_animations",
            "pub(crate) fn paint_state_key",
        );
        assert_before(
            enable,
            "if state.live_css_animations",
            "native_scroll.revoke_policy();",
        );
        assert_before(
            enable,
            "native_scroll.revoke_policy();",
            "state.live_css_animations = true",
        );

        for start in [
            "pub fn tick",
            "pub fn advance_clock_without_tasks",
            "async fn run_animation_frame_async",
            "pub fn run_timers",
        ] {
            let block = section(HOST, start, "event_loop.advance");
            assert!(block.contains("revoke_native_scroll_for_render_clock_advance"));
        }
    }
}
