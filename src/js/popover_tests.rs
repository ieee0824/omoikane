use super::JsRuntime;
use crate::html::TreeBuilder;
use crate::paint::{Canvas, Color, Image};
use std::fs;
use std::path::PathBuf;

const POPOVER_RENDER_FIXTURE: &str =
    include_str!("../../tests/fixtures/anonymized-popover-top-layer/page.html");

fn popover_render_fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("anonymized-popover-top-layer")
}

fn render_popover_fixture() -> Canvas {
    let document = TreeBuilder::parse(POPOVER_RENDER_FIXTURE).document();
    let mut runtime = JsRuntime::with_document(document).unwrap();
    runtime.set_viewport(160.0, 120.0);
    runtime
        .eval("document.getElementById('popover').showPopover()")
        .unwrap();
    runtime.paint_current_document().unwrap()
}

fn popover_render_output_path(variant: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("output")
        .join(format!(
            "anonymized-popover-top-layer.local-baseline.{variant}.png"
        ))
}

fn bool_result(runtime: &mut JsRuntime, source: &str) -> bool {
    runtime.eval(source).unwrap().as_boolean().unwrap_or(false)
}

#[test]
fn popover_idl_reflection_events_and_validity() {
    let mut runtime = JsRuntime::new().unwrap();
    assert!(bool_result(
        &mut runtime,
        r#"(() => {
          const detached = document.createElement("div");
          const errorName = callback => { try { callback(); } catch (error) { return error.name; } };
          const event = new ToggleEvent("toggle", {
            oldState: "closed", newState: "open", source: null,
          });
          const reflected = detached.popover === null;
          detached.popover = "";
          const emptyIsAuto = detached.popover === "auto" && detached.getAttribute("popover") === "";
          detached.popover = "invalid";
          const invalidIsManual = detached.popover === "manual";
          detached.popover = null;
          return reflected && emptyIsAuto && invalidIsManual && detached.popover === null &&
            typeof detached.showPopover === "function" &&
            typeof detached.hidePopover === "function" &&
            typeof detached.togglePopover === "function" &&
            errorName(() => detached.showPopover()) === "NotSupportedError" &&
            (detached.popover = "auto", errorName(() => detached.showPopover())) === "InvalidStateError" &&
            event.oldState === "closed" && event.newState === "open" && event.source === null &&
            Object.prototype.toString.call(event) === "[object ToggleEvent]" &&
            typeof __omoikane_get_popover_open === "undefined" &&
            typeof __omoikane_set_popover_open === "undefined" &&
            typeof __omoikane_set_modal_dialog === "undefined";
        })()"#,
    ));
}

