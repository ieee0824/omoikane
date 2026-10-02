use super::*;
use crate::paint::Color;
use base64::Engine;

fn page(html: &str) -> CdpSession {
    let mut session = CdpSession::new().unwrap();
    let encoded: String = html.bytes().map(|byte| format!("%{byte:02X}")).collect();
    session
        .dispatch(
            "Page.navigate",
            json!({"url":format!("data:text/html,{encoded}")}),
        )
        .unwrap();
    session.set_viewport(200, 100);
    session.drive_event_loop(0).unwrap();
    session
}

fn eval(session: &mut CdpSession, script: &str) {
    session
        .dispatch("Runtime.evaluate", json!({"expression":script}))
        .unwrap();
}

fn gif_uri(infinite: bool) -> String {
    let mut bytes = Vec::new();
    {
        let mut encoder = gif::Encoder::new(&mut bytes, 1, 1, &[255, 0, 0, 0, 0, 255]).unwrap();
        if infinite {
            encoder.set_repeat(gif::Repeat::Infinite).unwrap();
        }
        for index in [0, 1] {
            let frame = gif::Frame {
                width: 1,
                height: 1,
                delay: 10,
                buffer: std::borrow::Cow::Owned(vec![index]),
                ..gif::Frame::default()
            };
            encoder.write_frame(&frame).unwrap();
        }
    }
    format!(
        "data:image/gif;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    )
}

#[test]
fn visible_gif_uses_retained_layout_and_stops_after_its_finite_playback() {
    for infinite in [false, true] {
        let mut session = page(&format!(
            "<style>body{{margin:0}}img{{position:absolute;left:0;top:0;width:100px;height:100px}}</style><img src='{}'>",
            gif_uri(infinite)
        ));
        assert_eq!(
            session.paint_current_document().unwrap().pixel(10, 10),
            Some(Color::rgb(255, 0, 0))
        );
        let first = session.paint_state_key();
        assert_eq!(
            session.render_demand(Some(&first)).next,
            NextRendering::EveryFrame
        );
        session.drive_event_loop(99).unwrap();
        assert!(!session.render_demand(Some(&first)).needs_paint);
        session.drive_event_loop(1).unwrap();
        assert!(session.render_demand(Some(&first)).needs_paint);
        assert_eq!(
            session.paint_current_document().unwrap().pixel(10, 10),
            Some(Color::rgb(0, 0, 255))
        );
        let second = session.paint_state_key();
        session.drive_event_loop(100).unwrap();
        assert_eq!(
            session.render_demand(Some(&second)).next,
            if infinite {
                NextRendering::EveryFrame
            } else {
                NextRendering::Idle
            }
        );
        assert_eq!(
            session.paint_current_document().unwrap().pixel(10, 10),
            Some(if infinite {
                Color::rgb(255, 0, 0)
            } else {
                Color::rgb(0, 0, 255)
            })
        );
    }
}

#[test]
fn invisible_gifs_do_not_keep_the_page_awake() {
    for style in [
        "display:none",
        "visibility:hidden",
        "opacity:0",
        "left:300px",
        "transform:translateX(300px)",
    ] {
        let mut session = page(&format!(
            "<style>body{{margin:0}}img{{position:absolute;top:0;left:0;width:100px;height:100px;{style}}}</style><img src='{}'>",
            gif_uri(true)
        ));
        session.paint_current_document().unwrap();
        assert_eq!(
            session.render_demand(Some(&session.paint_state_key())).next,
            NextRendering::Idle,
            "{style}"
        );
    }
}

#[test]
fn animated_masks_change_the_paint_key_and_request_rendering() {
    let uri = gif_uri(true);
    let mut session = page(&format!(
        "<style>body{{margin:0}}div{{width:100px;height:100px;background:green;mask-image:url('{uri}');mask-mode:luminance;mask-size:100% 100%;mask-repeat:no-repeat}}</style><div></div>"
    ));
    let first = session.paint_current_document().unwrap().pixel(10, 10);
    let presented = session.paint_state_key();
    assert_eq!(
        session.render_demand(Some(&presented)).next,
        NextRendering::EveryFrame
    );
    session.drive_event_loop(100).unwrap();
    assert!(session.render_demand(Some(&presented)).needs_paint);
    assert_ne!(
        session.paint_current_document().unwrap().pixel(10, 10),
        first
    );
}

#[test]
fn animated_backgrounds_and_child_snapshots_preserve_visible_playback() {
    let uri = gif_uri(true);
    for html in [
        format!(
            "<style>body{{margin:0;width:100px;height:100px;background-image:url('{uri}')}}</style>"
        ),
        format!(
            "<style>body{{margin:0}}iframe{{border:0;width:100px;height:100px}}</style><iframe srcdoc=\"<style>body{{margin:0}}img{{position:absolute;top:0;left:0;width:100px;height:100px}}</style><img src='{uri}'>\"></iframe>"
        ),
    ] {
        let mut session = page(&html);
        assert_eq!(
            session.paint_current_document().unwrap().pixel(10, 10),
            Some(Color::rgb(255, 0, 0))
        );
        let first = session.paint_state_key();
        assert_eq!(
            session.render_demand(Some(&first)).next,
            NextRendering::EveryFrame
        );
        session.drive_event_loop(100).unwrap();
        assert!(session.render_demand(Some(&first)).needs_paint);
        assert_eq!(
            session.paint_current_document().unwrap().pixel(10, 10),
            Some(Color::rgb(0, 0, 255))
        );
    }
}

#[test]
fn smooth_scrolling_requests_frames_until_reaching_the_target() {
    let mut session = page("<style>body{margin:0;height:400px;background:red}</style>");
    session.paint_current_document().unwrap();
    let presented = session.paint_state_key();
    eval(&mut session, "window.scrollTo({top:100,behavior:'smooth'})");
    assert_eq!(
        session.render_demand(Some(&presented)).next,
        NextRendering::EveryFrame
    );
    session.drive_event_loop(150).unwrap();
    assert!(session.render_demand(Some(&presented)).needs_paint);
    assert_eq!(
        session.render_demand(Some(&presented)).next,
        NextRendering::EveryFrame
    );
    session.drive_event_loop(1000).unwrap();
    assert_eq!(
        session.render_demand(Some(&presented)).next,
        NextRendering::Idle
    );
    let result = session
        .dispatch("Runtime.evaluate", json!({"expression":"window.scrollY"}))
        .unwrap();
    assert_eq!(result["result"]["value"], json!(100));
}

#[test]
fn worker_timer_work_is_polled_until_completion() {
    let mut session = page("<body></body>");
    eval(
        &mut session,
        "window.done=false;window.worker=new Worker('data:text/javascript,'+encodeURIComponent('setTimeout(()=>postMessage(42),100)'));worker.onmessage=()=>done=true",
    );
    assert_eq!(session.render_demand(None).next, NextRendering::EveryFrame);
    session.drive_event_loop(99).unwrap();
    assert_eq!(session.render_demand(None).next, NextRendering::EveryFrame);
    session.drive_event_loop(1).unwrap();
    let result = session
        .dispatch("Runtime.evaluate", json!({"expression":"done"}))
        .unwrap();
    assert_eq!(result["result"]["value"], json!(true));
    assert_eq!(session.render_demand(None).next, NextRendering::Idle);
}

#[test]
fn completed_navigation_module_jobs_do_not_keep_the_page_awake() {
    use crate::test_support::http_fixture::{
        ACCEPT_TIMEOUT, READ_TIMEOUT, accept_with_timeout, bind_loopback, read_request_headers,
    };
    use std::io::Write;
    let listener = bind_loopback().unwrap();
    let url = format!("http://{}/page", listener.local_addr().unwrap());
    let server = std::thread::spawn(move || {
        for path in ["/page", "/module.js"] {
            let mut stream = accept_with_timeout(&listener, ACCEPT_TIMEOUT).unwrap();
            let request = read_request_headers(&mut stream, READ_TIMEOUT).unwrap();
            assert!(request.starts_with(&format!("GET {path} ")), "{request}");
            let (mime, body) = if path == "/page" {
                (
                    "text/html",
                    "<style>body{margin:0;min-height:100vh;background:red}</style><script>import('./module.js').then(()=>document.body.style.background='blue')</script>",
                )
            } else {
                ("text/javascript", "export const loaded=true;")
            };
            write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: {mime}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).unwrap();
        }
    });
    let mut session = CdpSession::new().unwrap();
    // Native navigation retains its existing synchronous job-drain contract.
    // Suspended downloads are tested separately at the native demand boundary.
    session
        .dispatch("Page.navigate", json!({"url":url}))
        .unwrap();
    server.join().unwrap();
    session.set_viewport(200, 100);
    session.drive_event_loop(0).unwrap();
    assert_eq!(
        session.paint_current_document().unwrap().pixel(100, 80),
        Some(Color::rgb(0, 0, 255))
    );
    let presented = session.paint_state_key();
    assert_eq!(
        session.render_demand(Some(&presented)),
        RenderDemand {
            needs_paint: false,
            next: NextRendering::Idle
        }
    );
}

