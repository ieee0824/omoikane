//! CSS Transitions declaration parsing.
//!
//! Transition values use comma-separated lists whose commas must survive the
//! generic declaration parser. This module validates those lists and expands
//! the shorthand into its longhands. Timeline sampling lives above the
//! CSS parser and consumes the normalized longhand values produced here.

use std::collections::{BTreeSet, HashMap, HashSet};

#[cfg(test)]
mod activity_tests;

use crate::paint::color::{Color, parse_color};

use super::style::{PropertyId, PropertyMap};
use super::{
    ComputedValue, Declaration, LengthPercentageMath, Value, is_css_wide_keyword,
    split_top_level_whitespace_strict as split_top_level_whitespace,
};

#[derive(Debug, Clone, PartialEq)]
struct TransitionDescriptor {
    property: String,
    duration: String,
    timing_function: String,
    delay: String,
    behavior: String,
}

/// Per-document CSS transition state sampled by a [`super::StyleResolver`].
///
/// Base computed values are retained across style invalidations. Running
/// transitions contribute their sampled value after the normal cascade and CSS
/// animation snapshot have been resolved.
#[derive(Debug, Default)]
pub(crate) struct TransitionTimeline {
    now_ms: f64,
    elements: HashMap<usize, ElementTransitionState>,
    /// IDs with running transitions; inactive base snapshots stay in `elements`.
    running_nodes: HashSet<usize>,
    events: Vec<TransitionEventRecord>,
    #[cfg(test)]
    activity_scan_visits: std::cell::Cell<usize>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct TransitionEventRecord {
    pub node_id: usize,
    pub event_type: &'static str,
    pub property_name: String,
    pub elapsed_time: f64,
}

/// Bounded diagnostic counts for currently running transition properties.
///
/// The summary intentionally omits property names, values, selectors, and DOM
/// handles. Its categories are diagnostic labels and do not relax the existing
/// layout invalidation policy.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct TransitionPropertyCounts {
    pub(crate) nodes: usize,
    pub(crate) properties: usize,
    pub(crate) geometry: usize,
    pub(crate) transform: usize,
    pub(crate) color: usize,
    pub(crate) background: usize,
    pub(crate) filter: usize,
    pub(crate) opacity: usize,
    pub(crate) other: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TransitionPropertyCategory {
    Geometry,
    Transform,
    Color,
    Background,
    Filter,
    Opacity,
    Other,
}

/// The smallest computed-style cache region that can safely be discarded for
/// one running transition node.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum TransitionCacheScope {
    /// Invalidate the transition owner and its light-DOM descendants.
    Subtree,
    /// Invalidate every ordinary and pseudo computed style.
    Full,
}

/// Classifies a transitioned property by the computed-style cache region that
/// can observe a newly sampled value.
pub(crate) fn transition_cache_scope(property: &str) -> TransitionCacheScope {
    let Some(property_id) = PropertyId::parse(property) else {
        return TransitionCacheScope::Full;
    };
    if matches!(
        property_id,
        PropertyId::AccentColor
            | PropertyId::CaretColor
            | PropertyId::Color
            | PropertyId::BorderBlockEndColor
            | PropertyId::BorderBlockStartColor
            | PropertyId::BorderBottomColor
            | PropertyId::BorderInlineEndColor
            | PropertyId::BorderInlineStartColor
            | PropertyId::BorderLeftColor
            | PropertyId::BorderRightColor
            | PropertyId::BorderTopColor
            | PropertyId::ColumnRuleColor
            | PropertyId::OutlineColor
            | PropertyId::ScrollbarColor
            | PropertyId::TextDecorationColor
            | PropertyId::TextEmphasisColor
            | PropertyId::WebkitTextFillColor
            | PropertyId::WebkitTextStrokeColor
            | PropertyId::BackgroundAttachment
            | PropertyId::BackgroundBlendMode
            | PropertyId::BackgroundClip
            | PropertyId::BackgroundColor
            | PropertyId::BackgroundImage
            | PropertyId::BackgroundOrigin
            | PropertyId::BackgroundPositionX
            | PropertyId::BackgroundPositionY
            | PropertyId::BackgroundRepeat
            | PropertyId::BackgroundSize
            | PropertyId::Opacity
            | PropertyId::Filter
            | PropertyId::BackdropFilter
            | PropertyId::Transform
    ) {
        TransitionCacheScope::Subtree
    } else {
        TransitionCacheScope::Full
    }
}

/// Returns whether a sampled property can change layout geometry.
///
/// These properties are consumed only while painting. Keeping this shared
/// between transitions and keyframe animations prevents the two clocks from
/// assigning different invalidation costs to the same visual change.
pub(crate) fn animated_property_requires_layout(property: &str) -> bool {
    !matches!(
        property,
        "opacity"
            | "filter"
            | "backdrop-filter"
            | "background-position-x"
            | "background-position-y"
    )
}

/// Returns whether a running transition must rebuild layout geometry.
///
/// Color transitions can retain geometry because transition sampling already
/// invalidates the owner's full style subtree, including inherited `color` and
/// descendant `currentColor` users. Keyframe animations use
/// [`animated_property_requires_layout`] directly because their target-only
/// cache invalidation does not yet provide that inheritance guarantee.
fn transition_property_requires_layout(property: &str) -> bool {
    if !animated_property_requires_layout(property) {
        return false;
    }
    !matches!(
        PropertyId::parse(property),
        Some(
            PropertyId::Color
                | PropertyId::BorderBlockEndColor
                | PropertyId::BorderBlockStartColor
                | PropertyId::BorderBottomColor
                | PropertyId::BorderInlineEndColor
                | PropertyId::BorderInlineStartColor
                | PropertyId::BorderLeftColor
                | PropertyId::BorderRightColor
                | PropertyId::BorderTopColor
                | PropertyId::ColumnRuleColor
                | PropertyId::OutlineColor
                | PropertyId::TextDecorationColor
                | PropertyId::BackgroundColor
        )
    )
}

impl TransitionPropertyCounts {
    fn add(&mut self, property: &str) {
        self.properties += 1;
        match transition_property_category(property) {
            TransitionPropertyCategory::Geometry => self.geometry += 1,
            TransitionPropertyCategory::Transform => self.transform += 1,
            TransitionPropertyCategory::Color => self.color += 1,
            TransitionPropertyCategory::Background => self.background += 1,
            TransitionPropertyCategory::Filter => self.filter += 1,
            TransitionPropertyCategory::Opacity => self.opacity += 1,
            TransitionPropertyCategory::Other => self.other += 1,
        }
    }

    #[cfg(test)]
    fn categorized_properties(self) -> usize {
        self.geometry
            + self.transform
            + self.color
            + self.background
            + self.filter
            + self.opacity
            + self.other
    }
}

fn transition_property_category(property: &str) -> TransitionPropertyCategory {
    if property.starts_with("--") {
        return TransitionPropertyCategory::Other;
    }
    if property == "opacity" {
        TransitionPropertyCategory::Opacity
    } else if matches!(property, "filter" | "backdrop-filter") {
        TransitionPropertyCategory::Filter
    } else if matches!(property, "transform" | "translate" | "rotate" | "scale") {
        TransitionPropertyCategory::Transform
    } else if property.starts_with("background-") {
        TransitionPropertyCategory::Background
    } else if property == "color" || property.ends_with("-color") {
        TransitionPropertyCategory::Color
    } else if transition_property_is_geometry(property) {
        TransitionPropertyCategory::Geometry
    } else {
        TransitionPropertyCategory::Other
    }
}

fn transition_property_is_geometry(property: &str) -> bool {
    matches!(
        property,
        "width"
            | "height"
            | "min-width"
            | "max-width"
            | "min-height"
            | "max-height"
            | "top"
            | "right"
            | "bottom"
            | "left"
            | "inset"
            | "inset-inline"
            | "inset-inline-start"
            | "inset-inline-end"
            | "inset-block"
            | "inset-block-start"
            | "inset-block-end"
            | "flex-basis"
            | "flex-grow"
            | "flex-shrink"
            | "row-gap"
            | "column-gap"
            | "gap"
            | "font-size"
            | "font-weight"
            | "line-height"
            | "letter-spacing"
            | "word-spacing"
            | "text-indent"
    ) || property.starts_with("margin-")
        || property.starts_with("padding-")
        || (property.starts_with("border-") && property.ends_with("-width"))
}

#[derive(Debug, Default)]
struct ElementTransitionState {
    base_values: PropertyMap,
    running: HashMap<String, RunningTransition>,
    initialized: bool,
}

#[derive(Debug, Clone)]
struct RunningTransition {
    start_value: ComputedValue,
    end_value: ComputedValue,
    reversing_adjusted_start_value: ComputedValue,
    reversing_shortening_factor: f64,
    start_ms: f64,
    end_ms: f64,
    timing: TimingFunction,
    discrete: bool,
    started: bool,
}

#[derive(Debug, Clone)]
enum TimingFunction {
    Linear,
    LinearStops(Vec<(f64, f64)>),
    CubicBezier(f64, f64, f64, f64),
    Steps(u32, StepPosition),
}

#[derive(Debug, Clone, Copy)]
enum StepPosition {
    JumpStart,
    JumpEnd,
    JumpNone,
    JumpBoth,
}

#[derive(Debug, Clone)]
struct TransitionParameters {
    duration_ms: f64,
    delay_ms: f64,
    timing: TimingFunction,
    allow_discrete: bool,
}

impl TransitionTimeline {
    fn sample_time_advances(&self, time_ms: f64) -> bool {
        time_ms.is_finite() && time_ms.max(0.0) > self.now_ms
    }

