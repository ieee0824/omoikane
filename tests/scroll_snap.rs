use omoikane::{html::TreeBuilder, js::JsRuntime};

fn runtime(html: &str) -> JsRuntime {
    JsRuntime::with_document_and_url(
        TreeBuilder::parse(html).document(),
        "https://example.test/scroll-snap.html",
    )
    .expect("create JavaScript runtime")
}

fn check(runtime: &mut JsRuntime, source: &str) {
    assert!(
        runtime
            .eval(source)
            .unwrap_or_else(|error| panic!("{source}: {error}"))
            .to_boolean(),
        "{source}"
    );
}

#[test]
fn mandatory_element_scroll_snaps_to_nearest_item_and_queues_one_event() {
    let mut runtime = runtime(
        r#"<style>
          html, body { margin: 0; }
          #scroller { width: 100px; height: 100px; overflow: auto;
                      scroll-snap-type: y mandatory; }
          .item { height: 100px; scroll-snap-align: start; }
        </style>
        <div id="scroller"><div class="item"></div><div class="item"></div>
          <div class="item"></div></div>"#,
    );

    check(
        &mut runtime,
        r#"(() => {
          const scroller = document.getElementById("scroller");
          globalThis.scrollEvents = 0;
          scroller.addEventListener("scroll", () => scrollEvents++);
          scroller.scrollTo(0, 65);
          return scroller.scrollTop === 100 && scrollEvents === 0;
        })()"#,
    );
    runtime.run_animation_frame(16).unwrap();
    check(&mut runtime, "scrollEvents === 1");
    check(
        &mut runtime,
        r#"(() => {
          const scroller = document.getElementById("scroller");
          scroller.scrollTo(0, 15);
          return scroller.scrollTop === 0;
        })()"#,
    );
}

#[test]
fn wheel_input_uses_the_same_snap_selection_as_scripted_scroll() {
    let mut runtime = runtime(
        r#"<style>
          html, body { margin: 0; }
          #scroller { width: 100px; height: 100px; overflow: auto;
                      scroll-snap-type: y mandatory; }
          .item { height: 100px; scroll-snap-align: start; }
        </style>
        <div id="scroller"><div class="item"></div><div class="item"></div>
          <div class="item"></div></div>"#,
    );
    check(
        &mut runtime,
        r#"(() => {
          const scroller = document.getElementById("scroller");
          const accepted = __omoikane_dispatch_wheel_input(scroller.__id,
            {deltaX: 0, deltaY: 65});
          return accepted !== 0 && scroller.scrollTop === 100;
        })()"#,
    );
}

#[test]
fn scroll_padding_and_margin_shift_snap_position() {
    let mut runtime = runtime(
        r#"<style>
          html, body { margin: 0; }
          #scroller { width: 100px; height: 100px; overflow: auto;
                      scroll-snap-type: y mandatory; scroll-padding-top: 20px; }
          .item { height: 100px; scroll-snap-align: start; }
          #second { scroll-margin-top: 10px; }
        </style>
        <div id="scroller"><div class="item"></div><div class="item" id="second"></div>
          <div class="item"></div></div>"#,
    );
    check(
        &mut runtime,
        r#"(() => {
          const scroller = document.getElementById("scroller");
          scroller.scrollTo(0, 65);
          return scroller.scrollTop === 70;
        })()"#,
    );
}

#[test]
fn proximity_leaves_distant_positions_free_but_captures_nearby_positions() {
    let mut runtime = runtime(
        r#"<style>
          html, body { margin: 0; }
          #scroller { width: 100px; height: 100px; overflow: auto;
                      scroll-snap-type: y proximity; }
          .item { height: 100px; scroll-snap-align: start; }
        </style>
        <div id="scroller"><div class="item"></div><div class="item"></div>
          <div class="item"></div></div>"#,
    );
    check(
        &mut runtime,
        r#"(() => {
          const scroller = document.getElementById("scroller");
          scroller.scrollTo(0, 60);
          if (scroller.scrollTop !== 60) return false;
          scroller.scrollTo(0, 85);
          return scroller.scrollTop === 100;
        })()"#,
    );
}