#[test]
fn static_page_query_is_idle_and_leaves_state_unchanged() {
    let mut session =
        page("<style>body{margin:0;min-height:100vh;background:red}</style><p>static</p>");
    assert!(session.render_demand(None).needs_paint);
    session.paint_current_document().unwrap();
    let presented = session.paint_state_key();
    for _ in 0..10 {
        assert_eq!(
            session.render_demand(Some(&presented)),
            RenderDemand {
                needs_paint: false,
                next: NextRendering::Idle
            }
        );
        assert_eq!(session.paint_state_key(), presented);
    }
    session.drive_event_loop(500).unwrap();
    assert!(!session.render_demand(Some(&presented)).needs_paint);
}

#[test]
fn timers_report_earliest_remaining_deadline_without_firing_early() {
    let mut session = page("<style>body{margin:0;min-height:100vh;background:red}</style>");
    session.paint_current_document().unwrap();
    let presented = session.paint_state_key();
    eval(
        &mut session,
        "setTimeout(()=>document.body.style.background='blue',500);window.short=setTimeout(()=>{},200)",
    );
    assert_eq!(
        session.render_demand(Some(&presented)).next,
        NextRendering::After(Duration::from_millis(200))
    );
    eval(&mut session, "clearTimeout(short)");
    assert_eq!(
        session.render_demand(Some(&presented)).next,
        NextRendering::After(Duration::from_millis(500))
    );
    session.drive_event_loop(499).unwrap();
    assert_eq!(
        session.render_demand(Some(&presented)),
        RenderDemand {
            needs_paint: false,
            next: NextRendering::After(Duration::from_millis(1))
        }
    );
    session.drive_event_loop(1).unwrap();
    assert_eq!(
        session.render_demand(Some(&presented)),
        RenderDemand {
            needs_paint: true,
            next: NextRendering::Idle
        }
    );
    assert_eq!(
        session.paint_current_document().unwrap().pixel(100, 80),
        Some(Color::rgb(0, 0, 255))
    );
}