    /// Whether this finite monotonic sample advances at least one transition.
    pub(crate) fn sample_will_advance(&self, time_ms: f64) -> bool {
        self.sample_time_advances(time_ms) && !self.running_nodes.is_empty()
    }

    /// Whether this advancing sample can localize every running transition and
    /// therefore needs document node lookup.
    ///
    /// A false result can mean either that the clock will not advance or that
    /// one running property requires full cache invalidation. Callers must
    /// still sample the clock.
    pub(crate) fn sample_needs_node_lookup(&self, time_ms: f64) -> bool {
        if !self.sample_time_advances(time_ms) {
            return false;
        }
        let mut has_running = false;
        for (_, state) in self.running_states() {
            for property in state.running.keys() {
                has_running = true;
                if transition_cache_scope(property) == TransitionCacheScope::Full {
                    return false;
                }
            }
        }
        has_running
    }

    pub(crate) fn set_time_ms(&mut self, time_ms: f64) -> bool {
        if time_ms.is_finite() {
            let next = self.now_ms.max(time_ms.max(0.0));
            if next > self.now_ms {
                self.now_ms = next;
                return !self.running_nodes.is_empty();
            }
        }
        false
    }

    pub(crate) fn sample(&mut self, node_id: usize, properties: &mut PropertyMap) {
        let state = self.elements.entry(node_id).or_default();
        sample_element_transitions(node_id, self.now_ms, state, properties, &mut self.events);
        let running = !state.running.is_empty();
        self.update_running_node(node_id, running);
    }

    #[cfg(test)]
    fn sample_without_fast_paths(&mut self, node_id: usize, properties: &mut PropertyMap) {
        let state = self.elements.entry(node_id).or_default();
        sample_element_without_fast_paths(
            node_id,
            self.now_ms,
            state,
            properties,
            &mut self.events,
        );
        let running = !state.running.is_empty();
        self.update_running_node(node_id, running);
    }

    pub(crate) fn take_events(&mut self) -> Vec<TransitionEventRecord> {
        std::mem::take(&mut self.events)
    }

    pub(crate) fn retain_nodes(&mut self, active_node_ids: &std::collections::HashSet<usize>) {
        let detached = self
            .elements
            .keys()
            .filter(|node_id| !active_node_ids.contains(node_id))
            .copied()
            .collect::<Vec<_>>();
        for node_id in detached {
            if let Some(state) = self.elements.remove(&node_id) {
                self.running_nodes.remove(&node_id);
                self.events
                    .extend(state.running.into_iter().map(|(property, running)| {
                        running.event_record(node_id, "transitioncancel", property, self.now_ms)
                    }));
            }
        }
    }

    pub(crate) fn running_node_ids(&self) -> Vec<usize> {
        self.running_nodes.iter().copied().collect()
    }

    /// Returns one strongest cache invalidation scope per running node.
    ///
    /// Callers collect this before advancing the clock so a transition that
    /// completes at the new sample time still has its final value recomputed.
    pub(crate) fn running_cache_scopes(&self) -> Vec<(usize, TransitionCacheScope)> {
        self.running_states()
            .filter_map(|(node_id, state)| {
                state
                    .running
                    .keys()
                    .map(|property| transition_cache_scope(property))
                    .max()
                    .map(|scope| (node_id, scope))
            })
            .collect()
    }

    pub(crate) fn has_running_transitions(&self) -> bool {
        !self.running_nodes.is_empty()
    }

    pub(crate) fn running_transitions_require_layout(&self) -> bool {
        self.running_states().any(|(_, state)| {
            state
                .running
                .keys()
                .any(|property| transition_property_requires_layout(property))
        })
    }

    fn update_running_node(&mut self, node_id: usize, running: bool) {
        if running {
            self.running_nodes.insert(node_id);
        } else {
            self.running_nodes.remove(&node_id);
        }
    }

    fn running_states(&self) -> impl Iterator<Item = (usize, &ElementTransitionState)> {
        self.running_nodes.iter().map(|node_id| {
            #[cfg(test)]
            self.activity_scan_visits
                .set(self.activity_scan_visits.get() + 1);
            (
                *node_id,
                self.elements
                    .get(node_id)
                    .expect("running transition retains its state"),
            )
        })
    }

    pub(crate) fn running_property_counts(&self) -> TransitionPropertyCounts {
        let mut counts = TransitionPropertyCounts::default();
        for state in self
            .elements
            .values()
            .filter(|state| !state.running.is_empty())
        {
            counts.nodes += 1;
            for property in state.running.keys() {
                counts.add(property);
            }
        }
        counts
    }

    pub(crate) fn cancel_detached_transitions(
        &mut self,
        active_node_ids: &std::collections::HashSet<usize>,
    ) {
        let detached = self
            .elements
            .iter()
            .filter_map(|(node_id, state)| {
                (!state.running.is_empty() && !active_node_ids.contains(node_id))
                    .then_some(*node_id)
            })
            .collect::<Vec<_>>();
        for node_id in detached {
            if let Some(state) = self.elements.remove(&node_id) {
                self.running_nodes.remove(&node_id);
                self.events
                    .extend(state.running.into_iter().map(|(property, running)| {
                        running.event_record(node_id, "transitioncancel", property, self.now_ms)
                    }));
            }
        }
    }
}

fn sample_element_transitions(
    node_id: usize,
    now_ms: f64,
    state: &mut ElementTransitionState,
    properties: &mut PropertyMap,
    events: &mut Vec<TransitionEventRecord>,
) {
    if !state.initialized {
        state.base_values = properties.clone();
        state.initialized = true;
        return;
    }
    // The base snapshot includes transition configuration. Equal snapshots
    // cannot start or cancel a transition, but existing ones still advance.
    if state.base_values == *properties {
        sample_running_transitions(node_id, now_ms, state, properties, events);
        return;
    }
    let configuration = TransitionConfiguration::from_properties(properties);
    if state.running.is_empty()
        && !configuration.may_start_transition(&state.base_values, properties)
    {
        // Keep the latest baseline even while transitions are disabled, so
        // enabling them later starts from the immediately preceding style.
        state.base_values = properties.clone();
        return;
    }
    cancel_unmatched_transitions(node_id, now_ms, state, &configuration, events);
    update_changed_transitions(node_id, now_ms, state, properties, &configuration, events);
    state.base_values = properties.clone();
    sample_running_transitions(node_id, now_ms, state, properties, events);
}

#[cfg(test)]
fn sample_element_without_fast_paths(
    node_id: usize,
    now_ms: f64,
    state: &mut ElementTransitionState,
    properties: &mut PropertyMap,
    events: &mut Vec<TransitionEventRecord>,
) {
    if !state.initialized {
        state.base_values = properties.clone();
        state.initialized = true;
        return;
    }
    let configuration = TransitionConfiguration::from_properties(properties);
    cancel_unmatched_transitions(node_id, now_ms, state, &configuration, events);
    update_changed_transitions(node_id, now_ms, state, properties, &configuration, events);
    state.base_values = properties.clone();
    sample_running_transitions(node_id, now_ms, state, properties, events);
}

fn cancel_unmatched_transitions(
    node_id: usize,
    now_ms: f64,
    state: &mut ElementTransitionState,
    configuration: &TransitionConfiguration,
    events: &mut Vec<TransitionEventRecord>,
) {
    let no_longer_matching: Vec<String> = state
        .running
        .keys()
        .filter(|property| configuration.matching_parameters(property).is_none())
        .cloned()
        .collect();
    for property in no_longer_matching {
        if let Some(running) = state.running.remove(&property) {
            events.push(running.event_record(node_id, "transitioncancel", property, now_ms));
        }
    }
}

fn update_changed_transitions(
    node_id: usize,
    now_ms: f64,
    state: &mut ElementTransitionState,
    properties: &PropertyMap,
    configuration: &TransitionConfiguration,
    events: &mut Vec<TransitionEventRecord>,
) {
    let mut changed_properties = BTreeSet::new();
    changed_properties.extend(state.base_values.keys().map(str::to_owned));
    changed_properties.extend(properties.keys().map(str::to_owned));
    changed_properties.retain(|property| {
        effective_value(property, state.base_values.get(property))
            != effective_value(property, properties.get(property))
    });
    for property in changed_properties {
        update_transition_for_property(
            node_id,
            now_ms,
            state,
            properties,
            configuration,
            events,
            property,
        );
    }
}

fn update_transition_for_property(
    node_id: usize,
    now_ms: f64,
    state: &mut ElementTransitionState,
    properties: &PropertyMap,
    configuration: &TransitionConfiguration,
    events: &mut Vec<TransitionEventRecord>,
    property: String,
) {
    let old_value = effective_value(&property, state.base_values.get(&property));
    let new_value = effective_value(&property, properties.get(&property));
    let Some(end_value) = new_value else {
        state.running.remove(&property);
        return;
    };
    if state
        .running
        .get(&property)
        .is_some_and(|running| running.end_value == end_value)
    {
        return;
    }
    let interrupted = state.running.get(&property).cloned();
    let start_value = interrupted
        .as_ref()
        .and_then(|running| {
            running.sample(
                &property,
                now_ms,
                &sampled_current_color(properties, &state.running, now_ms),
            )
        })
        .or(old_value);
    let Some(start_value) = start_value else {
        state.running.remove(&property);
        return;
    };
    if start_value == end_value {
        cancel_transition(node_id, now_ms, state, events, &property);
        return;
    }
    let Some(parameters) = configuration.matching_parameters(&property) else {
        cancel_transition(node_id, now_ms, state, events, &property);
        return;
    };
    let discrete = interpolate_property_with_color(
        &property,
        &start_value,
        &end_value,
        0.5,
        &sampled_current_color(properties, &state.running, now_ms),
    )
    .is_none();
    if parameters.duration_ms + parameters.delay_ms <= 0.0
        || (discrete && !parameters.allow_discrete)
    {
        cancel_transition(node_id, now_ms, state, events, &property);
        return;
    }
    if let Some(running) = interrupted.as_ref() {
        events.push(running.event_record(node_id, "transitioncancel", property.clone(), now_ms));
    }
    let running = RunningTransition::new(
        start_value,
        end_value,
        interrupted.as_ref(),
        parameters,
        discrete,
        now_ms,
    );
    events.push(running.event_record(node_id, "transitionrun", property.clone(), now_ms));
    if running.started {
        events.push(running.event_record(node_id, "transitionstart", property.clone(), now_ms));
    }
    state.running.insert(property, running);
}