#[test]
fn nested_scrollers_select_only_their_own_snap_areas() {
    let mut runtime = runtime(
        r#"<style>
          html, body { margin: 0; }
          #outer, #inner { width: 100px; height: 100px; overflow: auto;
                           scroll-snap-type: y mandatory; }
          .outer-item, .inner-item { height: 100px; scroll-snap-align: start; }
        </style>
        <div id="outer"><div class="outer-item"></div>
          <div class="outer-item"><div id="inner"><div class="inner-item"></div>
            <div class="inner-item"></div><div class="inner-item"></div></div></div>
          <div class="outer-item"></div></div>"#,
    );
    check(
        &mut runtime,
        r#"(() => {
          const outer = document.getElementById("outer");
          const inner = document.getElementById("inner");
          outer.scrollTo(0, 65);
          inner.scrollTo(0, 65);
          return outer.scrollTop === 100 && inner.scrollTop === 100;
        })()"#,
    );
}

#[test]
fn oversized_area_allows_free_scroll_within_its_visible_range() {
    let mut runtime = runtime(
        r#"<style>
          html, body { margin: 0; }
          #scroller { width: 100px; height: 100px; overflow: auto;
                      scroll-snap-type: y mandatory; }
          #large { height: 200px; scroll-snap-align: start; }
          #last { height: 100px; scroll-snap-align: start; }
        </style>
        <div id="scroller"><div id="large"></div><div id="last"></div></div>"#,
    );
    check(
        &mut runtime,
        r#"(() => {
          const scroller = document.getElementById("scroller");
          scroller.scrollTo(0, 60);
          if (scroller.scrollTop !== 60) return false;
          scroller.scrollTo(0, 120);
          return scroller.scrollTop === 100;
        })()"#,
    );
}

#[test]
fn layout_change_resnaps_to_the_same_element() {
    let mut runtime = runtime(
        r#"<style>
          html, body { margin: 0; }
          #scroller { width: 100px; height: 100px; overflow: auto;
                      scroll-snap-type: y mandatory; }
          .item { height: 100px; scroll-snap-align: start; }
        </style>
        <div id="scroller"><div id="first" class="item"></div>
          <div class="item"></div><div class="item"></div></div>"#,
    );
    check(
        &mut runtime,
        r#"(() => {
          const scroller = document.getElementById("scroller");
          globalThis.scrollEvents = 0;
          scroller.addEventListener("scroll", () => scrollEvents++);
          scroller.scrollTo(0, 65);
          return scroller.scrollTop === 100;
        })()"#,
    );
    runtime.run_animation_frame(16).unwrap();
    check(&mut runtime, "scrollEvents === 1");
    check(
        &mut runtime,
        r#"(() => {
          scrollEvents = 0;
          document.getElementById("first").style.height = "120px";
          return document.getElementById("scroller").scrollTop === 120 && scrollEvents === 0;
        })()"#,
    );
    runtime.run_animation_frame(32).unwrap();
    check(&mut runtime, "scrollEvents === 1");
}

#[test]
fn rtl_start_alignment_uses_the_right_edge() {
    let mut runtime = runtime(
        r#"<style>
          html, body { margin: 0; }
          #scroller { position: relative; width: 100px; height: 100px; overflow: auto;
                      direction: rtl; scroll-snap-type: inline mandatory; }
          #space { position: absolute; left: 0; top: 0; width: 300px; height: 100px; }
          #target { position: absolute; left: 100px; top: 0; width: 50px; height: 50px;
                    scroll-snap-align: start; }
        </style>
        <div id="scroller"><div id="space"></div><div id="target"></div></div>"#,
    );
    check(
        &mut runtime,
        r#"(() => {
          const scroller = document.getElementById("scroller");
          scroller.scrollTo(65, 0);
          return scroller.scrollLeft === 50;
        })()"#,
    );
}

