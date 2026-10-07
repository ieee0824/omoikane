//! Animation stacking-context metadata, separate from computed property values.
use super::super::*;

pub(in crate::css::style) fn has_transform_animation(
    resolver: &StyleResolver,
    node: &NodeHandle,
    pseudo: Option<PseudoElement>,
    scope: Option<usize>,
    properties: &PropertyMap,
) -> bool {
    if matches!(properties.get("display"), Some(ComputedValue::Keyword(value)) if value == "none") {
        return false;
    }
    let name = match properties.get("animation-name") {
        Some(ComputedValue::Keyword(name) | ComputedValue::String(name)) => name,
        _ => return false,
    };
    let Some(steps) = resolver.keyframes_for(node, scope, name) else {
        return false;
    };
    if !steps.iter().any(|step| {
        step.declarations.iter().any(|declaration| {
            matches!(
                declaration.name.as_str(),
                "translate" | "rotate" | "scale" | "transform"
            )
        })
    }) {
        return false;
    }
    if let Some(timeline) = &resolver.animation_timeline {
        return timeline
            .borrow()
            .applies_will_change(node.identity(), pseudo)
            .unwrap_or(false);
    }
    let keyword = |name| match properties.get(name) {
        Some(ComputedValue::Keyword(value)) => value.as_str(),
        _ => "",
    };
    if matches!(keyword("animation-fill-mode"), "forwards" | "both") {
        return true;
    }
    let duration = animation_seconds(properties.get("animation-duration")).unwrap_or(0.0);
    let delay = animation_seconds(properties.get("animation-delay")).unwrap_or(0.0);
    let iterations = match properties.get("animation-iteration-count") {
        Some(ComputedValue::Number(value)) => value.max(0.0),
        Some(ComputedValue::Keyword(value)) if value == "infinite" => f32::INFINITY,
        _ => 1.0,
    };
    let elapsed = if keyword("animation-play-state") == "paused" {
        0.0
    } else {
        STATIC_ANIMATION_TIME_SECONDS
    };
    let active_duration = if duration <= 0.0 {
        0.0
    } else {
        duration * iterations
    };
    elapsed - delay < active_duration
}