fn cancel_transition(
    node_id: usize,
    now_ms: f64,
    state: &mut ElementTransitionState,
    events: &mut Vec<TransitionEventRecord>,
    property: &str,
) {
    if let Some(running) = state.running.remove(property) {
        events.push(running.event_record(node_id, "transitioncancel", property.to_owned(), now_ms));
    }
}

fn sampled_current_color(
    properties: &PropertyMap,
    running: &HashMap<String, RunningTransition>,
    now_ms: f64,
) -> String {
    running
        .get("color")
        .and_then(|transition| transition.sample("color", now_ms, "black"))
        .or_else(|| properties.get("color").cloned())
        .map_or_else(|| "black".into(), |value| value.css_text())
}

fn sample_running_transitions(
    node_id: usize,
    now_ms: f64,
    state: &mut ElementTransitionState,
    properties: &mut PropertyMap,
    events: &mut Vec<TransitionEventRecord>,
) {
    if state.running.is_empty() {
        return;
    }
    // Resolve currentcolor from the sampled color independently of map iteration order.
    let current_color = sampled_current_color(properties, &state.running, now_ms);
    let mut completed = Vec::new();
    for (property, running) in &mut state.running {
        if !running.started && now_ms >= running.start_ms {
            running.started = true;
            events.push(running.event_record(node_id, "transitionstart", property.clone(), now_ms));
        }
        if now_ms >= running.end_ms {
            properties.insert(property.clone(), running.end_value.clone());
            events.push(running.event_record(node_id, "transitionend", property.clone(), now_ms));
            completed.push(property.clone());
        } else if let Some(value) = running.sample(property, now_ms, &current_color) {
            properties.insert(property.clone(), value);
        }
    }
    for property in completed {
        state.running.remove(&property);
    }
}

impl RunningTransition {
    fn new(
        start_value: ComputedValue,
        end_value: ComputedValue,
        interrupted: Option<&Self>,
        parameters: TransitionParameters,
        discrete: bool,
        now_ms: f64,
    ) -> Self {
        let (reversing_adjusted_start_value, reversing_shortening_factor) = interrupted
            .filter(|running| running.reversing_adjusted_start_value == end_value)
            .map(|running| {
                let factor = (running.timing.sample(running.input_progress(now_ms))
                    * running.reversing_shortening_factor
                    + 1.0
                    - running.reversing_shortening_factor)
                    .abs()
                    .clamp(0.0, 1.0);
                (running.end_value.clone(), factor)
            })
            .unwrap_or_else(|| (start_value.clone(), 1.0));
        let delay_ms = if parameters.delay_ms < 0.0 {
            parameters.delay_ms * reversing_shortening_factor
        } else {
            parameters.delay_ms
        };
        let start_ms = now_ms + delay_ms;
        let end_ms = start_ms + parameters.duration_ms * reversing_shortening_factor;
        let started = now_ms >= start_ms;
        Self {
            start_value,
            end_value,
            reversing_adjusted_start_value,
            reversing_shortening_factor,
            start_ms,
            end_ms,
            timing: parameters.timing,
            discrete,
            started,
        }
    }

    fn input_progress(&self, now_ms: f64) -> f64 {
        if now_ms >= self.end_ms || self.end_ms < self.start_ms {
            1.0
        } else if now_ms <= self.start_ms {
            0.0
        } else {
            (now_ms - self.start_ms) / (self.end_ms - self.start_ms)
        }
    }

    fn sample(&self, property: &str, now_ms: f64, current_color: &str) -> Option<ComputedValue> {
        let progress = self.input_progress(now_ms);
        if self.discrete {
            if now_ms < self.start_ms {
                return Some(self.start_value.clone());
            }
            return Some(
                if discrete_uses_end_value(
                    property,
                    &self.start_value,
                    &self.end_value,
                    self.timing.sample(progress),
                ) {
                    self.end_value.clone()
                } else {
                    self.start_value.clone()
                },
            );
        }
        interpolate_property_with_color(
            property,
            &self.start_value,
            &self.end_value,
            self.timing.sample(progress) as f32,
            current_color,
        )
    }

    fn event_record(
        &self,
        node_id: usize,
        event_type: &'static str,
        property_name: String,
        now_ms: f64,
    ) -> TransitionEventRecord {
        let active_duration = (self.end_ms - self.start_ms).max(0.0);
        let elapsed_ms = (now_ms - self.start_ms).clamp(0.0, active_duration);
        TransitionEventRecord {
            node_id,
            event_type,
            property_name,
            elapsed_time: elapsed_ms / 1000.0,
        }
    }
}

fn discrete_uses_end_value(
    property: &str,
    start: &ComputedValue,
    end: &ComputedValue,
    progress: f64,
) -> bool {
    let hidden_value = match property {
        "display" => "none",
        "content-visibility" => "hidden",
        _ => return progress >= 0.5,
    };
    let start_hidden = start.css_text().eq_ignore_ascii_case(hidden_value);
    let end_hidden = end.css_text().eq_ignore_ascii_case(hidden_value);
    match (start_hidden, end_hidden) {
        (true, false) => true,
        (false, true) => progress >= 1.0,
        _ => progress >= 0.5,
    }
}

impl TimingFunction {
    fn parse(input: &str) -> Option<Self> {
        let normalized = normalize_timing_function(input)?;
        match normalized.as_str() {
            "linear" => Some(Self::Linear),
            "ease" => Some(Self::CubicBezier(0.25, 0.1, 0.25, 1.0)),
            "ease-in" => Some(Self::CubicBezier(0.42, 0.0, 1.0, 1.0)),
            "ease-out" => Some(Self::CubicBezier(0.0, 0.0, 0.58, 1.0)),
            "ease-in-out" => Some(Self::CubicBezier(0.42, 0.0, 0.58, 1.0)),
            "step-start" => Some(Self::Steps(1, StepPosition::JumpStart)),
            "step-end" => Some(Self::Steps(1, StepPosition::JumpEnd)),
            _ if normalized.starts_with("cubic-bezier(") => {
                let arguments = function_arguments(&normalized, "cubic-bezier")?;
                let values = split_top_level(arguments, ',')?;
                Some(Self::CubicBezier(
                    values[0].parse().ok()?,
                    values[1].parse().ok()?,
                    values[2].parse().ok()?,
                    values[3].parse().ok()?,
                ))
            }
            _ if normalized.starts_with("steps(") => {
                let arguments = function_arguments(&normalized, "steps")?;
                let values = split_top_level(arguments, ',')?;
                let count = values[0].parse().ok()?;
                let position = match values.get(1).map(|value| value.trim()).unwrap_or("end") {
                    "jump-start" | "start" => StepPosition::JumpStart,
                    "jump-end" | "end" => StepPosition::JumpEnd,
                    "jump-none" => StepPosition::JumpNone,
                    "jump-both" => StepPosition::JumpBoth,
                    _ => return None,
                };
                Some(Self::Steps(count, position))
            }
            _ if normalized.starts_with("linear(") => {
                Some(Self::LinearStops(parse_linear_stops(&normalized)?))
            }
            _ => None,
        }
    }

    fn sample(&self, progress: f64) -> f64 {
        let progress = progress.clamp(0.0, 1.0);
        match self {
            Self::Linear => progress,
            Self::LinearStops(stops) => sample_linear_stops(progress, stops),
            Self::CubicBezier(x1, y1, x2, y2) => sample_cubic_bezier(progress, *x1, *y1, *x2, *y2),
            Self::Steps(count, position) => sample_steps(progress, *count, *position),
        }
    }
}

struct TransitionConfiguration {
    properties: Vec<String>,
    durations_ms: Vec<f64>,
    timings: Vec<TimingFunction>,
    delays_ms: Vec<f64>,
    allow_discrete: Vec<bool>,
}

impl TransitionConfiguration {
    fn from_properties(properties: &PropertyMap) -> Self {
        let property_text = property_keyword(properties, "transition-property", "all");
        let duration_text = property_keyword(properties, "transition-duration", "0s");
        let timing_text = property_keyword(properties, "transition-timing-function", "ease");
        let delay_text = property_keyword(properties, "transition-delay", "0s");
        let behavior_text = property_keyword(properties, "transition-behavior", "normal");
        Self {
            properties: split_top_level(property_text, ',')
                .unwrap_or_default()
                .into_iter()
                .map(|value| value.trim().to_ascii_lowercase())
                .collect(),
            durations_ms: parse_time_list_ms(duration_text, false),
            timings: split_top_level(timing_text, ',')
                .unwrap_or_default()
                .into_iter()
                .filter_map(TimingFunction::parse)
                .collect(),
            delays_ms: parse_time_list_ms(delay_text, true),
            allow_discrete: split_top_level(behavior_text, ',')
                .unwrap_or_default()
                .into_iter()
                .map(|value| value.trim().eq_ignore_ascii_case("allow-discrete"))
                .collect(),
        }
    }