#[test]
fn animation_frame_loop_requests_frames_until_cancelled_and_pauses_when_hidden() {
    let mut session = page("<body></body>");
    session.paint_current_document().unwrap();
    let presented = session.paint_state_key();
    eval(
        &mut session,
        "function loop(){window.id=requestAnimationFrame(loop)};window.id=requestAnimationFrame(loop)",
    );
    assert_eq!(
        session.render_demand(Some(&presented)).next,
        NextRendering::EveryFrame
    );
    session.drive_event_loop(16).unwrap();
    assert_eq!(
        session.render_demand(Some(&presented)).next,
        NextRendering::EveryFrame
    );
    session.set_host_visibility(true);
    assert_eq!(
        session.render_demand(Some(&presented)).next,
        NextRendering::Idle
    );
    session.set_host_visibility(false);
    eval(&mut session, "cancelAnimationFrame(id)");
    session.drive_event_loop(0).unwrap();
    assert_eq!(
        session.render_demand(Some(&presented)).next,
        NextRendering::Idle
    );
}

#[test]
fn paint_key_covers_dom_scroll_viewports_focus_and_caret() {
    for script in [
        "document.body.style.background='blue'",
        "document.body.appendChild(document.createElement('p'))",
        "window.scrollTo(0,40)",
        "document.getElementById('field').focus()",
        "document.getElementById('field').setSelectionRange(1,1)",
    ] {
        let mut session = page(
            "<style>body{margin:0;min-height:2000px;background:red}</style><input id='field' value='abcd'>",
        );
        eval(&mut session, "document.getElementById('field').focus()");
        if script.ends_with("focus()") {
            eval(&mut session, "document.getElementById('field').blur()");
        }
        session.paint_current_document().unwrap();
        let presented = session.paint_state_key();
        eval(&mut session, script);
        assert!(
            session.render_demand(Some(&presented)).needs_paint,
            "{script}"
        );
    }
    let mut session = page("<body></body>");
    session.paint_current_document().unwrap();
    let presented = session.paint_state_key();
    session.set_viewport(300, 120);
    assert!(session.render_demand(Some(&presented)).needs_paint);
    session.drive_event_loop(0).unwrap();
    session.paint_current_document().unwrap();
    let presented = session.paint_state_key();
    session.set_visual_viewport(150.0, 60.0, 10.0, 10.0, 2.0);
    assert!(session.render_demand(Some(&presented)).needs_paint);
}

