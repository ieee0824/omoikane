use omoikane::{html::TreeBuilder, js::JsRuntime};

fn runtime() -> JsRuntime {
    JsRuntime::with_document(TreeBuilder::parse("<div id='target'></div>").document())
        .expect("runtime")
}

fn eval_json(runtime: &mut JsRuntime, source: &str) -> String {
    runtime
        .eval(source)
        .expect("evaluate Web Animations probe")
        .as_string()
        .expect("JSON string")
        .to_std_string_escaped()
}

#[test]
fn keyframe_effect_owns_normalized_frames_and_timing() {
    let mut runtime = runtime();
    assert_eq!(
        eval_json(
            &mut runtime,
            "(() => { const target = document.getElementById('target');
              const input = [{ opacity: 0 }, { opacity: 1, offset: 1 }];
              const effect = new KeyframeEffect(target, input,
                { duration: 100, fill: 'both', composite: 'add' });
              input[0].opacity = 9;
              const first = effect.getKeyframes(); first[0].opacity = 8;
              effect.updateTiming({ delay: 20 });
              return JSON.stringify([
                effect instanceof AnimationEffect, effect.target === target,
                effect.composite, effect.getKeyframes().map(frame =>
                  [frame.opacity, frame.offset, frame.computedOffset, frame.easing]),
                effect.getTiming().duration, effect.getTiming().delay,
                effect.getComputedTiming().endTime
              ]); })()",
        ),
        "[true,true,\"add\",[[0,null,0,\"linear\"],[1,1,1,\"linear\"]],100,20,120]"
    );
}

#[test]
fn animation_accepts_an_effect_and_element_animate_remains_compatible() {
    let mut runtime = runtime();
    assert_eq!(
        eval_json(
            &mut runtime,
            "(() => { const target = document.getElementById('target');
              const effect = new KeyframeEffect(target, [{ opacity: 0 }, { opacity: 1 }], 10);
              const idle = new Animation(effect, document.timeline);
              const running = target.animate([{ opacity: 0 }, { opacity: 1 }], 10);
              return JSON.stringify([
                idle.effect === effect, idle.timeline === document.timeline, idle.playState,
                running.effect instanceof KeyframeEffect, running.playState,
                document.timeline instanceof DocumentTimeline,
                typeof CSSAnimation, typeof CSSTransition
              ]); })()",
        ),
        "[true,true,\"idle\",true,\"running\",true,\"function\",\"function\"]"
    );
}

#[test]
fn linear_easing_is_exposed_through_cssom_validation() {
    let mut runtime = runtime();
    assert_eq!(
        eval_json(
            &mut runtime,
            "(() => { const target = document.getElementById('target');
              target.style.transitionTimingFunction = 'linear(0, .5 25% 75%, 1)';
              return JSON.stringify([
                CSS.supports('transition-timing-function', 'linear(0, .5 25% 75%, 1)'),
                CSS.supports('transition-timing-function', 'linear(0)'),
                target.style.transitionTimingFunction
              ]); })()",
        ),
        "[true,false,\"linear(0 0%, 0.5 25%, 0.5 75%, 1 100%)\"]"
    );
}