    fn may_start_transition(&self, old: &PropertyMap, new: &PropertyMap) -> bool {
        // Avoid even scanning property names for the common zero-time case.
        if !self.properties.iter().enumerate().any(|(index, _)| {
            self.parameters_at(index)
                .is_some_and(|parameters| parameters.duration_ms + parameters.delay_ms > 0.0)
        }) {
            return false;
        }
        old.keys().chain(new.keys()).any(|property| {
            self.matching_parameters(property)
                .is_some_and(|parameters| parameters.duration_ms + parameters.delay_ms > 0.0)
        })
    }

    fn matching_parameters(&self, property: &str) -> Option<TransitionParameters> {
        let index = self
            .properties
            .iter()
            .enumerate()
            .rev()
            .find_map(|(index, candidate)| {
                transition_property_matches(candidate, property).then_some(index)
            })?;
        self.parameters_at(index)
    }

    fn parameters_at(&self, index: usize) -> Option<TransitionParameters> {
        Some(TransitionParameters {
            duration_ms: *repeated(&self.durations_ms, index)?,
            delay_ms: *repeated(&self.delays_ms, index)?,
            timing: repeated(&self.timings, index)?.clone(),
            allow_discrete: *repeated(&self.allow_discrete, index)?,
        })
    }
}

fn transition_property_matches(candidate: &str, property: &str) -> bool {
    if candidate == "all" || candidate == property {
        return true;
    }
    match candidate {
        "margin" => property.starts_with("margin-"),
        "padding" => property.starts_with("padding-"),
        "margin-inline" => matches!(property, "margin-inline-start" | "margin-inline-end"),
        "margin-block" => matches!(property, "margin-block-start" | "margin-block-end"),
        "padding-inline" => matches!(property, "padding-inline-start" | "padding-inline-end"),
        "padding-block" => matches!(property, "padding-block-start" | "padding-block-end"),
        "border-width" => property.starts_with("border-") && property.ends_with("-width"),
        "border-color" => property.starts_with("border-") && property.ends_with("-color"),
        "border-style" => property.starts_with("border-") && property.ends_with("-style"),
        "border" => {
            property.starts_with("border-")
                && (property.ends_with("-width")
                    || property.ends_with("-color")
                    || property.ends_with("-style"))
        }
        "background" => property.starts_with("background-"),
        "flex" => matches!(property, "flex-grow" | "flex-shrink" | "flex-basis"),
        "flex-flow" => matches!(property, "flex-direction" | "flex-wrap"),
        "overflow" => matches!(property, "overflow-x" | "overflow-y"),
        "border-radius" => property.starts_with("border-") && property.ends_with("-radius"),
        "outline" => property.starts_with("outline-"),
        "grid-column" => matches!(property, "grid-column-start" | "grid-column-end"),
        "grid-row" => matches!(property, "grid-row-start" | "grid-row-end"),
        "place-items" => matches!(property, "align-items" | "justify-items"),
        "place-self" => matches!(property, "align-self" | "justify-self"),
        "place-content" => matches!(property, "align-content" | "justify-content"),
        _ => false,
    }
}

fn property_keyword<'a>(properties: &'a PropertyMap, name: &str, fallback: &'a str) -> &'a str {
    match properties.get(name) {
        Some(ComputedValue::Keyword(value)) => value,
        _ => fallback,
    }
}

fn repeated<T>(values: &[T], index: usize) -> Option<&T> {
    (!values.is_empty()).then(|| &values[index % values.len()])
}

fn parse_time_list_ms(input: &str, allow_negative: bool) -> Vec<f64> {
    split_top_level(input, ',')
        .unwrap_or_default()
        .into_iter()
        .filter_map(|value| time_to_ms(value, allow_negative))
        .collect()
}

fn time_to_ms(input: &str, allow_negative: bool) -> Option<f64> {
    let normalized = normalize_time(input, allow_negative)?;
    if let Some(value) = normalized.strip_suffix("ms") {
        value.parse().ok()
    } else {
        normalized
            .strip_suffix('s')?
            .parse::<f64>()
            .ok()
            .map(|value| value * 1000.0)
    }
}

fn effective_value(property: &str, value: Option<&ComputedValue>) -> Option<ComputedValue> {
    value.cloned().or_else(|| match property {
        "opacity" => Some(ComputedValue::Number(1.0)),
        "filter" | "backdrop-filter" => Some(ComputedValue::Keyword("none".into())),
        "flex-grow" => Some(ComputedValue::Number(0.0)),
        "flex-shrink" => Some(ComputedValue::Number(1.0)),
        property
            if property.starts_with("margin-")
                || property.starts_with("padding-")
                || (property.starts_with("border-") && property.ends_with("-width"))
                || property.ends_with("-radius") =>
        {
            Some(ComputedValue::Px(0.0))
        }
        _ => None,
    })
}

fn interpolate_property(
    property: &str,
    start: &ComputedValue,
    end: &ComputedValue,
    progress: f32,
) -> Option<ComputedValue> {
    let mix = |start: f32, end: f32| start + (end - start) * progress;
    match (start, end) {
        (ComputedValue::Number(start), ComputedValue::Number(end))
            if property.starts_with("--") || is_transitionable_number_property(property) =>
        {
            Some(ComputedValue::Number(mix(*start, *end)))
        }
        (ComputedValue::Px(start), ComputedValue::Px(end)) => {
            Some(ComputedValue::Px(mix(*start, *end)))
        }
        (ComputedValue::Percentage(start), ComputedValue::Percentage(end)) => {
            Some(ComputedValue::Percentage(mix(*start, *end)))
        }
        _ if length_components(start).is_some() && length_components(end).is_some() => {
            let (start_px, start_percent) = length_components(start)?;
            let (end_px, end_percent) = length_components(end)?;
            let px = mix(start_px, end_px);
            let percentage = mix(start_percent, end_percent);
            Some(if percentage == 0.0 {
                ComputedValue::Px(px)
            } else if px == 0.0 {
                ComputedValue::Percentage(percentage)
            } else {
                ComputedValue::LengthPercentage(LengthPercentageMath::Linear { px, percentage })
            })
        }
        (ComputedValue::Color(start), ComputedValue::Color(end))
            if property.starts_with("--")
                || property == "color"
                || property.ends_with("-color") =>
        {
            let start = parse_color(start)?;
            let end = parse_color(end)?;
            Some(ComputedValue::Color(interpolate_color(
                start, end, progress,
            )))
        }
        (ComputedValue::Keyword(start), ComputedValue::Keyword(end))
            if matches!(property, "translate" | "rotate" | "scale") =>
        {
            Some(ComputedValue::Keyword(
                super::style::individual_transform::interpolate(property, start, end, progress)?,
            ))
        }
        (ComputedValue::Keyword(start), ComputedValue::Keyword(end)) if property == "transform" => {
            Some(ComputedValue::Keyword(super::interpolate_transform_lists(
                start, end, progress,
            )?))
        }
        (ComputedValue::Keyword(start), ComputedValue::Keyword(end))
            if matches!(property, "filter" | "backdrop-filter") =>
        {
            Some(ComputedValue::Keyword(super::interpolate_filter_lists(
                start, end, progress,
            )?))
        }
        _ => None,
    }
}

pub(crate) fn interpolate_property_with_color(
    property: &str,
    start: &ComputedValue,
    end: &ComputedValue,
    progress: f32,
    current_color: &str,
) -> Option<ComputedValue> {
    if property == "text-shadow" {
        return super::style::text_shadow::interpolate(
            &start.css_text(),
            &end.css_text(),
            progress,
            current_color,
        )
        .map(ComputedValue::Keyword);
    }
    interpolate_property(property, start, end, progress)
}

pub(crate) fn interpolate_custom_property(
    property: &str,
    start: &ComputedValue,
    end: &ComputedValue,
    progress: f32,
) -> Option<ComputedValue> {
    interpolate_property(property, start, end, progress)
}

fn length_components(value: &ComputedValue) -> Option<(f32, f32)> {
    value.linear_length_percentage_components()
}

fn is_transitionable_number_property(property: &str) -> bool {
    matches!(
        property,
        "opacity" | "flex-grow" | "flex-shrink" | "line-height" | "font-weight"
    )
}

fn interpolate_color(start: Color, end: Color, progress: f32) -> String {
    let start_alpha = start.a as f32 / 255.0;
    let end_alpha = end.a as f32 / 255.0;
    let alpha = start_alpha + (end_alpha - start_alpha) * progress;
    let channel = |start: u8, end: u8| {
        let premultiplied_start = start as f32 * start_alpha;
        let premultiplied_end = end as f32 * end_alpha;
        let premultiplied =
            premultiplied_start + (premultiplied_end - premultiplied_start) * progress;
        if alpha <= f32::EPSILON {
            0
        } else {
            (premultiplied / alpha).round().clamp(0.0, 255.0) as u8
        }
    };
    let red = channel(start.r, end.r);
    let green = channel(start.g, end.g);
    let blue = channel(start.b, end.b);
    if alpha >= 1.0 - f32::EPSILON {
        format!("rgb({red}, {green}, {blue})")
    } else {
        format!("rgba({red}, {green}, {blue}, {})", format_number(alpha))
    }
}

fn sample_cubic_bezier(progress: f64, x1: f64, y1: f64, x2: f64, y2: f64) -> f64 {
    let coordinate = |time: f64, first: f64, second: f64| {
        let inverse = 1.0 - time;
        3.0 * inverse * inverse * time * first
            + 3.0 * inverse * time * time * second
            + time * time * time
    };
    let mut low = 0.0;
    let mut high = 1.0;
    for _ in 0..20 {
        let middle = (low + high) * 0.5;
        if coordinate(middle, x1, x2) < progress {
            low = middle;
        } else {
            high = middle;
        }
    }
    coordinate((low + high) * 0.5, y1, y2)
}

