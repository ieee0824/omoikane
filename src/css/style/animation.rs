//! Owned live CSS animation clocks; static resolvers retain snapshot rendering.

use super::*;

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
    fn running(&self, now_ms: f64) -> bool {
        self.paused_elapsed_ms.is_none()
            && (self.elapsed_ms(now_ms) / 1000.0) as f32 - self.spec.delay
                < self.spec.active_duration()
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct AnimationTimeline {
    now_ms: f64,
    elements: HashMap<(usize, Option<PseudoElement>), AnimationState>,
}

impl AnimationTimeline {
    fn set_time(&mut self, now_ms: f64) -> bool {
        let next = if now_ms.is_finite() {
            now_ms.max(self.now_ms)
        } else {
            self.now_ms
        };
        let changed = self
            .elements
            .values()
            .any(|state| state.progress(self.now_ms) != state.progress(next));
        self.now_ms = next;
        changed
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
    pub(super) fn apply_animation_effect(
        &self,
        node: &NodeHandle,
        pseudo: Option<PseudoElement>,
        properties: &mut PropertyMap,
        important: &HashSet<String>,
        scope: Option<usize>,
    ) {
        if self.animation_timeline.is_some() {
            self.apply_live_animation(node, pseudo, properties, important, scope);
        } else {
            self.apply_animation_snapshot(node, properties, important, scope);
        }
    }

    /// Enables explicit live sampling, preserving the clock and start times on
    /// cache rebuilds. This is an effect at the rendering lifecycle boundary.
    pub(crate) fn set_animation_time_ms(&mut self, time_ms: f64) -> bool {
        let timeline = self.animation_timeline.get_or_insert_with(Default::default);
        let changed = timeline.get_mut().set_time(time_ms);
        if changed {
            self.cache.clear();
            self.pseudo_cache.clear();
        }
        changed
    }
    pub(crate) fn take_animation_timeline(&mut self) -> Option<AnimationTimeline> {
        self.animation_timeline.take().map(RefCell::into_inner)
    }
    pub(crate) fn install_animation_timeline(&mut self, timeline: Option<AnimationTimeline>) {
        self.animation_timeline = timeline.map(RefCell::new);
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
    ) {
        let Some(timeline) = &self.animation_timeline else {
            return;
        };
        let name = match properties.get(&PropertyId::AnimationName) {
            Some(ComputedValue::Keyword(name) | ComputedValue::String(name)) => name.clone(),
            _ => String::new(),
        };
        let steps = self.keyframes_for(node, scope, &name);
        let hidden = matches!(
            properties.get(&PropertyId::Display),
            Some(ComputedValue::Keyword(display)) if display == "none"
        );
        let Some(steps) = steps.filter(|_| !hidden) else {
            timeline
                .borrow_mut()
                .elements
                .remove(&(node.identity(), pseudo));
            return;
        };
        let spec = AnimationSpec::from_properties(&name, scope, properties);
        let paused = matches!(
            properties.get(&PropertyId::AnimationPlayState),
            Some(ComputedValue::Keyword(value)) if value == "paused"
        );
        let progress = timeline
            .borrow_mut()
            .sample(node.identity(), pseudo, spec.clone(), paused);
        if let Some(progress) = progress {
            self.interpolate_live_keyframes(
                steps,
                progress,
                &spec.timing,
                self.color_container_size(node),
                properties,
                important,
            );
        }
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
        let names: HashSet<_> = steps
            .iter()
            .flat_map(|step| step.declarations.iter().map(|d| d.name.as_str()))
            .collect();
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
                let value = super::super::transition::interpolate_custom_property(
                    name,
                    &lower,
                    &upper,
                    eased_progress,
                )
                .unwrap_or_else(|| if eased_progress < 0.5 { lower } else { upper });
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
