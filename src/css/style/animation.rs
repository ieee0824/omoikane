//! Owned live CSS animation clocks; static resolvers retain snapshot rendering.

use super::*;

pub(super) fn snapshot_shadow_base(
    steps: &[KeyframeStep],
    properties: &PropertyMap,
) -> Option<[Option<ComputedValue>; 2]> {
    steps
        .iter()
        .any(|step| {
            step.declarations
                .iter()
                .any(|declaration| declaration.name == "text-shadow")
        })
        .then(|| {
            [
                properties.get("color").cloned(),
                properties.get("text-shadow").cloned(),
            ]
        })
}

/// Shares phase, iteration, fill and direction rules with the owned live clock.
pub(super) fn snapshot_progress(properties: &PropertyMap, elapsed: f32) -> Option<f32> {
    AnimationSpec::from_properties("", None, properties).progress(elapsed)
}

#[derive(Debug, Clone, PartialEq)]
enum AnimationDirection {
    Normal,
    Reverse,
    Alternate,
    AlternateReverse,
}

impl AnimationDirection {
    fn from_keyword(value: &str) -> Self {
        match value {
            "reverse" => Self::Reverse,
            "alternate" => Self::Alternate,
            "alternate-reverse" => Self::AlternateReverse,
            _ => Self::Normal,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
enum AnimationFill {
    None,
    Forwards,
    Backwards,
    Both,
}

impl AnimationFill {
    fn from_keyword(value: &str) -> Self {
        match value {
            "forwards" => Self::Forwards,
            "backwards" => Self::Backwards,
            "both" => Self::Both,
            _ => Self::None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
struct AnimationSpec {
    name: String,
    scope: Option<usize>,
    duration: f32,
    delay: f32,
    iterations: f32,
    direction: AnimationDirection,
    fill: AnimationFill,
    timing: String,
    requires_layout: bool,
    refreshes_visibility: bool,
    opacity_only: bool,
}

impl AnimationSpec {
    fn from_properties(name: &str, scope: Option<usize>, properties: &PropertyMap) -> Self {
        let keyword = |property, fallback: &str| match properties.get(&property) {
            Some(ComputedValue::Keyword(value)) => value.to_ascii_lowercase(),
            _ => fallback.to_string(),
        };
        let iterations = match properties.get(&PropertyId::AnimationIterationCount) {
            Some(ComputedValue::Keyword(value)) if value.eq_ignore_ascii_case("infinite") => {
                f32::INFINITY
            }
            Some(ComputedValue::Number(value)) => value.max(0.0),
            _ => 1.0,
        };
        Self {
            name: name.to_string(),
            scope,
            duration: animation_seconds(properties.get(&PropertyId::AnimationDuration))
                .unwrap_or(0.0)
                .max(0.0),
            delay: animation_seconds(properties.get(&PropertyId::AnimationDelay)).unwrap_or(0.0),
            iterations,
            direction: AnimationDirection::from_keyword(&keyword(
                PropertyId::AnimationDirection,
                "normal",
            )),
            fill: AnimationFill::from_keyword(&keyword(PropertyId::AnimationFillMode, "none")),
            timing: keyword(PropertyId::AnimationTimingFunction, "ease"),
            requires_layout: true,
            refreshes_visibility: false,
            opacity_only: false,
        }
    }

    fn active_duration(&self) -> f32 {
        if self.duration <= 0.0 {
            0.0
        } else {
            self.duration * self.iterations
        }
    }

    fn progress(&self, elapsed: f32) -> Option<f32> {
        let active = elapsed - self.delay;
        if active < 0.0 {
            return matches!(self.fill, AnimationFill::Backwards | AnimationFill::Both)
                .then(|| self.directed(0.0, 0));
        }
        if active >= self.active_duration() {
            if !matches!(self.fill, AnimationFill::Forwards | AnimationFill::Both) {
                return None;
            }
            let count = self.iterations;
            let fraction = count.fract();
            let progress = if count <= 0.0 {
                0.0
            } else if fraction == 0.0 {
                1.0
            } else {
                fraction
            };
            return Some(self.directed(progress, count.ceil().max(1.0) as u64 - 1));
        }
        let position = active / self.duration;
        Some(self.directed(position.fract(), position.floor() as u64))
    }

    fn directed(&self, progress: f32, iteration: u64) -> f32 {
        let reverse = match self.direction {
            AnimationDirection::Reverse => true,
            AnimationDirection::Alternate => iteration % 2 == 1,
            AnimationDirection::AlternateReverse => iteration % 2 == 0,
            _ => false,
        };
        if reverse { 1.0 - progress } else { progress }
    }
}

#[derive(Debug, Clone)]
struct AnimationState {
    spec: AnimationSpec,
    start_ms: f64,
    paused_elapsed_ms: Option<f64>,
}

impl AnimationState {
    fn elapsed_ms(&self, now_ms: f64) -> f64 {
        self.paused_elapsed_ms
            .unwrap_or_else(|| (now_ms - self.start_ms).max(0.0))
    }
    fn progress(&self, now_ms: f64) -> Option<f32> {
        self.spec
            .progress((self.elapsed_ms(now_ms) / 1000.0) as f32)
    }
    fn applies_will_change(&self, now_ms: f64) -> bool {
        (self.elapsed_ms(now_ms) / 1000.0) as f32 - self.spec.delay < self.spec.active_duration()
            || matches!(
                self.spec.fill,
                AnimationFill::Forwards | AnimationFill::Both
            )
    }

    fn running(&self, now_ms: f64) -> bool {
        self.paused_elapsed_ms.is_none()
            && (self.elapsed_ms(now_ms) / 1000.0) as f32 - self.spec.delay
                < self.spec.active_duration()
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct AnimationTimeline {
    now_ms: f64,
    layout_changed: bool,
    elements: HashMap<(usize, Option<PseudoElement>), AnimationState>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct AnimationClockEffects {
    requires_layout: bool,
    refreshes_visibility: bool,
    retained_elements: usize,
    retained_pseudos: usize,
    changed_elements: usize,
    changed_pseudos: usize,
    changed_targets: Vec<(usize, Option<PseudoElement>)>,
    opacity_only: bool,
}

impl AnimationClockEffects {
    fn changed(&self) -> bool {
        self.changed_elements != 0 || self.changed_pseudos != 0
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct KeyframePropertyEffects {
    requires_layout: bool,
    refreshes_visibility: bool,
    opacity_only: bool,
}

/// Classifies properties whose values are sampled into the computed style.
/// `animation-timing-function` is keyframe metadata: the current sampler does
/// not implement per-keyframe easing, but it must not turn an otherwise
/// opacity-only animation into a geometry animation.
fn classify_keyframe_properties(steps: &[KeyframeStep]) -> KeyframePropertyEffects {
    let mut effects = KeyframePropertyEffects {
        opacity_only: true,
        ..Default::default()
    };
    let mut has_animated_property = false;
    for declaration in steps.iter().flat_map(|step| &step.declarations) {
        if declaration.name == "animation-timing-function" {
            continue;
        }
        has_animated_property = true;
        effects.opacity_only &= declaration.name == "opacity";
        if declaration.name == "visibility" {
            effects.refreshes_visibility = true;
        } else {
            effects.requires_layout |=
                super::super::transition::animated_property_requires_layout(&declaration.name);
        }
    }
    effects.opacity_only &= has_animated_property;
    effects
}

/// Bounded summary of one transition and animation clock sample.
///
/// The summary contains counts, booleans, and a bounded cache outcome only. It
/// does not retain DOM nodes, property names, CSS values, or source locations.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct StyleClockEffects {
    pub(crate) transition_changed: bool,
    pub(crate) transition_requires_layout: bool,
    /// Transition cache processing before animation invalidation in this sample.
    pub(crate) transition_cache_before_animation: TransitionCacheEffects,
    pub(crate) animation_changed: bool,
    pub(crate) animation_requires_layout: bool,
    /// Every animation target changed only `opacity` in this sample.
    pub(crate) animation_opacity_only: bool,
    /// A changed keyframe sample updates visibility without changing geometry.
    pub(crate) animation_refreshes_visibility: bool,
    pub(crate) retained_animation_elements: usize,
    pub(crate) retained_animation_pseudos: usize,
    pub(crate) changed_animation_elements: usize,
    pub(crate) changed_animation_pseudos: usize,
}

impl StyleClockEffects {
    /// Whether either sampled clock requires cached styles to be invalidated.
    pub(crate) fn changed(self) -> bool {
        self.transition_changed || self.animation_changed
    }

    /// Whether the current invalidation policy requires rebuilt geometry.
    pub(crate) fn requires_layout(self) -> bool {
        self.transition_requires_layout
            || (self.animation_changed && self.animation_requires_layout)
    }
}

impl AnimationTimeline {
    pub(super) fn applies_will_change(
        &self,
        node: usize,
        pseudo: Option<PseudoElement>,
    ) -> Option<bool> {
        self.elements
            .get(&(node, pseudo))
            .map(|state| state.applies_will_change(self.now_ms))
    }

    fn set_time(&mut self, now_ms: f64) -> AnimationClockEffects {
        let next = if now_ms.is_finite() {
            now_ms.max(self.now_ms)
        } else {
            self.now_ms
        };
        let mut effects = AnimationClockEffects::default();
        effects.opacity_only = true;
        for ((node, pseudo), state) in &self.elements {
            if pseudo.is_some() {
                effects.retained_pseudos += 1;
            } else {
                effects.retained_elements += 1;
            }
            let sample_changed = state.progress(self.now_ms) != state.progress(next)
                || state.applies_will_change(self.now_ms) != state.applies_will_change(next);
            if sample_changed {
                effects.changed_targets.push((*node, *pseudo));
                if pseudo.is_some() {
                    effects.changed_pseudos += 1;
                } else {
                    effects.changed_elements += 1;
                }
                effects.requires_layout |= state.spec.requires_layout;
                effects.refreshes_visibility |= state.spec.refreshes_visibility;
                effects.opacity_only &= state.spec.opacity_only;
            }
        }
        self.now_ms = next;
        self.layout_changed = effects.requires_layout;
        effects
    }
    fn sample(
        &mut self,
        node: usize,
        pseudo: Option<PseudoElement>,
        spec: AnimationSpec,
        paused: bool,
    ) -> Option<f32> {
        let now = self.now_ms;
        let state = self
            .elements
            .entry((node, pseudo))
            .or_insert_with(|| AnimationState {
                spec: spec.clone(),
                start_ms: now,
                paused_elapsed_ms: None,
            });
        if state.spec.name != spec.name || state.spec.scope != spec.scope {
            state.start_ms = now;
            state.paused_elapsed_ms = None;
        }
        state.spec = spec;
        if paused && state.paused_elapsed_ms.is_none() {
            state.paused_elapsed_ms = Some(state.elapsed_ms(now));
        } else if !paused && let Some(elapsed) = state.paused_elapsed_ms.take() {
            state.start_ms = now - elapsed;
        }
        state.progress(now)
    }
}

impl StyleResolver {
    /// Uses the same shadow interpolation for deterministic paused snapshots.
    pub(super) fn interpolate_snapshot_text_shadow(
        &self,
        steps: &[KeyframeStep],
        progress: f32,
        properties: &mut PropertyMap,
        important: &HashSet<String>,
        underlying: Option<[Option<ComputedValue>; 2]>,
    ) {
        let Some(underlying) = underlying else {
            return;
        };
        if important.contains("text-shadow") {
            return;
        }
        let steps = steps
            .iter()
            .map(|step| KeyframeStep {
                offset: step.offset,
                declarations: step
                    .declarations
                    .iter()
                    .filter(|declaration| {
                        matches!(declaration.name.as_str(), "color" | "text-shadow")
                    })
                    .cloned()
                    .collect(),
            })
            .collect::<Vec<_>>();
        for (name, value) in ["color", "text-shadow"].into_iter().zip(underlying) {
            if let Some(value) = value {
                properties.insert(name, value);
            } else {
                properties.remove(name);
            }
        }
        let timing = properties
            .get("animation-timing-function")
            .map_or_else(|| "ease".into(), ComputedValue::css_text);
        self.interpolate_live_keyframes(&steps, progress, &timing, None, properties, important);
    }

    pub(super) fn apply_animation_effect(
        &self,
        node: &NodeHandle,
        pseudo: Option<PseudoElement>,
        properties: &mut PropertyMap,
        important: &HashSet<String>,
        scope: Option<usize>,
        custom_properties: &BTreeMap<String, Value>,
    ) -> BTreeMap<String, Value> {
        let untyped = if self.animation_timeline.is_some() {
            self.apply_live_animation(
                node,
                pseudo,
                properties,
                important,
                scope,
                custom_properties,
            )
        } else {
            self.apply_animation_snapshot(node, properties, important, scope, custom_properties)
        };
        for (name, value) in &untyped {
            properties.insert(name.clone(), ComputedValue::Keyword(render_value(value)));
        }
        untyped
    }

    /// Reads the retained sample clock without changing timeline state.
    pub(crate) fn animation_diagnostic_time_ms(&self) -> Option<f64> {
        self.animation_timeline
            .as_ref()
            .map(|timeline| timeline.borrow().now_ms)
    }

    /// Node identities needed only by opt-in property-name diagnostics.
    pub(crate) fn animation_diagnostic_node_ids(&self) -> Vec<usize> {
        self.animation_timeline
            .as_ref()
            .map_or_else(Vec::new, |timeline| {
                let mut ids: Vec<_> = timeline
                    .borrow()
                    .elements
                    .keys()
                    .map(|(node, _)| *node)
                    .collect();
                ids.sort_unstable();
                ids.dedup();
                ids
            })
    }

    /// Returns owned, sorted property-name unions for running and changed
    /// animation states. Names describe keyframe declarations, not proof that
    /// each property's interpolated value changed. Missing nodes are reported
    /// explicitly; this read does not resolve styles or advance either clock.
    pub(crate) fn animation_diagnostic_property_names(
        &self,
        previous_ms: Option<f64>,
        mut lookup: impl FnMut(usize) -> Option<NodeHandle>,
    ) -> (Vec<String>, Vec<String>, usize) {
        let mut running = std::collections::BTreeSet::new();
        let mut changed = std::collections::BTreeSet::new();
        let mut missing = 0;
        if let Some(timeline) = self.animation_timeline.as_ref() {
            let timeline = timeline.borrow();
            for ((node_id, _), state) in &timeline.elements {
                let is_running = state.running(timeline.now_ms);
                let is_changed = previous_ms.is_some_and(|previous| {
                    state.progress(previous) != state.progress(timeline.now_ms)
                        || state.applies_will_change(previous)
                            != state.applies_will_change(timeline.now_ms)
                });
                if !is_running && !is_changed {
                    continue;
                }
                let Some(node) = lookup(*node_id) else {
                    missing += 1;
                    continue;
                };
                let Some(steps) = self.keyframes_for(&node, state.spec.scope, &state.spec.name)
                else {
                    missing += 1;
                    continue;
                };
                for declaration in steps.iter().flat_map(|step| &step.declarations) {
                    if is_running {
                        running.insert(declaration.name.clone());
                    }
                    if is_changed {
                        changed.insert(declaration.name.clone());
                    }
                }
            }
        }
        (
            running.into_iter().collect(),
            changed.into_iter().collect(),
            missing,
        )
    }

    /// Enables explicit live sampling, preserving the clock and start times on
    /// cache rebuilds. First activation drops static snapshots; later pseudo-only
    /// samples retain ordinary element styles. This is an effect at the rendering
    /// lifecycle boundary.
    fn sample_animation_time_ms(&mut self, time_ms: f64) -> AnimationClockEffects {
        let first_live_sample = self.animation_timeline.is_none();
        let timeline = self.animation_timeline.get_or_insert_with(Default::default);
        let timeline = timeline.get_mut();
        let effects = timeline.set_time(time_ms);
        if first_live_sample {
            self.cache.clear();
            self.pseudo_cache.clear();
        } else if effects.changed() && effects.requires_layout {
            // Preserve the conservative fallback for layout-affecting clocks:
            // a retained ordinary state can still supply a filled final value
            // while only a pseudo sample advances.
            if effects.retained_elements != 0 {
                self.cache.clear();
            }
            self.pseudo_cache.clear();
        } else if effects.changed() {
            for (node, pseudo) in &effects.changed_targets {
                if let Some(pseudo) = pseudo {
                    self.pseudo_cache.remove(&(*node, *pseudo));
                } else {
                    self.cache.remove(node);
                }
            }
        }
        effects
    }

    /// Advances the live sampling clock while preserving its change-only result.
    pub(crate) fn set_animation_time_ms(&mut self, time_ms: f64) -> bool {
        self.sample_animation_time_ms(time_ms).changed()
    }

    /// Whether the most recent live sample can change layout geometry.
    pub(crate) fn animation_sample_requires_layout(&self) -> bool {
        self.animation_timeline
            .as_ref()
            .is_some_and(|timeline| timeline.borrow().layout_changed)
    }

    /// Advances both live style clocks and returns a value-only diagnostic
    /// summary. Cache invalidation remains inside the sampling helpers so
    /// callers cannot observe a sampled clock with stale caches.
    pub(crate) fn advance_style_clocks(
        &mut self,
        time_ms: f64,
        live_animations: bool,
    ) -> StyleClockEffects {
        let (transition_changed, transition_cache) = self.sample_transition_time_ms(time_ms);
        self.finish_style_clock_advance(
            transition_changed,
            transition_cache,
            time_ms,
            live_animations,
        )
    }

    /// Advances both live style clocks while resolving running transition
    /// nodes through a document-scoped lookup for localized cache eviction.
    pub(crate) fn advance_style_clocks_with_node_lookup<F>(
        &mut self,
        time_ms: f64,
        live_animations: bool,
        lookup: F,
    ) -> StyleClockEffects
    where
        F: FnMut(usize) -> Option<NodeHandle>,
    {
        let (transition_changed, transition_cache) =
            self.sample_transition_time_ms_with_node_lookup(time_ms, lookup);
        self.finish_style_clock_advance(
            transition_changed,
            transition_cache,
            time_ms,
            live_animations,
        )
    }

    fn finish_style_clock_advance(
        &mut self,
        transition_changed: bool,
        transition_cache_before_animation: TransitionCacheEffects,
        time_ms: f64,
        live_animations: bool,
    ) -> StyleClockEffects {
        // Preserve the existing conservative policy: a retained layout-affecting
        // transition participates in the combined result whenever either clock
        // changes, even if this transition sample itself did not advance.
        let transition_requires_layout = self.running_transitions_require_layout();
        let animation = if live_animations {
            self.sample_animation_time_ms(time_ms)
        } else {
            AnimationClockEffects::default()
        };
        StyleClockEffects {
            transition_changed,
            transition_requires_layout,
            transition_cache_before_animation,
            animation_changed: animation.changed(),
            animation_requires_layout: animation.requires_layout,
            animation_opacity_only: animation.changed() && animation.opacity_only,
            animation_refreshes_visibility: animation.refreshes_visibility,
            retained_animation_elements: animation.retained_elements,
            retained_animation_pseudos: animation.retained_pseudos,
            changed_animation_elements: animation.changed_elements,
            changed_animation_pseudos: animation.changed_pseudos,
        }
    }
    pub(crate) fn take_animation_timeline(&mut self) -> Option<AnimationTimeline> {
        let timeline = self.animation_timeline.take().map(RefCell::into_inner);
        if timeline.is_some() {
            self.cache.clear();
            self.pseudo_cache.clear();
        }
        timeline
    }
    pub(crate) fn install_animation_timeline(&mut self, timeline: Option<AnimationTimeline>) {
        self.animation_timeline = timeline.map(RefCell::new);
        self.cache.clear();
        self.pseudo_cache.clear();
    }
    /// Reads playback state without sampling or mutating computed styles.
    pub(crate) fn has_running_animations(&self) -> bool {
        self.animation_timeline.as_ref().is_some_and(|timeline| {
            let timeline = timeline.borrow();
            timeline
                .elements
                .values()
                .any(|state| state.running(timeline.now_ms))
        })
    }
    /// Paint-only clocks in disjoint branches cannot change this action's styles.
    /// Sampling still invalidates animated targets; the owner still rebuilds geometry.
    pub(super) fn animations_allow_user_action_subtree_invalidation(
        &self,
        invalidated: &HashSet<usize>,
        roots: &[NodeHandle],
    ) -> bool {
        let Some(timeline) = &self.animation_timeline else {
            return true;
        };
        let timeline = timeline.borrow();
        timeline.elements.iter().all(|((identity, _), state)| {
            if !state.running(timeline.now_ms) {
                return true;
            }
            if state.spec.requires_layout || invalidated.contains(identity) {
                return false;
            }
            !roots.iter().any(|root| {
                let mut ancestor = root.parent_node();
                while let Some(node) = ancestor {
                    if node.identity() == *identity {
                        return true;
                    }
                    ancestor = node.parent_node();
                }
                false
            })
        })
    }
    pub(crate) fn animation_node_ids(&self) -> Vec<usize> {
        self.animation_timeline
            .as_ref()
            .map(|timeline| {
                timeline
                    .borrow()
                    .elements
                    .keys()
                    .map(|(node, _)| *node)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Element targets whose running keyframes affect only opacity.
    pub(crate) fn opacity_only_animation_node_ids(&self) -> HashSet<usize> {
        self.animation_timeline
            .as_ref()
            .map(|timeline| {
                timeline
                    .borrow()
                    .elements
                    .iter()
                    .filter_map(|((node, pseudo), state)| {
                        (pseudo.is_none() && state.spec.opacity_only).then_some(*node)
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Whether an opacity-only keyframe targets a pseudo-element. The child
    /// effect cache has no pseudo identity in its key, so such documents fail
    /// closed until that identity is represented explicitly.
    pub(crate) fn has_opacity_only_animation_pseudos(&self) -> bool {
        self.animation_timeline.as_ref().is_some_and(|timeline| {
            timeline
                .borrow()
                .elements
                .iter()
                .any(|((_, pseudo), state)| pseudo.is_some() && state.spec.opacity_only)
        })
    }
    pub(crate) fn retain_animation_nodes(&mut self, nodes: &HashSet<usize>) {
        if let Some(timeline) = &mut self.animation_timeline {
            timeline
                .get_mut()
                .elements
                .retain(|(node, _), _| nodes.contains(node));
        }
    }

    pub(super) fn apply_live_animation(
        &self,
        node: &NodeHandle,
        pseudo: Option<PseudoElement>,
        properties: &mut PropertyMap,
        important: &HashSet<String>,
        scope: Option<usize>,
        custom_properties: &BTreeMap<String, Value>,
    ) -> BTreeMap<String, Value> {
        let Some(timeline) = &self.animation_timeline else {
            return BTreeMap::new();
        };
        let name = match properties.get(&PropertyId::AnimationName) {
            Some(ComputedValue::Keyword(name) | ComputedValue::String(name)) => name.clone(),
            _ => String::new(),
        };
        let steps = self.keyframes_for(node, scope, &name);
        let hidden = matches!(
            properties.get(&PropertyId::Display),
            Some(ComputedValue::Keyword(display)) if display.eq_ignore_ascii_case("none")
        );
        let Some(steps) = steps.filter(|_| !hidden) else {
            timeline
                .borrow_mut()
                .elements
                .remove(&(node.identity(), pseudo));
            return BTreeMap::new();
        };
        let mut spec = AnimationSpec::from_properties(&name, scope, properties);
        // Paint-only properties do not change containing blocks or geometry.
        // Unknown/custom properties retain conservative reflow. Keyframe
        // metadata is not itself an animated computed-style target.
        let property_effects = classify_keyframe_properties(steps);
        spec.refreshes_visibility = property_effects.refreshes_visibility;
        spec.opacity_only = property_effects.opacity_only;
        spec.requires_layout = property_effects.requires_layout;
        let paused = matches!(
            properties.get(&PropertyId::AnimationPlayState),
            Some(ComputedValue::Keyword(value)) if value.eq_ignore_ascii_case("paused")
        );
        let progress = timeline
            .borrow_mut()
            .sample(node.identity(), pseudo, spec.clone(), paused);
        if let Some(progress) = progress {
            let steps = resolve_animation_keyframes(
                steps,
                node,
                custom_properties,
                &self.registered_custom_properties,
            );
            self.interpolate_live_keyframes(
                &steps,
                progress,
                &spec.timing,
                self.color_container_size(node),
                properties,
                important,
            );
            return sample_untyped_custom_values(
                &steps,
                progress,
                &spec.timing,
                custom_properties,
                &self.registered_custom_properties,
                important,
            );
        }
        BTreeMap::new()
    }

    fn interpolate_live_keyframes(
        &self,
        steps: &[KeyframeStep],
        progress: f32,
        timing: &str,
        color_container_size: Option<[f32; 2]>,
        properties: &mut PropertyMap,
        important: &HashSet<String>,
    ) {
        let font_size = match properties.get(&PropertyId::FontSize) {
            Some(ComputedValue::Px(px)) => *px,
            _ => 16.0,
        };
        let ctx = ResolutionContext {
            parent_font_size: font_size,
            root_font_size: self.root_font_size,
            line_height: used_line_height_value(properties.get(&PropertyId::LineHeight), font_size),
            root_line_height: self.root_line_height(),
            font_metrics: CssRelativeFontMetrics::fallback(font_size, false),
            viewport_width: self.viewport_width,
            viewport_height: self.viewport_height,
            color_container_size,
        };
        let custom: BTreeMap<_, _> = properties
            .iter()
            .filter(|(name, _)| name.starts_with("--"))
            .map(|(name, value)| (name.to_string(), computed_value_to_value(value)))
            .collect();
        let mut names: Vec<_> = steps
            .iter()
            .flat_map(|step| step.declarations.iter().map(|d| d.name.as_str()))
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        // Dependent currentcolor values must see this frame's animated color.
        names.sort_unstable_by_key(|name| (*name != "color", *name));
        for name in names {
            if important.contains(name) {
                continue;
            }
            let lower = steps
                .iter()
                .rev()
                .filter(|step| step.offset <= progress)
                .find_map(|step| declaration_in(step, name));
            let upper = steps
                .iter()
                .filter(|step| step.offset >= progress)
                .find_map(|step| declaration_in(step, name));
            let current = properties.get(name).cloned();
            let resolve = |entry: Option<(f32, &Value)>| {
                entry
                    .and_then(|(_, value)| resolve_value_with_custom_properties(value, &custom))
                    .map(|value| {
                        if let Some(registration) = self.registered_custom_properties.get(name) {
                            compute_registered_value(&value, &registration.syntax, ctx)
                        } else {
                            compute_value(&value, name, ctx)
                        }
                    })
                    .or_else(|| current.clone())
            };
            let start = lower.map(|entry| entry.0).unwrap_or(0.0);
            let end = upper.map(|entry| entry.0).unwrap_or(1.0);
            let linear_progress = if end > start {
                ((progress - start) / (end - start)).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let eased_progress =
                super::super::transition::animation_timing_progress(timing, linear_progress);
            if let (Some(lower), Some(upper)) = (resolve(lower), resolve(upper)) {
                let current_color = properties
                    .get("color")
                    .map_or_else(|| "black".into(), ComputedValue::css_text);
                let value = super::super::transition::interpolate_property_with_color(
                    name,
                    &lower,
                    &upper,
                    eased_progress,
                    &current_color,
                )
                .unwrap_or_else(|| if eased_progress < 0.5 { lower } else { upper });
                let value = if let Some(registration) = self.registered_custom_properties.get(name)
                {
                    normalize_registered_numeric(value, &registration.syntax)
                } else {
                    value
                };
                let flow = logical_flow_from_properties(properties);
                insert_computed_property(properties, name, value, flow);
            }
        }
    }
}

fn declaration_in<'a>(step: &'a KeyframeStep, name: &str) -> Option<(f32, &'a Value)> {
    step.declarations
        .iter()
        .rev()
        .find(|declaration| declaration.name == name)
        .map(|declaration| (step.offset, &declaration.value))
}

#[cfg(test)]
mod keyframe_property_effect_tests {
    use super::*;

    fn step(offset: f32, names: &[&str]) -> KeyframeStep {
        KeyframeStep {
            offset,
            declarations: names
                .iter()
                .map(|name| Declaration {
                    name: (*name).to_owned(),
                    value: Value::Keyword("test".to_owned()),
                    important: false,
                })
                .collect(),
        }
    }

    #[test]
    fn timing_metadata_preserves_an_opacity_only_target() {
        let effects = classify_keyframe_properties(&[
            step(0.0, &["opacity", "animation-timing-function"]),
            step(0.5, &["animation-timing-function"]),
            step(1.0, &["opacity"]),
        ]);
        assert_eq!(
            effects,
            KeyframePropertyEffects {
                opacity_only: true,
                ..Default::default()
            }
        );
    }

    #[test]
    fn timing_metadata_does_not_request_layout_for_live_opacity_animation() {
        let node = NodeHandle::element("div");
        let mut resolver = StyleResolver::new();
        resolver.add_stylesheet(
            Origin::Author,
            crate::css::parse_stylesheet(
                "@keyframes fade { \
                   from { opacity: 0; animation-timing-function: steps(2) } \
                   to { opacity: 1 } \
                 } \
                 div { animation: fade 1s linear infinite }",
            )
            .unwrap(),
        );
        resolver.set_animation_time_ms(0.0);
        resolver.computed_style(&node);

        let effects = resolver.advance_style_clocks(500.0, true);
        assert!(effects.animation_changed);
        assert!(effects.animation_opacity_only);
        assert!(!effects.animation_requires_layout);
        assert!(!effects.requires_layout());
    }

    #[test]
    fn metadata_without_an_animated_target_is_not_opacity_only() {
        assert_eq!(
            classify_keyframe_properties(&[step(0.0, &["animation-timing-function"])]),
            KeyframePropertyEffects::default()
        );
    }

    #[test]
    fn non_metadata_properties_keep_conservative_classification() {
        for property in ["width", "unknown-property", "--custom"] {
            let effects = classify_keyframe_properties(&[step(0.0, &[property])]);
            assert!(effects.requires_layout, "{property}");
            assert!(!effects.opacity_only, "{property}");
            assert!(!effects.refreshes_visibility, "{property}");
        }

        assert_eq!(
            classify_keyframe_properties(&[step(0.0, &["visibility"])]),
            KeyframePropertyEffects {
                refreshes_visibility: true,
                ..Default::default()
            }
        );
    }
}

/// Samples discrete custom-property token streams without losing their source scopes.
/// The caller supplies resolved keyframes, timing, base values and registrations.
pub(super) fn sample_untyped_custom_values(
    steps: &[KeyframeStep],
    progress: f32,
    timing: &str,
    custom: &BTreeMap<String, Value>,
    registrations: &BTreeMap<String, RegisteredCustomProperty>,
    important: &HashSet<String>,
) -> BTreeMap<String, Value> {
    let names: std::collections::BTreeSet<_> = steps
        .iter()
        .flat_map(|step| {
            step.declarations
                .iter()
                .map(|declaration| declaration.name.as_str())
        })
        .filter(|name| name.starts_with("--") && !important.contains(*name))
        .filter(|name| {
            registrations.get(*name).is_none_or(|registration| {
                matches!(registration.syntax, RegisteredPropertySyntax::Universal)
            })
        })
        .collect();
    names
        .into_iter()
        .filter_map(|name| {
            let lower = steps
                .iter()
                .rev()
                .filter(|step| step.offset <= progress)
                .find_map(|step| declaration_in(step, name));
            let upper = steps
                .iter()
                .filter(|step| step.offset >= progress)
                .find_map(|step| declaration_in(step, name));
            let start = lower.map_or(0.0, |entry| entry.0);
            let end = upper.map_or(1.0, |entry| entry.0);
            let fraction = if end > start {
                ((progress - start) / (end - start)).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let eased = super::super::transition::animation_timing_progress(timing, fraction);
            let endpoint = if eased < 0.5 { lower } else { upper };
            endpoint
                .map(|entry| entry.1)
                .or_else(|| custom.get(name))
                .map(|value| (name.to_string(), value.clone()))
        })
        .collect()
}

#[cfg(test)]
mod property_name_diagnostic_tests {
    use super::*;

    #[test]
    fn diagnostic_names_are_sorted_owned_and_do_not_change_sampling() {
        let node = NodeHandle::element("div");
        let mut resolver = StyleResolver::new();
        resolver.add_stylesheet(
            Origin::Author,
            crate::css::parse_stylesheet(
                "@keyframes move {from{width:10px;opacity:0}to{width:20px;opacity:1}} \
             div{animation-name:move;animation-duration:1s;animation-timing-function:linear}",
            )
            .unwrap(),
        );
        resolver.set_animation_time_ms(0.0);
        resolver.computed_style(&node);
        let previous = resolver.animation_diagnostic_time_ms();
        assert!(resolver.set_animation_time_ms(500.0));
        let expected = vec!["opacity".to_owned(), "width".to_owned()];
        let names = resolver.animation_diagnostic_property_names(previous, |id| {
            (id == node.identity()).then(|| node.clone())
        });
        assert_eq!(names, (expected.clone(), expected, 0));
        assert_eq!(resolver.animation_diagnostic_time_ms(), Some(500.0));
        assert!(!resolver.set_animation_time_ms(500.0));
        assert_eq!(
            resolver
                .animation_diagnostic_property_names(previous, |_| None)
                .2,
            1
        );
    }

    #[test]
    fn background_position_animation_invalidates_paint_without_rebuilding_layout() {
        let node = NodeHandle::element("div");
        let unrelated = NodeHandle::element("p");
        let mut resolver = StyleResolver::new();
        resolver.add_stylesheet(
            Origin::Author,
            crate::css::parse_stylesheet(
                "@keyframes move {from{background-position:0px 0px}\
                 to{background-position:100px 50px}}\
                 div{animation:move 1s linear infinite}",
            )
            .unwrap(),
        );
        resolver.set_animation_time_ms(0.0);
        let before = resolver.computed_style_shared(&node);
        let unrelated_before = resolver.computed_style_shared(&unrelated);
        let effects = resolver.advance_style_clocks(750.0, true);
        let after = resolver.computed_style_shared(&node);
        let unrelated_after = resolver.computed_style_shared(&unrelated);

        assert!(effects.animation_changed);
        assert!(!effects.animation_requires_layout);
        assert!(!effects.requires_layout());
        assert!(!Arc::ptr_eq(&before, &after));
        assert!(Arc::ptr_eq(&unrelated_before, &unrelated_after));
        assert_ne!(
            before.get("background-position-x"),
            after.get("background-position-x")
        );
        assert_ne!(
            before.get("background-position-y"),
            after.get("background-position-y")
        );
    }
}