#[test]
fn vertical_writing_inline_axis_respects_rtl_start() {
    let mut runtime = runtime(
        r#"<style>
          html, body { margin: 0; }
          #scroller { position: relative; width: 100px; height: 100px; overflow: auto;
                      writing-mode: vertical-rl; direction: rtl;
                      scroll-snap-type: inline mandatory; }
          #space { position: absolute; left: 0; top: 0; width: 100px; height: 300px; }
          #target { position: absolute; left: 0; top: 100px; width: 50px; height: 50px;
                    scroll-snap-align: start; }
        </style>
        <div id="scroller"><div id="space"></div><div id="target"></div></div>"#,
    );
    check(
        &mut runtime,
        r#"(() => {
          const scroller = document.getElementById("scroller");
          scroller.scrollTo(0, 65);
          return scroller.scrollTop === 50;
        })()"#,
    );
}

#[test]
fn viewport_scroll_uses_root_snap_type_and_dispatches_scroll_event() {
    let mut runtime = runtime(
        r#"<style>
          html, body { margin: 0; }
          html { scroll-snap-type: y mandatory; }
          .item { height: 720px; scroll-snap-align: start; }
        </style>
        <body><div class="item"></div><div class="item"></div>
          <div class="item"></div></body>"#,
    );
    check(
        &mut runtime,
        r#"(() => {
          globalThis.scrollEvents = 0;
          addEventListener("scroll", () => scrollEvents++);
          scrollTo(0, 500);
          return scrollY === 720 && scrollEvents === 0;
        })()"#,
    );
    runtime.run_animation_frame(16).unwrap();
    check(&mut runtime, "scrollEvents === 1");
}

#[test]
fn scroll_into_view_accounts_for_nested_scroll_padding_and_margin() {
    let mut runtime = runtime(
        r#"<style>
          html, body { margin: 0; }
          #scroller { width: 100px; height: 100px; overflow: auto;
                      scroll-padding: 10px; }
          #space { width: 400px; height: 200px; }
          #target { width: 20px; height: 20px; margin-left: 200px;
                    scroll-margin: 10px; }
          #after { width: 400px; height: 400px; }
        </style>
        <div id="scroller"><div id="space"></div><div id="target"></div>
          <div id="after"></div></div>"#,
    );
    check(
        &mut runtime,
        r#"(() => {
          const scroller = document.getElementById("scroller");
          const target = document.getElementById("target");
          target.scrollIntoView({block: "start", inline: "start"});
          if (scroller.scrollTop !== 180 || scroller.scrollLeft !== 180) return false;
          scroller.scrollTo(0, 0);
          target.scrollIntoView({block: "end", inline: "end"});
          return scroller.scrollTop === 140 && scroller.scrollLeft === 140;
        })()"#,
    );
}

#[test]
fn scroll_into_view_nearest_keeps_an_oversized_area_covering_the_scrollport() {
    let mut runtime = runtime(
        r#"<style>
          html, body { margin: 0; }
          #scroller { width: 100px; height: 100px; overflow: auto; }
          #target { height: 200px; }
          #after { height: 100px; }
        </style>
        <div id="scroller"><div id="target"></div><div id="after"></div></div>"#,
    );
    check(
        &mut runtime,
        r#"(() => {
          const scroller = document.getElementById("scroller");
          scroller.scrollTop = 50;
          document.getElementById("target").scrollIntoView({block: "nearest", inline: "nearest"});
          return scroller.scrollTop === 50;
        })()"#,
    );
}

#[test]
fn ancestor_transform_moves_the_snap_area() {
    let mut runtime = runtime(
        r#"<style>
          html, body { margin: 0; }
          #scroller { width: 100px; height: 100px; overflow: auto;
                      scroll-snap-type: y mandatory; }
          #before { height: 100px; }
          #parent { transform: translateY(20px); }
          #target { height: 50px; scroll-snap-align: start; }
          #after { height: 200px; }
        </style>
        <div id="scroller"><div id="before"></div><div id="parent">
          <div id="target"></div></div><div id="after"></div></div>"#,
    );
    check(
        &mut runtime,
        r#"(() => {
          const scroller = document.getElementById("scroller");
          scroller.scrollTo(0, 75);
          return scroller.scrollTop === 120;
        })()"#,
    );
}