fn sample_steps(progress: f64, count: u32, position: StepPosition) -> f64 {
    let count = count as f64;
    let value = match position {
        StepPosition::JumpStart => (progress * count).floor() + 1.0,
        StepPosition::JumpEnd => (progress * count).floor(),
        StepPosition::JumpNone => (progress * count).floor() / (count - 1.0) * count,
        StepPosition::JumpBoth => (progress * count).floor() + 1.0,
    };
    let denominator = match position {
        StepPosition::JumpBoth => count + 1.0,
        _ => count,
    };
    (value / denominator).clamp(0.0, 1.0)
}

pub(crate) fn expand_transition_shorthand(value: Value, important: bool) -> Vec<Declaration> {
    let Value::Keyword(input) = value else {
        return Vec::new();
    };
    let lower = input.trim().to_ascii_lowercase();
    if is_css_wide_keyword(&lower) {
        return transition_longhands()
            .into_iter()
            .map(|name| Declaration {
                name: name.to_string(),
                value: Value::Keyword(lower.clone()),
                important,
            })
            .collect();
    }
    let Some(descriptors) = parse_transition_shorthand(&input) else {
        return Vec::new();
    };

    let join = |select: fn(&TransitionDescriptor) -> &str| {
        descriptors
            .iter()
            .map(select)
            .collect::<Vec<_>>()
            .join(", ")
    };
    vec![
        Declaration {
            name: "transition-property".to_string(),
            value: Value::Keyword(join(|item| &item.property)),
            important,
        },
        Declaration {
            name: "transition-duration".to_string(),
            value: Value::Keyword(join(|item| &item.duration)),
            important,
        },
        Declaration {
            name: "transition-timing-function".to_string(),
            value: Value::Keyword(join(|item| &item.timing_function)),
            important,
        },
        Declaration {
            name: "transition-delay".to_string(),
            value: Value::Keyword(join(|item| &item.delay)),
            important,
        },
        Declaration {
            name: "transition-behavior".to_string(),
            value: Value::Keyword(join(|item| &item.behavior)),
            important,
        },
    ]
}

pub(crate) fn normalize_transition_shorthand(input: &str) -> Option<String> {
    let lower = input.trim().to_ascii_lowercase();
    if is_css_wide_keyword(&lower) {
        return Some(lower);
    }
    let descriptors = parse_transition_shorthand(input)?;
    let mut properties = PropertyMap::new();
    let join = |select: fn(&TransitionDescriptor) -> &str| {
        descriptors
            .iter()
            .map(select)
            .collect::<Vec<_>>()
            .join(", ")
    };
    properties.insert(
        "transition-property".to_string(),
        ComputedValue::Keyword(join(|item| &item.property)),
    );
    properties.insert(
        "transition-duration".to_string(),
        ComputedValue::Keyword(join(|item| &item.duration)),
    );
    properties.insert(
        "transition-timing-function".to_string(),
        ComputedValue::Keyword(join(|item| &item.timing_function)),
    );
    properties.insert(
        "transition-delay".to_string(),
        ComputedValue::Keyword(join(|item| &item.delay)),
    );
    properties.insert(
        "transition-behavior".to_string(),
        ComputedValue::Keyword(join(|item| &item.behavior)),
    );
    Some(computed_transition_shorthand(&properties))
}

pub(crate) fn normalize_transition_longhand(name: &str, value: &str) -> Option<String> {
    let input = value.trim();
    let lower = input.to_ascii_lowercase();
    if is_css_wide_keyword(&lower) {
        return Some(lower);
    }
    let items = split_top_level(input, ',')?;
    if items.is_empty() {
        return None;
    }
    let normalized = match name {
        "transition-property" => {
            let mut properties = Vec::with_capacity(items.len());
            for item in items {
                properties.push(normalize_property_name(item)?);
            }
            if properties.len() > 1 && properties.iter().any(|item| item == "none") {
                return None;
            }
            properties
        }
        "transition-duration" => items
            .into_iter()
            .map(|item| normalize_time(item, false))
            .collect::<Option<Vec<_>>>()?,
        "transition-delay" => items
            .into_iter()
            .map(|item| normalize_time(item, true))
            .collect::<Option<Vec<_>>>()?,
        "transition-timing-function" => items
            .into_iter()
            .map(normalize_timing_function)
            .collect::<Option<Vec<_>>>()?,
        "transition-behavior" => items
            .into_iter()
            .map(|item| match item.trim().to_ascii_lowercase().as_str() {
                "normal" => Some("normal".to_string()),
                "allow-discrete" => Some("allow-discrete".to_string()),
                _ => None,
            })
            .collect::<Option<Vec<_>>>()?,
        _ => return None,
    };
    Some(normalized.join(", "))
}

pub(crate) fn computed_transition_longhand(name: &str, value: &str) -> Option<String> {
    let normalized = normalize_transition_longhand(name, value)?;
    if is_css_wide_keyword(&normalized) {
        return Some(normalized);
    }
    if matches!(name, "transition-duration" | "transition-delay") {
        return Some(
            split_top_level(&normalized, ',')?
                .into_iter()
                .map(|value| {
                    let seconds = time_to_ms(value, name == "transition-delay")? / 1000.0;
                    Some(format!("{}s", format_number(seconds as f32)))
                })
                .collect::<Option<Vec<_>>>()?
                .join(", "),
        );
    }
    Some(normalized)
}

fn parse_transition_shorthand(input: &str) -> Option<Vec<TransitionDescriptor>> {
    let items = split_top_level(input, ',')?;
    if items.is_empty() {
        return None;
    }
    let mut descriptors = Vec::with_capacity(items.len());
    for item in items {
        let components = split_top_level_whitespace(item)?;
        if components.is_empty() {
            return None;
        }
        let component_count = components.len();
        let mut property = None;
        let mut duration = None;
        let mut timing_function = None;
        let mut delay = None;
        let mut behavior = None;
        for component in components {
            if let Some(time) = normalize_time(component, delay.is_none()) {
                if duration.is_none() {
                    if time.starts_with('-') {
                        return None;
                    }
                    duration = Some(time);
                } else if delay.is_none() {
                    delay = normalize_time(component, true);
                } else {
                    return None;
                }
                continue;
            }
            if timing_function.is_none()
                && let Some(timing) = normalize_timing_function(component)
            {
                timing_function = Some(timing);
                continue;
            }
            if behavior.is_none()
                && matches!(
                    component.to_ascii_lowercase().as_str(),
                    "normal" | "allow-discrete"
                )
            {
                behavior = Some(component.to_ascii_lowercase());
                continue;
            }
            if property.is_none()
                && let Some(candidate) = normalize_property_name(component)
            {
                property = Some(candidate);
                continue;
            }
            return None;
        }
        let property = property.unwrap_or_else(|| "all".to_string());
        // `none` is the alternative to the whole `<single-transition>#`
        // grammar, not a property name that can be combined with timings.
        if property == "none" && component_count != 1 {
            return None;
        }
        descriptors.push(TransitionDescriptor {
            property,
            duration: duration.unwrap_or_else(|| "0s".to_string()),
            timing_function: timing_function.unwrap_or_else(|| "ease".to_string()),
            delay: delay.unwrap_or_else(|| "0s".to_string()),
            behavior: behavior.unwrap_or_else(|| "normal".to_string()),
        });
    }
    if descriptors.len() > 1 && descriptors.iter().any(|item| item.property == "none") {
        return None;
    }
    Some(descriptors)
}

