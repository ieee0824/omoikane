use super::*;
use serde_json::json;

fn demand(next: NextRendering) -> RenderDemand {
    RenderDemand {
        needs_paint: false,
        next,
    }
}

#[test]
fn scheduling_is_pure_and_anchors_timer_deadlines_across_os_wakeups() {
    let origin = Instant::now();
    let mut scheduler = PlatformFrameScheduler::new(origin, Duration::from_millis(16));
    assert_eq!(
        scheduler.plan(origin, demand(NextRendering::Idle)),
        PlatformFramePlan::Redraw
    );
    assert!(scheduler.queue_redraw());
    assert!(!scheduler.queue_redraw());
    assert_eq!(
        scheduler.plan(origin, demand(NextRendering::EveryFrame)),
        PlatformFramePlan::Wait
    );
    scheduler.begin_frame(origin);
    assert_eq!(
        scheduler.plan(origin, demand(NextRendering::Idle)),
        PlatformFramePlan::Wait
    );
    assert_eq!(
        scheduler.plan(origin, demand(NextRendering::EveryFrame)),
        PlatformFramePlan::WaitUntil(origin + Duration::from_millis(16))
    );
    let timer = demand(NextRendering::After(Duration::from_millis(500)));
    for elapsed in [0, 100, 499] {
        assert_eq!(
            scheduler.plan(origin + Duration::from_millis(elapsed), timer),
            PlatformFramePlan::WaitUntil(origin + Duration::from_millis(500))
        );
    }
    assert_eq!(
        scheduler.plan(origin + Duration::from_millis(500), timer),
        PlatformFramePlan::Redraw
    );
    scheduler.advance_time(origin + Duration::from_millis(300));
    assert_eq!(
        scheduler.plan(
            origin + Duration::from_millis(400),
            demand(NextRendering::After(Duration::from_millis(200)))
        ),
        PlatformFramePlan::WaitUntil(origin + Duration::from_millis(500))
    );
    scheduler.request_rendering_opportunity(origin + Duration::from_millis(400));
    assert_eq!(
        scheduler.plan(
            origin + Duration::from_millis(400),
            demand(NextRendering::Idle)
        ),
        PlatformFramePlan::Redraw
    );
}

fn navigate(session: &mut CdpSession, html: &str) {
    let encoded: String = html.bytes().map(|byte| format!("%{byte:02X}")).collect();
    session
        .dispatch(
            "Page.navigate",
            json!({"url": format!("data:text/html,{encoded}")}),
        )
        .unwrap();
}

fn eval(session: &mut CdpSession, expression: &str) -> serde_json::Value {
    session
        .dispatch("Runtime.evaluate", json!({"expression": expression}))
        .unwrap()["result"]["value"]
        .clone()
}

#[test]
fn frame_cache_skips_stable_pages_and_refreshes_dom_scroll_resize_and_navigation() {
    let mut session = CdpSession::new().unwrap();
    navigate(
        &mut session,
        "<style>body{margin:0;min-height:300px;background:red}</style>",
    );
    let mut cache = BrowserFrameCache::default();
    assert!(cache.update(&mut session, 100, 100, 0).unwrap());
    let retained_pixels = cache.frame().unwrap().pixels().as_ptr();
    for _ in 0..10 {
        assert!(!cache.update(&mut session, 100, 100, 16).unwrap());
        assert_eq!(cache.frame().unwrap().pixels().as_ptr(), retained_pixels);
        assert_eq!(cache.demand(&session).next, NextRendering::Idle);
    }
    eval(&mut session, "document.body.style.background='blue'");
    assert!(cache.update(&mut session, 100, 100, 0).unwrap());
    assert_eq!(&cache.frame().unwrap().pixels()[..4], &[0, 0, 255, 255]);
    eval(&mut session, "window.scrollTo(0,50)");
    assert!(cache.update(&mut session, 100, 100, 0).unwrap());
    assert!(cache.update(&mut session, 120, 80, 0).unwrap());
    assert_eq!(
        (
            cache.frame().unwrap().width(),
            cache.frame().unwrap().height()
        ),
        (120, 80)
    );
    navigate(
        &mut session,
        "<style>body{margin:0;min-height:100vh;background:green}</style>",
    );
    assert!(cache.update(&mut session, 120, 80, 0).unwrap());
    assert_eq!(&cache.frame().unwrap().pixels()[..4], &[0, 128, 0, 255]);
}

#[test]
fn native_input_after_idle_starts_timers_now_without_delivering_extra_raf() {
    let mut session = CdpSession::new().unwrap();
    navigate(
        &mut session,
        "<body><script>window.frames=0;requestAnimationFrame(()=>frames++)</script>",
    );
    session.advance_tasks(10_000).unwrap();
    assert_eq!(eval(&mut session, "frames"), json!(0));
    eval(
        &mut session,
        "window.fired=false;setTimeout(()=>fired=true,500)",
    );
    session.drive_event_loop(0).unwrap();
    assert_eq!(eval(&mut session, "frames"), json!(1));
    session.drive_event_loop(499).unwrap();
    assert_eq!(eval(&mut session, "fired"), json!(false));
    session.drive_event_loop(1).unwrap();
    assert_eq!(eval(&mut session, "fired"), json!(true));
}