#[test]
fn two_axis_alignment_uses_block_and_inline_values_independently() {
    let mut runtime = runtime(
        r#"<style>
          html, body { margin: 0; }
          #scroller { position: relative; width: 100px; height: 100px; overflow: auto;
                      scroll-snap-type: both mandatory; }
          #space { position: absolute; left: 0; top: 0; width: 300px; height: 300px; }
          #target { position: absolute; left: 100px; top: 100px;
                    width: 50px; height: 50px; scroll-snap-align: start end; }
        </style>
        <div id="scroller"><div id="space"></div><div id="target"></div></div>"#,
    );
    check(
        &mut runtime,
        r#"(() => {
          const scroller = document.getElementById("scroller");
          scroller.scrollTo(65, 65);
          return scroller.scrollLeft === 50 && scroller.scrollTop === 100;
        })()"#,
    );
}

#[test]
fn percentage_scroll_padding_uses_the_scrollport_size() {
    let mut runtime = runtime(
        r#"<style>
          html, body { margin: 0; }
          #scroller { width: 100px; height: 100px; overflow: auto;
                      scroll-snap-type: y mandatory; scroll-padding-top: 10%; }
          .item { height: 100px; scroll-snap-align: start; }
        </style>
        <div id="scroller"><div class="item"></div><div class="item"></div>
          <div class="item"></div></div>"#,
    );
    check(
        &mut runtime,
        r#"(() => {
          const scroller = document.getElementById("scroller");
          scroller.scrollTo(0, 65);
          return scroller.scrollTop === 90;
        })()"#,
    );
}

#[test]
fn vertical_rtl_logical_padding_and_margin_shift_inline_start() {
    let mut runtime = runtime(
        r#"<style>
          html, body { margin: 0; }
          #scroller { position: relative; width: 100px; height: 100px; overflow: auto;
                      writing-mode: vertical-rl; direction: rtl;
                      scroll-snap-type: inline mandatory;
                      scroll-padding-inline-start: 10px; }
          #space { position: absolute; left: 0; top: 0; width: 100px; height: 300px; }
          #target { position: absolute; left: 0; top: 100px; width: 50px; height: 50px;
                    scroll-snap-align: start; scroll-margin-inline-start: 5px; }
        </style>
        <div id="scroller"><div id="space"></div><div id="target"></div></div>"#,
    );
    check(
        &mut runtime,
        r#"(() => {
          const scroller = document.getElementById("scroller");
          scroller.scrollTo(0, 72);
          return scroller.scrollTop === 65;
        })()"#,
    );
}

#[test]
fn smooth_scroll_animates_toward_the_snap_endpoint() {
    let mut runtime = runtime(
        r#"<style>
          html, body { margin: 0; }
          #scroller { width: 100px; height: 100px; overflow: auto;
                      scroll-snap-type: y mandatory; }
          .item { height: 100px; scroll-snap-align: start; }
        </style>
        <div id="scroller"><div class="item"></div><div class="item"></div>
          <div class="item"></div></div>"#,
    );
    check(
        &mut runtime,
        r#"(() => {
          const scroller = document.getElementById("scroller");
          scroller.scrollTo({top: 65, behavior: "smooth"});
          return scroller.scrollTop === 0;
        })()"#,
    );
    runtime.run_animation_frame(150).unwrap();
    check(
        &mut runtime,
        "document.getElementById('scroller').scrollTop === 50",
    );
    runtime.run_animation_frame(150).unwrap();
    check(
        &mut runtime,
        "document.getElementById('scroller').scrollTop === 100",
    );
}

#[test]
fn enabling_mandatory_snap_resnaps_an_existing_scroll_position() {
    let mut runtime = runtime(
        r#"<style>
          html, body { margin: 0; }
          #scroller { width: 100px; height: 100px; overflow: auto; }
          .item { height: 100px; scroll-snap-align: start; }
        </style>
        <div id="scroller"><div class="item"></div><div class="item"></div>
          <div class="item"></div></div>"#,
    );
    check(
        &mut runtime,
        r#"(() => {
          const scroller = document.getElementById("scroller");
          scroller.scrollTo(0, 65);
          if (scroller.scrollTop !== 65) return false;
          scroller.style.scrollSnapType = "y mandatory";
          return scroller.scrollTop === 100;
        })()"#,
    );
}