pub(crate) fn computed_transition_shorthand(properties: &PropertyMap) -> String {
    let property_items = split_keyword_property(properties, "transition-property", "all");
    if property_items.as_slice() == ["none"] {
        return "none".to_string();
    }
    let durations = split_keyword_property(properties, "transition-duration", "0s");
    let timings = split_keyword_property(properties, "transition-timing-function", "ease");
    let delays = split_keyword_property(properties, "transition-delay", "0s");
    let behaviors = split_keyword_property(properties, "transition-behavior", "normal");
    property_items
        .iter()
        .enumerate()
        .map(|(index, property)| {
            let duration = repeated(&durations, index)
                .map(String::as_str)
                .unwrap_or("0s");
            let timing = repeated(&timings, index)
                .map(String::as_str)
                .unwrap_or("ease");
            let delay = repeated(&delays, index).map(String::as_str).unwrap_or("0s");
            let behavior = repeated(&behaviors, index)
                .map(String::as_str)
                .unwrap_or("normal");
            let duration_is_zero = time_to_ms(duration, false) == Some(0.0);
            let delay_is_zero = time_to_ms(delay, true) == Some(0.0);
            let timing_is_ease = timing.eq_ignore_ascii_case("ease");
            let mut components = Vec::new();
            if property != "all" || (duration_is_zero && timing_is_ease && delay_is_zero) {
                components.push(property.as_str());
            }
            if !duration_is_zero || !delay_is_zero {
                components.push(duration);
            }
            if !timing_is_ease {
                components.push(timing);
            }
            if !delay_is_zero {
                components.push(delay);
            }
            if behavior != "normal" {
                components.push(behavior);
            }
            if components.is_empty() {
                "all".to_string()
            } else {
                components.join(" ")
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn split_keyword_property(properties: &PropertyMap, name: &str, fallback: &str) -> Vec<String> {
    split_top_level(property_keyword(properties, name, fallback), ',')
        .unwrap_or_default()
        .into_iter()
        .map(|value| value.trim().to_string())
        .collect()
}

fn transition_longhands() -> [&'static str; 5] {
    [
        "transition-property",
        "transition-duration",
        "transition-timing-function",
        "transition-delay",
        "transition-behavior",
    ]
}

fn split_top_level(input: &str, separator: char) -> Option<Vec<&str>> {
    let mut values = Vec::new();
    let mut depth = 0usize;
    let mut start = 0usize;
    for (index, ch) in input.char_indices() {
        match ch {
            '(' => depth = depth.checked_add(1)?,
            ')' => depth = depth.checked_sub(1)?,
            _ if ch == separator && depth == 0 => {
                let value = input[start..index].trim();
                if value.is_empty() {
                    return None;
                }
                values.push(value);
                start = index + ch.len_utf8();
            }
            _ => {}
        }
    }
    if depth != 0 {
        return None;
    }
    let value = input[start..].trim();
    if value.is_empty() {
        return None;
    }
    values.push(value);
    Some(values)
}

fn normalize_time(input: &str, allow_negative: bool) -> Option<String> {
    let lower = input.trim().to_ascii_lowercase();
    let (number, unit) = if let Some(number) = lower.strip_suffix("ms") {
        (number, "ms")
    } else if let Some(number) = lower.strip_suffix('s') {
        (number, "s")
    } else {
        return None;
    };
    let parsed = number.parse::<f32>().ok()?;
    if !parsed.is_finite() || (!allow_negative && parsed < 0.0) {
        return None;
    }
    Some(format_number(parsed) + unit)
}

fn normalize_timing_function(input: &str) -> Option<String> {
    let lower = input.trim().to_ascii_lowercase();
    if matches!(
        lower.as_str(),
        "linear" | "ease" | "ease-in" | "ease-out" | "ease-in-out"
    ) {
        return Some(lower);
    }
    if lower == "step-start" {
        return Some("steps(1, start)".to_string());
    }
    if lower == "step-end" {
        return Some("steps(1)".to_string());
    }
    if function_arguments(&lower, "linear").is_some() {
        let stops = parse_linear_stops(&lower)?;
        return Some(format!(
            "linear({})",
            stops
                .iter()
                .map(|(output, input)| format!(
                    "{} {}%",
                    format_number(*output as f32),
                    format_number((*input * 100.0) as f32)
                ))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if let Some(arguments) = function_arguments(&lower, "cubic-bezier") {
        let values = split_top_level(arguments, ',')?;
        if values.len() != 4 {
            return None;
        }
        let numbers = values
            .iter()
            .map(|value| value.parse::<f32>().ok())
            .collect::<Option<Vec<_>>>()?;
        if numbers.iter().any(|number| !number.is_finite())
            || !(0.0..=1.0).contains(&numbers[0])
            || !(0.0..=1.0).contains(&numbers[2])
        {
            return None;
        }
        return Some(format!(
            "cubic-bezier({}, {}, {}, {})",
            format_number(numbers[0]),
            format_number(numbers[1]),
            format_number(numbers[2]),
            format_number(numbers[3])
        ));
    }
    if let Some(arguments) = function_arguments(&lower, "steps") {
        let values = split_top_level(arguments, ',')?;
        if values.is_empty() || values.len() > 2 {
            return None;
        }
        let count = values[0].parse::<u32>().ok()?;
        if count == 0 {
            return None;
        }
        if values.len() == 1 {
            return Some(format!("steps({count})"));
        }
        let position = values[1].trim();
        if !matches!(
            position,
            "jump-start" | "jump-end" | "jump-none" | "jump-both" | "start" | "end"
        ) || (position == "jump-none" && count < 2)
        {
            return None;
        }
        if matches!(position, "end" | "jump-end") {
            return Some(format!("steps({count})"));
        }
        return Some(format!("steps({count}, {position})"));
    }
    None
}

fn parse_linear_stops(input: &str) -> Option<Vec<(f64, f64)>> {
    let arguments = function_arguments(input, "linear")?;
    let items = split_top_level(arguments, ',')?;
    if items.len() < 2 {
        return None;
    }
    let mut stops = Vec::new();
    for item in items {
        let components = split_top_level_whitespace(item.trim())?;
        if components.is_empty() || components.len() > 3 {
            return None;
        }
        let output = components[0].parse::<f64>().ok()?;
        if !output.is_finite() {
            return None;
        }
        let mut positions = components[1..]
            .iter()
            .map(|component| {
                let value = component.strip_suffix('%')?.parse::<f64>().ok()? / 100.0;
                value.is_finite().then_some(value)
            })
            .collect::<Option<Vec<_>>>()?;
        if positions.len() == 2 && positions[0] > positions[1] {
            return None;
        }
        if positions.is_empty() {
            stops.push((output, f64::NAN));
        } else {
            for position in positions.drain(..) {
                stops.push((output, position));
            }
        }
    }
    if stops[0].1.is_nan() {
        stops[0].1 = 0.0;
    }
    if stops.last()?.1.is_nan() {
        stops.last_mut()?.1 = 1.0;
    }
    let mut index = 1;
    while index < stops.len() {
        if !stops[index].1.is_nan() {
            index += 1;
            continue;
        }
        let start = index - 1;
        let mut end = index;
        while end < stops.len() && stops[end].1.is_nan() {
            end += 1;
        }
        let from = stops[start].1;
        let to = stops.get(end)?.1;
        for missing in index..end {
            stops[missing].1 = from + (to - from) * (missing - start) as f64 / (end - start) as f64;
        }
        index = end + 1;
    }
    let mut previous = f64::NEG_INFINITY;
    for (_, position) in &mut stops {
        *position = position.max(previous);
        previous = *position;
    }
    Some(stops)
}

fn sample_linear_stops(progress: f64, stops: &[(f64, f64)]) -> f64 {
    let first = stops[0];
    let last = stops[stops.len() - 1];
    if progress <= first.1 {
        let next = stops
            .iter()
            .copied()
            .find(|stop| stop.1 > first.1)
            .unwrap_or(last);
        return interpolate_linear_stop(progress, first, next);
    }
    if progress >= last.1 {
        let previous = stops
            .iter()
            .rev()
            .copied()
            .find(|stop| stop.1 < last.1)
            .unwrap_or(first);
        return interpolate_linear_stop(progress, previous, last);
    }
    let right = stops.partition_point(|stop| stop.1 <= progress);
    interpolate_linear_stop(progress, stops[right - 1], stops[right])
}

fn interpolate_linear_stop(progress: f64, from: (f64, f64), to: (f64, f64)) -> f64 {
    if to.1 == from.1 {
        return to.0;
    }
    from.0 + (to.0 - from.0) * (progress - from.1) / (to.1 - from.1)
}

fn function_arguments<'a>(input: &'a str, name: &str) -> Option<&'a str> {
    input
        .strip_prefix(name)?
        .strip_prefix('(')?
        .strip_suffix(')')
}

fn is_transition_property_name(input: &str) -> bool {
    let lower = input.to_ascii_lowercase();
    if input.is_empty() || is_css_wide_keyword(&lower) || lower == "default" {
        return false;
    }
    if lower == "all" || lower == "none" {
        return true;
    }
    let mut chars = input.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first.is_ascii_alphabetic() || first == '_' || first == '-')
        && chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
}

fn normalize_property_name(input: &str) -> Option<String> {
    let input = input.trim();
    if !is_transition_property_name(input) {
        return None;
    }
    let lower = input.to_ascii_lowercase();
    if matches!(lower.as_str(), "all" | "none") || super::style::is_supported_property(&lower) {
        Some(lower)
    } else {
        Some(input.to_string())
    }
}

fn format_number(value: f32) -> String {
    if value == 0.0 {
        "0".to_string()
    } else if value.fract() == 0.0 {
        format!("{}", value as i64)
    } else {
        value.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn timeline_style(
        opacity: Option<f32>,
        property: &str,
        duration: &str,
        delay: &str,
    ) -> PropertyMap {
        let mut values = PropertyMap::new();
        if let Some(opacity) = opacity {
            values.insert("opacity", ComputedValue::Number(opacity));
        }
        values.insert("width", ComputedValue::Px(16.0));
        for (name, value) in [
            ("transition-property", property),
            ("transition-duration", duration),
            ("transition-delay", delay),
            ("transition-timing-function", "linear"),
        ] {
            values.insert(name, ComputedValue::Keyword(value.into()));
        }
        values
    }

    fn assert_timeline_state_matches(actual: &TransitionTimeline, expected: &TransitionTimeline) {
        assert_eq!(actual.now_ms, expected.now_ms);
        assert_eq!(actual.elements.len(), expected.elements.len());
        assert_eq!(actual.running_nodes, expected.running_nodes);
        for (id, state) in &actual.elements {
            let expected = &expected.elements[id];
            assert_eq!(state.initialized, expected.initialized);
            assert_eq!(state.base_values, expected.base_values);
            assert_eq!(state.running.len(), expected.running.len());
            for (property, running) in &state.running {
                let expected = &expected.running[property];
                assert_eq!(running.start_value, expected.start_value);
                assert_eq!(running.end_value, expected.end_value);
                assert_eq!(
                    running.reversing_adjusted_start_value,
                    expected.reversing_adjusted_start_value
                );
                assert_eq!(
                    running.reversing_shortening_factor,
                    expected.reversing_shortening_factor
                );
                assert_eq!(running.start_ms, expected.start_ms);
                assert_eq!(running.end_ms, expected.end_ms);
                assert_eq!(running.started, expected.started);
                assert_eq!(running.discrete, expected.discrete);
                for progress in [0.0, 0.25, 0.5, 1.0] {
                    assert_eq!(
                        running.timing.sample(progress),
                        expected.timing.sample(progress)
                    );
                }
            }
        }
    }

    fn compare_timeline_step(
        actual: &mut TransitionTimeline,
        expected: &mut TransitionTimeline,
        now_ms: f64,
        base: &PropertyMap,
    ) -> (PropertyMap, Vec<TransitionEventRecord>) {
        assert_eq!(actual.set_time_ms(now_ms), expected.set_time_ms(now_ms));
        let mut actual_values = base.clone();
        let mut expected_values = base.clone();
        actual.sample(1, &mut actual_values);
        expected.sample_without_fast_paths(1, &mut expected_values);
        assert_eq!(actual_values, expected_values);
        assert_timeline_state_matches(actual, expected);
        let events = actual.take_events();
        assert_eq!(events, expected.take_events());
        (actual_values, events)
    }

    #[test]
    fn static_and_disabled_transitions_keep_latest_base_when_enabled_later() {
        let mut actual = TransitionTimeline::default();
        let mut expected = TransitionTimeline::default();
        for (time, opacity) in [
            (0.0, Some(0.0)),
            (10.0, Some(0.2)),
            (20.0, None),
            (30.0, Some(0.4)),
        ] {
            let style = timeline_style(opacity, "none", "1s", "0s");
            assert!(
                compare_timeline_step(&mut actual, &mut expected, time, &style)
                    .1
                    .is_empty()
            );
        }
        let enabled = timeline_style(Some(0.4), "opacity", "1s", "0s");
        assert!(
            compare_timeline_step(&mut actual, &mut expected, 40.0, &enabled)
                .1
                .is_empty()
        );
        let target = timeline_style(Some(0.8), "opacity", "1s", "0s");
        let (values, events) = compare_timeline_step(&mut actual, &mut expected, 50.0, &target);
        assert_eq!(values.get("opacity"), Some(&ComputedValue::Number(0.4)));
        assert_eq!(
            events
                .iter()
                .map(|event| event.event_type)
                .collect::<Vec<_>>(),
            ["transitionrun", "transitionstart"]
        );
        let (values, events) = compare_timeline_step(&mut actual, &mut expected, 550.0, &target);
        assert_eq!(values.get("opacity"), Some(&ComputedValue::Number(0.6)));
        assert!(events.is_empty());
        let (_, events) = compare_timeline_step(&mut actual, &mut expected, 1050.0, &target);
        assert_eq!(events[0].event_type, "transitionend");
        assert!(
            compare_timeline_step(&mut actual, &mut expected, 1100.0, &target)
                .1
                .is_empty()
        );
    }

    #[test]
    fn no_effective_transition_skips_diff_without_losing_fallback_baselines() {
        for (property, duration, delay) in [
            ("all", "0s", "0s"),
            ("opacity", "1s", "-1s"),
            ("opacity", "1s", "-2s"),
            ("unmatched-property", "1s", "0s"),
            ("opacity, opacity", "1s, 0s", "0s"),
        ] {
            let mut actual = TransitionTimeline::default();
            let mut expected = TransitionTimeline::default();
            for (time, opacity) in [(0.0, Some(0.0)), (10.0, None), (20.0, Some(0.5))] {
                let base = timeline_style(opacity, property, duration, delay);
                assert!(
                    compare_timeline_step(&mut actual, &mut expected, time, &base)
                        .1
                        .is_empty()
                );
            }
            let enabled = timeline_style(Some(1.0), "opacity", "1s", "0s");
            let (values, events) =
                compare_timeline_step(&mut actual, &mut expected, 30.0, &enabled);
            assert_eq!(values.get("opacity"), Some(&ComputedValue::Number(0.5)));
            assert_eq!(events.len(), 2);
        }
    }

    #[test]
    fn unchanged_base_preserves_delayed_zero_duration_start_and_end_order() {
        let mut actual = TransitionTimeline::default();
        let mut expected = TransitionTimeline::default();
        let initial = timeline_style(Some(0.0), "opacity", "0s", "100ms");
        compare_timeline_step(&mut actual, &mut expected, 0.0, &initial);
        let target = timeline_style(Some(1.0), "opacity", "0s", "100ms");
        let (_, events) = compare_timeline_step(&mut actual, &mut expected, 10.0, &target);
        assert_eq!(
            events
                .iter()
                .map(|event| event.event_type)
                .collect::<Vec<_>>(),
            ["transitionrun"]
        );
        assert!(
            compare_timeline_step(&mut actual, &mut expected, 109.0, &target)
                .1
                .is_empty()
        );
        let (_, events) = compare_timeline_step(&mut actual, &mut expected, 110.0, &target);
        assert_eq!(
            events
                .iter()
                .map(|event| event.event_type)
                .collect::<Vec<_>>(),
            ["transitionstart", "transitionend"]
        );
        assert!(events.iter().all(|event| event.elapsed_time == 0.0));
    }

    #[test]
    fn changed_properties_keep_alphabetic_start_order_and_staggered_completion() {
        let mut actual = TransitionTimeline::default();
        let mut expected = TransitionTimeline::default();
        let initial = timeline_style(Some(0.0), "width, opacity", "100ms, 200ms", "0s");
        compare_timeline_step(&mut actual, &mut expected, 0.0, &initial);
        let mut target = initial.clone();
        target.insert("opacity", ComputedValue::Number(1.0));
        target.insert("width", ComputedValue::Px(32.0));
        let (_, events) = compare_timeline_step(&mut actual, &mut expected, 10.0, &target);
        assert_eq!(
            events
                .iter()
                .map(|event| (event.property_name.as_str(), event.event_type))
                .collect::<Vec<_>>(),
            [
                ("opacity", "transitionrun"),
                ("opacity", "transitionstart"),
                ("width", "transitionrun"),
                ("width", "transitionstart")
            ],
        );
        let (_, events) = compare_timeline_step(&mut actual, &mut expected, 110.0, &target);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].property_name, "width");
        assert_eq!(events[0].event_type, "transitionend");
        let (_, events) = compare_timeline_step(&mut actual, &mut expected, 210.0, &target);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].property_name, "opacity");
        assert_eq!(events[0].event_type, "transitionend");
    }

    #[test]
    fn negative_delay_reversal_and_configuration_cancellation_match_unoptimized_timeline() {
        let mut actual = TransitionTimeline::default();
        let mut expected = TransitionTimeline::default();
        let initial = timeline_style(Some(0.0), "opacity", "1s", "-250ms");
        compare_timeline_step(&mut actual, &mut expected, 0.0, &initial);
        let target = timeline_style(Some(1.0), "opacity", "1s", "-250ms");
        let (values, events) = compare_timeline_step(&mut actual, &mut expected, 100.0, &target);
        assert_eq!(values.get("opacity"), Some(&ComputedValue::Number(0.25)));
        assert!(events.iter().all(|event| event.elapsed_time == 0.25));
        let (values, events) = compare_timeline_step(&mut actual, &mut expected, 350.0, &target);
        assert_eq!(values.get("opacity"), Some(&ComputedValue::Number(0.5)));
        assert!(events.is_empty());
        let (_, events) = compare_timeline_step(&mut actual, &mut expected, 350.0, &initial);
        assert_eq!(
            events
                .iter()
                .map(|event| event.event_type)
                .collect::<Vec<_>>(),
            ["transitioncancel", "transitionrun", "transitionstart"]
        );
        assert_eq!(
            actual.elements[&1].running["opacity"].reversing_shortening_factor,
            0.5
        );
        let cancelled = timeline_style(Some(0.0), "none", "1s", "-250ms");
        let (_, events) = compare_timeline_step(&mut actual, &mut expected, 400.0, &cancelled);
        assert_eq!(
            events
                .iter()
                .map(|event| event.event_type)
                .collect::<Vec<_>>(),
            ["transitioncancel"]
        );
        assert!(!actual.has_running_transitions());
        assert!(
            compare_timeline_step(&mut actual, &mut expected, 2000.0, &cancelled)
                .1
                .is_empty()
        );
    }

    #[test]
    fn running_property_diagnostics_are_bounded_and_exhaustive() {
        let running = RunningTransition::new(
            ComputedValue::Number(0.0),
            ComputedValue::Number(1.0),
            None,
            TransitionParameters {
                duration_ms: 1_000.0,
                delay_ms: 0.0,
                timing: TimingFunction::Linear,
                allow_discrete: false,
            },
            false,
            0.0,
        );
        let mut timeline = TransitionTimeline::default();
        for (node_id, properties) in [
            (
                10,
                ["bottom", "transform", "color", "background-color"].as_slice(),
            ),
            (
                20,
                [
                    "filter",
                    "backdrop-filter",
                    "opacity",
                    "text-shadow",
                    "--tone",
                ]
                .as_slice(),
            ),
        ] {
            timeline.elements.insert(
                node_id,
                ElementTransitionState {
                    running: properties
                        .iter()
                        .map(|property| ((*property).to_string(), running.clone()))
                        .collect(),
                    ..ElementTransitionState::default()
                },
            );
        }
        timeline
            .elements
            .insert(30, ElementTransitionState::default());

        for id in [10, 20] {
            timeline.update_running_node(id, true);
        }
        assert_eq!(
            timeline
                .running_node_ids()
                .into_iter()
                .collect::<BTreeSet<_>>(),
            BTreeSet::from([10, 20])
        );
        let counts = timeline.running_property_counts();
        assert_eq!(
            counts,
            TransitionPropertyCounts {
                nodes: 2,
                properties: 9,
                geometry: 1,
                transform: 1,
                color: 1,
                background: 1,
                filter: 2,
                opacity: 1,
                other: 2,
            }
        );
        assert_eq!(counts.categorized_properties(), counts.properties);
    }

    #[test]
    fn transition_cache_scope_is_an_exact_audited_property_allowlist() {
        use crate::css::style::PropertyId;

        let localized = PropertyId::ALL
            .iter()
            .filter_map(|property| {
                (transition_cache_scope(property.as_str()) == TransitionCacheScope::Subtree)
                    .then_some(property.as_str())
            })
            .collect::<Vec<_>>();
        assert_eq!(
            localized,
            [
                "-webkit-text-fill-color",
                "-webkit-text-stroke-color",
                "accent-color",
                "backdrop-filter",
                "background-attachment",
                "background-blend-mode",
                "background-clip",
                "background-color",
                "background-image",
                "background-origin",
                "background-position-x",
                "background-position-y",
                "background-repeat",
                "background-size",
                "border-block-end-color",
                "border-block-start-color",
                "border-bottom-color",
                "border-inline-end-color",
                "border-inline-start-color",
                "border-left-color",
                "border-right-color",
                "border-top-color",
                "caret-color",
                "color",
                "column-rule-color",
                "filter",
                "opacity",
                "outline-color",
                "scrollbar-color",
                "text-decoration-color",
                "text-emphasis-color",
                "transform",
            ],
            "adding another localized property requires a dependency audit and this contract update",
        );

        for property in PropertyId::ALL {
            if matches!(
                transition_property_category(property.as_str()),
                TransitionPropertyCategory::Color | TransitionPropertyCategory::Background
            ) {
                assert_eq!(
                    transition_cache_scope(property.as_str()),
                    TransitionCacheScope::Subtree,
                    "every implemented color/background diagnostic property needs an audited cache scope: {}",
                    property.as_str(),
                );
            }
        }

        for property in [
            "background",
            "border-color",
            "border-block-color",
            "border-inline-color",
            "width",
            "text-shadow",
            "box-shadow",
            "--accent-color",
            "future-color",
            "background-future",
        ] {
            assert_eq!(
                transition_cache_scope(property),
                TransitionCacheScope::Full,
                "unknown, custom, shorthand, and unaudited properties must fail closed: {property}",
            );
        }
    }

    fn run_static_timeline_benchmark(
        sample: fn(&mut TransitionTimeline, usize, &mut PropertyMap),
        template: &PropertyMap,
        changing: bool,
    ) -> (std::time::Duration, TransitionTimeline, Vec<PropertyMap>) {
        let mut timeline = TransitionTimeline::default();
        let mut last = vec![PropertyMap::new(); 32];
        for (node, values) in last.iter_mut().enumerate() {
            *values = template.clone();
            sample(&mut timeline, node, values);
        }
        let started = std::time::Instant::now();
        for frame in 1..=16 {
            timeline.set_time_ms(frame as f64 * 16.0);
            for (node, values) in last.iter_mut().enumerate() {
                *values = template.clone();
                if changing {
                    values.insert("width", ComputedValue::Px(16.0 + frame as f32));
                }
                sample(&mut timeline, node, std::hint::black_box(&mut *values));
                std::hint::black_box(&*values);
            }
        }
        (started.elapsed(), timeline, last)
    }

    #[test]
    #[ignore = "manual comparison of static CSS transition sampling fast paths"]
    fn static_transition_sampling_microbenchmark() {
        for (name, property, duration, changing) in [
            ("static-default", "all", "0s", false),
            ("static-enabled", "opacity", "1s", false),
            ("changing-zero-time", "all", "0s", true),
            ("changing-unmatched", "unmatched-property", "1s", true),
        ] {
            let mut style = timeline_style(Some(0.5), property, duration, "0s");
            for index in 0..64 {
                style.insert(
                    format!("--benchmark-{index:02}"),
                    ComputedValue::Number(index as f32),
                );
            }
            let (reference_time, mut reference, reference_values) = run_static_timeline_benchmark(
                TransitionTimeline::sample_without_fast_paths,
                &style,
                changing,
            );
            let (optimized_time, mut optimized, optimized_values) =
                run_static_timeline_benchmark(TransitionTimeline::sample, &style, changing);
            assert_eq!(optimized_values, reference_values);
            assert_eq!(optimized.take_events(), reference.take_events());
            assert_timeline_state_matches(&optimized, &reference);
            println!(
                "TRANSITION_SAMPLE_BENCH case={name} nodes=32 frames=16 reference_ms={:.3} optimized_ms={:.3} reference_over_optimized={:.3}",
                reference_time.as_secs_f64() * 1000.0,
                optimized_time.as_secs_f64() * 1000.0,
                reference_time.as_secs_f64() / optimized_time.as_secs_f64(),
            );
        }
    }

    #[test]
    fn expands_shorthand_lists_and_defaults() {
        let declarations = expand_transition_shorthand(
            Value::Keyword(
                "opacity 200ms linear 50ms, transform 1s cubic-bezier(.1, .2, .3, 1)".into(),
            ),
            false,
        );
        assert_eq!(declarations.len(), 5);
        assert_eq!(
            declarations[0].value,
            Value::Keyword("opacity, transform".into())
        );
        assert_eq!(declarations[1].value, Value::Keyword("200ms, 1s".into()));
        assert_eq!(
            declarations[2].value,
            Value::Keyword("linear, cubic-bezier(0.1, 0.2, 0.3, 1)".into())
        );
        assert_eq!(declarations[3].value, Value::Keyword("50ms, 0s".into()));
        assert_eq!(
            declarations[4].value,
            Value::Keyword("normal, normal".into())
        );
    }

    #[test]
    fn rejects_negative_duration_and_invalid_timing_functions() {
        assert!(parse_transition_shorthand("opacity -1s").is_none());
        assert!(parse_transition_shorthand("none 1s").is_none());
        assert!(parse_transition_shorthand("opacity 1s cubic-bezier(2, 0, 0, 1)").is_none());
        assert!(normalize_transition_longhand("transition-duration", "1s, -1ms").is_none());
        assert!(normalize_transition_longhand("transition-timing-function", "steps(0)").is_none());
    }

    #[test]
    fn accepts_negative_delays_and_css_wide_keywords() {
        let descriptors = parse_transition_shorthand("opacity 2s ease -500ms").unwrap();
        assert_eq!(descriptors[0].duration, "2s");
        assert_eq!(descriptors[0].delay, "-500ms");
        assert_eq!(
            normalize_transition_longhand("transition-property", "initial"),
            Some("initial".into())
        );
    }

    #[test]
    fn parses_transition_behavior_in_longhand_and_shorthand() {
        assert_eq!(
            normalize_transition_longhand("transition-behavior", "normal, ALLOW-DISCRETE"),
            Some("normal, allow-discrete".into())
        );
        let descriptors = parse_transition_shorthand("visibility 1s allow-discrete").unwrap();
        assert_eq!(descriptors[0].behavior, "allow-discrete");
        assert!(normalize_transition_longhand("transition-behavior", "auto").is_none());
    }

    #[test]
    fn allow_discrete_switches_non_interpolable_values_at_half_progress() {
        let mut timeline = TransitionTimeline::default();
        let mut initial = timeline_style(None, "visibility", "1s", "0s");
        initial.insert("visibility", ComputedValue::Keyword("visible".into()));
        initial.insert(
            "transition-behavior",
            ComputedValue::Keyword("allow-discrete".into()),
        );
        timeline.sample(1, &mut initial);

        let mut target_style = initial.clone();
        target_style.insert("visibility", ComputedValue::Keyword("hidden".into()));
        timeline.set_time_ms(10.0);
        let mut target = target_style.clone();
        timeline.sample(1, &mut target);
        assert_eq!(
            target.get("visibility"),
            Some(&ComputedValue::Keyword("visible".into()))
        );
        assert_eq!(timeline.take_events().len(), 2);

        timeline.set_time_ms(509.0);
        let mut target = target_style.clone();
        timeline.sample(1, &mut target);
        assert_eq!(
            target.get("visibility"),
            Some(&ComputedValue::Keyword("visible".into()))
        );
        timeline.set_time_ms(510.0);
        let mut target = target_style;
        timeline.sample(1, &mut target);
        assert_eq!(
            target.get("visibility"),
            Some(&ComputedValue::Keyword("hidden".into()))
        );
    }

    #[test]
    fn samples_cubic_bezier_and_step_timing_functions() {
        let ease_middle = TimingFunction::parse("ease").unwrap().sample(0.5);
        assert!((ease_middle - 0.8024).abs() < 0.001);
        assert_eq!(
            TimingFunction::parse("steps(4)").unwrap().sample(0.49),
            0.25
        );
        assert_eq!(
            TimingFunction::parse("step-start").unwrap().sample(0.0),
            1.0
        );
    }

    #[test]
    fn normalizes_and_samples_linear_easing_stops() {
        assert_eq!(
            normalize_timing_function("linear(0, .5 25% 75%, 1)"),
            Some("linear(0 0%, 0.5 25%, 0.5 75%, 1 100%)".into())
        );
        let timing = TimingFunction::parse("linear(0, .5 25% 75%, 1)").unwrap();
        assert!((timing.clone().sample(0.125) - 0.25).abs() < 0.0001);
        assert!((timing.clone().sample(0.5) - 0.5).abs() < 0.0001);
        assert!((timing.sample(0.875) - 0.75).abs() < 0.0001);
        assert!(normalize_timing_function("linear(0)").is_none());
        assert!(normalize_timing_function("linear(0 80% 20%, 1)").is_none());
    }

    #[test]
    fn interpolates_compatible_filter_values() {
        assert_eq!(
            interpolate_property(
                "filter",
                &ComputedValue::Keyword("brightness(1) blur(0px)".into()),
                &ComputedValue::Keyword("brightness(2) blur(10px)".into()),
                0.5,
            ),
            Some(ComputedValue::Keyword("brightness(1.5) blur(5px)".into()))
        );
    }
}

/// Evaluates the shared CSS easing curve from explicit inputs.
pub(crate) fn animation_timing_progress(input: &str, progress: f32) -> f32 {
    TimingFunction::parse(input)
        .map_or(progress, |timing| timing.sample(f64::from(progress)) as f32)
}