#[test]
fn popover_auto_manual_nesting_focus_escape_disconnect_and_dialog() {
    let mut runtime = JsRuntime::new().unwrap();
    runtime
        .eval(
            r#"(() => {
              document.body.innerHTML = `
                <button id="before">before</button>
                <div id="parent" popover="auto"><button id="inside" autofocus>inside</button>
                  <div id="child" popover="auto"><button id="deep" autofocus>deep</button></div>
                </div>
                <div id="other" popover="auto">other</div>
                <div id="manual" popover="manual">manual</div>
                <dialog id="dialog"><button id="dialogButton">dialog</button>
                  <div id="dialogPopover" popover="auto"><button id="dialogAuto" autofocus>auto</button></div>
                </dialog>`;
              globalThis.popoverLog = [];
              const beforeButton = document.getElementById("before");
              const parentPopover = document.getElementById("parent");
              const childPopover = document.getElementById("child");
              const otherPopover = document.getElementById("other");
              const manualPopover = document.getElementById("manual");
              const dialogElement = document.getElementById("dialog");
              const dialogButtonElement = document.getElementById("dialogButton");
              const dialogPopoverElement = document.getElementById("dialogPopover");
              for (const node of [parentPopover, childPopover, otherPopover, manualPopover, dialogPopoverElement]) {
                node.addEventListener("beforetoggle", event => {
                  popoverLog.push(node.id + ":before:" + event.oldState + ":" + event.newState + ":" +
                    (event.source && event.source.id || ""));
                  if (node === parentPopover && globalThis.cancelParent) event.preventDefault();
                });
                node.addEventListener("toggle", event =>
                  popoverLog.push(node.id + ":toggle:" + event.oldState + ":" + event.newState));
              }
              beforeButton.focus();
              globalThis.cancelParent = true;
              parentPopover.showPopover();
              const canceled = !parentPopover.matches(":popover-open") && document.activeElement === beforeButton;
              globalThis.cancelParent = false;
              parentPopover.showPopover({ source: beforeButton });
              const parentFocused = parentPopover.matches(":popover-open") && document.activeElement.id === "inside";
              childPopover.showPopover({ source: document.getElementById("inside") });
              const nested = parentPopover.matches(":popover-open") && childPopover.matches(":popover-open") &&
                document.activeElement.id === "deep";
              otherPopover.showPopover();
              const unrelatedClosesStack = !parentPopover.matches(":popover-open") &&
                !childPopover.matches(":popover-open") && otherPopover.matches(":popover-open");
              manualPopover.showPopover();
              __omoikane_dispatch_keyboard_input("keydown", { key: "Escape", code: "Escape" });
              const escapeOnlyAuto = !otherPopover.matches(":popover-open") && manualPopover.matches(":popover-open");
              manualPopover.hidePopover();
              manualPopover.showPopover();
              document.body.removeChild(manualPopover);
              document.body.appendChild(manualPopover);
              const disconnectedHides = !manualPopover.matches(":popover-open");
              dialogElement.showModal();
              dialogPopoverElement.showPopover({ source: dialogButtonElement });
              __omoikane_dispatch_keyboard_input("keydown", { key: "Escape", code: "Escape" });
              const dialogPopoverFirst = dialogElement.open && !dialogPopoverElement.matches(":popover-open");
              __omoikane_dispatch_keyboard_input("keydown", { key: "Escape", code: "Escape" });
              const dialogThenCloses = !dialogElement.open;
              manualPopover.showPopover();
              const remainingManualOpen = manualPopover.matches(":popover-open");
              globalThis.popoverSnapshot = {
                canceled, parentFocused, nested, unrelatedClosesStack, escapeOnlyAuto,
                disconnectedHides, dialogPopoverFirst, dialogThenCloses, remainingManualOpen,
              };
            })()"#,
        )
        .unwrap();
    runtime.tick(0).unwrap();
    let event_log = runtime
        .eval("popoverLog.join('|')")
        .unwrap()
        .as_string()
        .unwrap()
        .to_std_string_escaped();
    for name in [
        "canceled",
        "parentFocused",
        "nested",
        "unrelatedClosesStack",
        "escapeOnlyAuto",
        "disconnectedHides",
        "dialogPopoverFirst",
        "dialogThenCloses",
        "remainingManualOpen",
    ] {
        assert!(
            bool_result(&mut runtime, &format!("popoverSnapshot.{name}")),
            "popover condition failed: {name}"
        );
    }
    for entry in [
        "parent:before:closed:open:before",
        "parent:toggle:closed:closed",
        "parent:before:open:closed:",
        "manual:toggle:closed:open",
    ] {
        assert!(
            bool_result(
                &mut runtime,
                &format!("popoverLog.some(value => value === {entry:?})")
            ),
            "missing popover event: {entry}; actual: {event_log}"
        );
    }
}

#[test]
fn popover_target_attributes_drive_button_and_input_activation() {
    let mut runtime = JsRuntime::new().unwrap();
    assert!(bool_result(
        &mut runtime,
        r#"(() => {
          document.body.innerHTML = `
            <button id="trigger" type="button" popovertarget="target">toggle</button>
            <input id="inputTrigger" type="button" popovertarget="target" popovertargetaction="show">
            <input id="submitTrigger" type="submit" popovertarget="target" popovertargetaction="show">
            <input id="textTrigger" type="text" popovertarget="target" popovertargetaction="show">
            <div id="target" popover="manual">target</div>`;
          const trigger = document.getElementById("trigger");
          const inputTrigger = document.getElementById("inputTrigger");
          const submitTrigger = document.getElementById("submitTrigger");
          const textTrigger = document.getElementById("textTrigger");
          const target = document.getElementById("target");
          const reflected = trigger.popoverTargetElement === target &&
            trigger.popoverTargetAction === "toggle" && inputTrigger.popoverTargetElement === target &&
            inputTrigger.popoverTargetAction === "show";
          trigger.click();
          const opened = target.matches(":popover-open");
          trigger.click();
          const closed = !target.matches(":popover-open");
          inputTrigger.click();
          const shown = target.matches(":popover-open");
          inputTrigger.click();
          const showIsIdempotent = target.matches(":popover-open");
          trigger.popoverTargetAction = "hide";
          trigger.click();
          const hidden = !target.matches(":popover-open") &&
            trigger.getAttribute("popovertargetaction") === "hide";
          submitTrigger.click();
          const submitOpened = target.matches(":popover-open");
          target.hidePopover();
          textTrigger.click();
          const textIgnored = !target.matches(":popover-open");
          trigger.disabled = true;
          trigger.popoverTargetAction = "show";
          trigger.click();
          return reflected && opened && closed && shown && showIsIdempotent && hidden &&
            submitOpened && textIgnored &&
            !target.matches(":popover-open");
        })()"#,
    ));
}