#[test]
fn finite_keyframes_use_page_time_and_become_idle_after_the_final_paint() {
    let mut session = page(
        "<style>body{margin:0;min-height:100vh;animation:color 1s linear forwards}@keyframes color{from{background-color:red}to{background-color:blue}}</style>",
    );
    assert_eq!(
        session.paint_current_document().unwrap().pixel(100, 80),
        Some(Color::rgb(255, 0, 0))
    );
    let presented = session.paint_state_key();
    assert_eq!(
        session.render_demand(Some(&presented)).next,
        NextRendering::EveryFrame
    );
    session.drive_event_loop(500).unwrap();
    assert!(session.render_demand(Some(&presented)).needs_paint);
    assert_eq!(
        session.paint_current_document().unwrap().pixel(100, 80),
        Some(Color::rgb(128, 0, 128))
    );
    session.drive_event_loop(500).unwrap();
    assert_eq!(
        session.paint_current_document().unwrap().pixel(100, 80),
        Some(Color::rgb(0, 0, 255))
    );
    let presented = session.paint_state_key();
    assert_eq!(
        session.render_demand(Some(&presented)),
        RenderDemand {
            needs_paint: false,
            next: NextRendering::Idle
        }
    );
    session.drive_event_loop(1000).unwrap();
    assert_eq!(
        session.render_demand(Some(&presented)),
        RenderDemand {
            needs_paint: false,
            next: NextRendering::Idle
        }
    );
}

#[test]
fn infinite_keyframes_pause_resume_and_cancel_without_resetting_progress() {
    let mut session = page(
        "<style>body{margin:0;min-height:100vh;animation:color 1s linear infinite}@keyframes color{from{background-color:red}to{background-color:blue}}</style>",
    );
    session.paint_current_document().unwrap();
    session.drive_event_loop(500).unwrap();
    assert_eq!(
        session.paint_current_document().unwrap().pixel(100, 80),
        Some(Color::rgb(128, 0, 128))
    );
    eval(
        &mut session,
        "document.body.style.animationPlayState='paused'",
    );
    session.drive_event_loop(0).unwrap();
    session.paint_current_document().unwrap();
    let presented = session.paint_state_key();
    assert_eq!(
        session.render_demand(Some(&presented)).next,
        NextRendering::Idle
    );
    session.drive_event_loop(500).unwrap();
    assert!(!session.render_demand(Some(&presented)).needs_paint);
    eval(
        &mut session,
        "document.body.style.animationPlayState='running'",
    );
    session.drive_event_loop(0).unwrap();
    assert_eq!(
        session.paint_current_document().unwrap().pixel(100, 80),
        Some(Color::rgb(128, 0, 128))
    );
    assert_eq!(session.render_demand(None).next, NextRendering::EveryFrame);
    session.drive_event_loop(500).unwrap();
    assert_eq!(
        session.paint_current_document().unwrap().pixel(100, 80),
        Some(Color::rgb(255, 0, 0))
    );
    eval(&mut session, "document.body.style.animationName='none'");
    session.drive_event_loop(0).unwrap();
    session.paint_current_document().unwrap();
    assert_eq!(session.render_demand(None).next, NextRendering::Idle);
}

#[test]
fn shorthand_delay_reverse_and_finite_iterations_follow_the_live_clock() {
    let mut session = page(
        "<style>body{margin:0;min-height:100vh;animation:color 1s 200ms linear 2 reverse both}@keyframes color{from{background-color:red}to{background-color:blue}}</style>",
    );
    assert_eq!(
        session.paint_current_document().unwrap().pixel(100, 80),
        Some(Color::rgb(0, 0, 255))
    );
    session.drive_event_loop(700).unwrap();
    assert_eq!(
        session.paint_current_document().unwrap().pixel(100, 80),
        Some(Color::rgb(128, 0, 128))
    );
    assert_eq!(session.render_demand(None).next, NextRendering::EveryFrame);
    session.drive_event_loop(1500).unwrap();
    assert_eq!(
        session.paint_current_document().unwrap().pixel(100, 80),
        Some(Color::rgb(255, 0, 0))
    );
    assert_eq!(session.render_demand(None).next, NextRendering::Idle);
}

#[test]
fn css_transitions_request_frames_until_completion() {
    let mut session = page(
        "<style>body{margin:0;min-height:100vh;background-color:red;transition:background-color 1s linear}</style>",
    );
    session.paint_current_document().unwrap();
    eval(&mut session, "document.body.style.backgroundColor='blue'");
    session.drive_event_loop(0).unwrap();
    assert_eq!(session.render_demand(None).next, NextRendering::EveryFrame);
    session.drive_event_loop(500).unwrap();
    assert_eq!(
        session.paint_current_document().unwrap().pixel(100, 80),
        Some(Color::rgb(128, 0, 128))
    );
    session.drive_event_loop(500).unwrap();
    session.paint_current_document().unwrap();
    let presented = session.paint_state_key();
    assert_eq!(
        session.render_demand(Some(&presented)),
        RenderDemand {
            needs_paint: false,
            next: NextRendering::Idle
        }
    );
}

#[test]
fn setting_an_unchanged_viewport_preserves_the_presented_key() {
    let mut session = page("<body>static</body>");
    session.paint_current_document().unwrap();
    let presented = session.paint_state_key();
    session.set_viewport(200, 100);
    assert_eq!(session.paint_state_key(), presented);
}

#[test]
fn uppercase_paused_keyword_stops_live_keyframes() {
    let mut session = page(
        "<style>body{margin:0;min-height:100vh;animation:color 1s linear infinite;animation-play-state:PAUSED}@keyframes color{from{background-color:red}to{background-color:blue}}</style>",
    );
    assert_eq!(
        session.paint_current_document().unwrap().pixel(100, 80),
        Some(Color::rgb(255, 0, 0))
    );
    let presented = session.paint_state_key();
    assert_eq!(
        session.render_demand(Some(&presented)).next,
        NextRendering::Idle
    );
    session.drive_event_loop(500).unwrap();
    assert_eq!(
        session.paint_current_document().unwrap().pixel(100, 80),
        Some(Color::rgb(255, 0, 0))
    );
    eval(
        &mut session,
        "document.body.style.animationPlayState='RUNNING'",
    );
    session.paint_current_document().unwrap();
    session.drive_event_loop(500).unwrap();
    assert_eq!(
        session.paint_current_document().unwrap().pixel(100, 80),
        Some(Color::rgb(128, 0, 128))
    );
}

#[test]
fn uppercase_none_keyword_removes_live_keyframes() {
    let mut session = page(
        "<style>body{margin:0;min-height:100vh;animation:color 1s linear infinite}@keyframes color{from{background-color:red}to{background-color:blue}}</style>",
    );
    session.paint_current_document().unwrap();
    assert_eq!(session.render_demand(None).next, NextRendering::EveryFrame);
    eval(&mut session, "document.body.style.display='NONE'");
    session.paint_current_document().unwrap();
    let presented = session.paint_state_key();
    assert_eq!(
        session.render_demand(Some(&presented)).next,
        NextRendering::Idle
    );
}