#[test]
fn popover_top_layer_backdrop_paints_and_hit_tests_above_maximum_z_index() {
    let document = TreeBuilder::parse(
        r#"<html><head><style>
          * { margin: 0; padding: 0; border: 0 }
          body { background: #0000ff }
          #cover { position: fixed; inset: 0; z-index: 2147483647; background: #0000ff }
          #popover { position: fixed; left: 10px; top: 10px; transform: none;
                     width: 20px; height: 20px; background: #ff0000 }
          #popover::backdrop { background: #00ff00 }
        </style></head><body><div id="cover"></div><div id="popover" popover="manual"></div></body></html>"#,
    )
    .document();
    let mut runtime = JsRuntime::with_document(document).unwrap();
    runtime.set_viewport(50.0, 50.0);
    runtime
        .eval("document.getElementById('popover').showPopover()")
        .unwrap();

    let canvas = runtime.paint_current_document().unwrap();
    assert_eq!(canvas.pixel(0, 0), Some(Color::rgb(0, 255, 0)));
    assert_eq!(canvas.pixel(15, 15), Some(Color::rgb(255, 0, 0)));
    let hit = runtime.hit_test(15.0, 15.0).expect("popover must be hit");
    assert_eq!(hit.get_attribute("id").as_deref(), Some("popover"));
}

#[test]
fn popover_top_layer_fixture_matches_local_baseline_png() {
    let actual = render_popover_fixture();
    assert_eq!((actual.width(), actual.height()), (160, 120));
    let baseline_path = popover_render_fixture_dir().join("page.baseline.png");
    let expected_png = fs::read(&baseline_path).unwrap_or_else(|error| {
        panic!(
            "missing popover rendering baseline at {}: {error}",
            baseline_path.display()
        )
    });
    let expected = Image::decode_png(&expected_png).expect("decode popover rendering baseline");
    assert_eq!((expected.width(), expected.height()), (160, 120));

    let changed = actual
        .pixels()
        .chunks_exact(4)
        .zip(expected.pixels().chunks_exact(4))
        .filter(|(actual, expected)| actual != expected)
        .count();
    if changed != 0 {
        let mut diff = Canvas::new(actual.width(), actual.height());
        for (index, (actual_pixel, expected_pixel)) in actual
            .pixels()
            .chunks_exact(4)
            .zip(expected.pixels().chunks_exact(4))
            .enumerate()
        {
            if actual_pixel != expected_pixel {
                diff.set_pixel(
                    index as u32 % actual.width(),
                    index as u32 / actual.width(),
                    Color::rgb(255, 0, 255),
                );
            }
        }
        fs::create_dir_all(popover_render_output_path("actual").parent().unwrap()).unwrap();
        fs::write(popover_render_output_path("actual"), actual.encode_png()).unwrap();
        fs::write(popover_render_output_path("expected"), expected_png).unwrap();
        fs::write(popover_render_output_path("diff"), diff.encode_png()).unwrap();
        panic!(
            "popover rendering diverged from the checked-in baseline ({changed} pixels differ); wrote actual, expected, and diff PNGs to tests/output"
        );
    }
}

#[test]
#[ignore = "maintenance utility: set OMOIKANE_REFRESH_BASELINE=1 to rewrite the checked-in popover baseline image"]
fn refresh_popover_top_layer_baseline_png() {
    if std::env::var("OMOIKANE_REFRESH_BASELINE").as_deref() != Ok("1") {
        eprintln!(
            "skipping refresh_popover_top_layer_baseline_png: set OMOIKANE_REFRESH_BASELINE=1 to rewrite {}",
            popover_render_fixture_dir()
                .join("page.baseline.png")
                .display()
        );
        return;
    }
    fs::write(
        popover_render_fixture_dir().join("page.baseline.png"),
        render_popover_fixture().encode_png(),
    )
    .unwrap();
}
