//! CSS Transitions declaration parsing.
//!
//! Transition values use comma-separated lists whose commas must survive the
//! generic declaration parser. This module validates those lists and expands
//! the shorthand into its four longhands. Timeline sampling lives above the
//! CSS parser and consumes the normalized longhand values produced here.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use crate::paint::color::{Color, parse_color};

use super::{ComputedValue, Declaration, Value};

#[derive(Debug, Clone, PartialEq)]
struct TransitionDescriptor {
    property: String,
    duration: String,
    timing_function: String,
    delay: String,
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
    events: Vec<TransitionEventRecord>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct TransitionEventRecord {
    pub node_id: usize,
    pub event_type: &'static str,
    pub property_name: String,
    pub elapsed_time: f64,
}

#[derive(Debug, Default)]
struct ElementTransitionState {
    base_values: BTreeMap<String, ComputedValue>,
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
    started: bool,
}

#[derive(Debug, Clone, Copy)]
enum TimingFunction {
    Linear,
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

#[derive(Debug, Clone, Copy)]
struct TransitionParameters {
    duration_ms: f64,
    delay_ms: f64,
    timing: TimingFunction,
}

impl TransitionTimeline {
    pub(crate) fn set_time_ms(&mut self, time_ms: f64) -> bool {
        if time_ms.is_finite() {
            let next = self.now_ms.max(time_ms.max(0.0));
            if next > self.now_ms {
                self.now_ms = next;
                return self
                    .elements
                    .values()
                    .any(|state| !state.running.is_empty());
            }
        }
        false
    }

    pub(crate) fn sample(
        &mut self,
        node_id: usize,
        properties: &mut BTreeMap<String, ComputedValue>,
    ) {
        let now_ms = self.now_ms;
        let state = self.elements.entry(node_id).or_default();
        if !state.initialized {
            state.base_values = properties.clone();
            state.initialized = true;
            return;
        }

        let configuration = TransitionConfiguration::from_properties(properties);
        let no_longer_matching: Vec<String> = state
            .running
            .keys()
            .filter(|property| configuration.matching_parameters(property).is_none())
            .cloned()
            .collect();
        for property in no_longer_matching {
            if let Some(running) = state.running.remove(&property) {
                self.events.push(running.event_record(
                    node_id,
                    "transitioncancel",
                    property,
                    now_ms,
                ));
            }
        }

        let mut changed_properties = BTreeSet::new();
        changed_properties.extend(state.base_values.keys().cloned());
        changed_properties.extend(properties.keys().cloned());
        changed_properties.retain(|property| {
            effective_value(property, state.base_values.get(property))
                != effective_value(property, properties.get(property))
        });

        for property in changed_properties {
            let old_value = effective_value(&property, state.base_values.get(&property));
            let new_value = effective_value(&property, properties.get(&property));
            let Some(end_value) = new_value else {
                state.running.remove(&property);
                continue;
            };
            if state
                .running
                .get(&property)
                .is_some_and(|running| running.end_value == end_value)
            {
                continue;
            }
            let interrupted = state.running.get(&property).cloned();
            let start_value = interrupted
                .as_ref()
                .and_then(|running| running.sample(&property, now_ms))
                .or(old_value);
            let Some(start_value) = start_value else {
                state.running.remove(&property);
                continue;
            };
            if start_value == end_value {
                if let Some(running) = state.running.remove(&property) {
                    self.events.push(running.event_record(
                        node_id,
                        "transitioncancel",
                        property.clone(),
                        now_ms,
                    ));
                }
                continue;
            }
            let Some(parameters) = configuration.matching_parameters(&property) else {
                if let Some(running) = state.running.remove(&property) {
                    self.events.push(running.event_record(
                        node_id,
                        "transitioncancel",
                        property.clone(),
                        now_ms,
                    ));
                }
                continue;
            };
            if parameters.duration_ms + parameters.delay_ms <= 0.0
                || interpolate_property(&property, &start_value, &end_value, 0.5).is_none()
            {
                if let Some(running) = state.running.remove(&property) {
                    self.events.push(running.event_record(
                        node_id,
                        "transitioncancel",
                        property.clone(),
                        now_ms,
                    ));
                }
                continue;
            }
            if let Some(running) = interrupted.as_ref() {
                self.events.push(running.event_record(
                    node_id,
                    "transitioncancel",
                    property.clone(),
                    now_ms,
                ));
            }
            let (reversing_adjusted_start_value, reversing_shortening_factor) = interrupted
                .as_ref()
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
            let running = RunningTransition {
                start_value,
                end_value,
                reversing_adjusted_start_value,
                reversing_shortening_factor,
                start_ms,
                end_ms,
                timing: parameters.timing,
                started,
            };
            self.events.push(running.event_record(
                node_id,
                "transitionrun",
                property.clone(),
                now_ms,
            ));
            if started {
                self.events.push(running.event_record(
                    node_id,
                    "transitionstart",
                    property.clone(),
                    now_ms,
                ));
            }
            state.running.insert(property, running);
        }
        state.base_values = properties.clone();

        let mut completed = Vec::new();
        for (property, running) in &mut state.running {
            if !running.started && now_ms >= running.start_ms {
                running.started = true;
                self.events.push(running.event_record(
                    node_id,
                    "transitionstart",
                    property.clone(),
                    now_ms,
                ));
            }
            if now_ms >= running.end_ms {
                properties.insert(property.clone(), running.end_value.clone());
                self.events.push(running.event_record(
                    node_id,
                    "transitionend",
                    property.clone(),
                    now_ms,
                ));
                completed.push(property.clone());
            } else if let Some(value) = running.sample(property, now_ms) {
                properties.insert(property.clone(), value);
            }
        }
        for property in completed {
            state.running.remove(&property);
        }
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
                self.events.extend(state.running.into_iter().map(|(property, running)| {
                    running.event_record(node_id, "transitioncancel", property, self.now_ms)
                }));
            }
        }
    }

    pub(crate) fn running_node_ids(&self) -> Vec<usize> {
        self.elements
            .iter()
            .filter_map(|(node_id, state)| (!state.running.is_empty()).then_some(*node_id))
            .collect()
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
                self.events.extend(state.running.into_iter().map(|(property, running)| {
                    running.event_record(node_id, "transitioncancel", property, self.now_ms)
                }));
            }
        }
    }
}

impl RunningTransition {
    fn input_progress(&self, now_ms: f64) -> f64 {
        if now_ms >= self.end_ms || self.end_ms < self.start_ms {
            1.0
        } else if now_ms <= self.start_ms {
            0.0
        } else {
            (now_ms - self.start_ms) / (self.end_ms - self.start_ms)
        }
    }

    fn sample(&self, property: &str, now_ms: f64) -> Option<ComputedValue> {
        let progress = self.input_progress(now_ms);
        interpolate_property(
            property,
            &self.start_value,
            &self.end_value,
            self.timing.sample(progress) as f32,
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
            _ => None,
        }
    }

    fn sample(self, progress: f64) -> f64 {
        let progress = progress.clamp(0.0, 1.0);
        match self {
            Self::Linear => progress,
            Self::CubicBezier(x1, y1, x2, y2) => sample_cubic_bezier(progress, x1, y1, x2, y2),
            Self::Steps(count, position) => sample_steps(progress, count, position),
        }
    }
}

struct TransitionConfiguration {
    properties: Vec<String>,
    durations_ms: Vec<f64>,
    timings: Vec<TimingFunction>,
    delays_ms: Vec<f64>,
}

impl TransitionConfiguration {
    fn from_properties(properties: &BTreeMap<String, ComputedValue>) -> Self {
        let property_text = property_keyword(properties, "transition-property", "all");
        let duration_text = property_keyword(properties, "transition-duration", "0s");
        let timing_text = property_keyword(properties, "transition-timing-function", "ease");
        let delay_text = property_keyword(properties, "transition-delay", "0s");
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
        }
    }

    fn matching_parameters(&self, property: &str) -> Option<TransitionParameters> {
        let index = self
            .properties
            .iter()
            .enumerate()
            .rev()
            .find_map(|(index, candidate)| {
                (candidate == "all" || candidate == property).then_some(index)
            })?;
        Some(TransitionParameters {
            duration_ms: *repeated(&self.durations_ms, index)?,
            delay_ms: *repeated(&self.delays_ms, index)?,
            timing: *repeated(&self.timings, index)?,
        })
    }
}

fn property_keyword<'a>(
    properties: &'a BTreeMap<String, ComputedValue>,
    name: &str,
    fallback: &'a str,
) -> &'a str {
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
            if is_transitionable_number_property(property) =>
        {
            Some(ComputedValue::Number(mix(*start, *end)))
        }
        (ComputedValue::Px(start), ComputedValue::Px(end)) => {
            Some(ComputedValue::Px(mix(*start, *end)))
        }
        (ComputedValue::Percentage(start), ComputedValue::Percentage(end)) => {
            Some(ComputedValue::Percentage(mix(*start, *end)))
        }
        (
            ComputedValue::CalcPxPercent(start_px, start_percent),
            ComputedValue::CalcPxPercent(end_px, end_percent),
        ) => Some(ComputedValue::CalcPxPercent(
            mix(*start_px, *end_px),
            mix(*start_percent, *end_percent),
        )),
        _ if length_components(start).is_some() && length_components(end).is_some() => {
            let (start_px, start_percent) = length_components(start)?;
            let (end_px, end_percent) = length_components(end)?;
            Some(ComputedValue::CalcPxPercent(
                mix(start_px, end_px),
                mix(start_percent, end_percent),
            ))
        }
        (ComputedValue::Color(start), ComputedValue::Color(end))
            if property == "color" || property.ends_with("-color") =>
        {
            let start = parse_color(start)?;
            let end = parse_color(end)?;
            Some(ComputedValue::Color(interpolate_color(
                start, end, progress,
            )))
        }
        (ComputedValue::Keyword(start), ComputedValue::Keyword(end)) if property == "transform" => {
            Some(ComputedValue::Keyword(super::interpolate_transform_lists(
                start, end, progress,
            )?))
        }
        _ => None,
    }
}

fn length_components(value: &ComputedValue) -> Option<(f32, f32)> {
    match value {
        ComputedValue::Px(px) => Some((*px, 0.0)),
        ComputedValue::Percentage(percent) => Some((0.0, *percent)),
        ComputedValue::CalcPxPercent(px, percent) => Some((*px, *percent)),
        _ => None,
    }
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
    ]
}

pub(crate) fn normalize_transition_shorthand(input: &str) -> Option<String> {
    let lower = input.trim().to_ascii_lowercase();
    if is_css_wide_keyword(&lower) {
        return Some(lower);
    }
    let descriptors = parse_transition_shorthand(input)?;
    let mut properties = BTreeMap::new();
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
        });
    }
    if descriptors.len() > 1 && descriptors.iter().any(|item| item.property == "none") {
        return None;
    }
    Some(descriptors)
}

pub(crate) fn computed_transition_shorthand(
    properties: &BTreeMap<String, ComputedValue>,
) -> String {
    let property_items = split_keyword_property(properties, "transition-property", "all");
    if property_items.as_slice() == ["none"] {
        return "none".to_string();
    }
    let durations = split_keyword_property(properties, "transition-duration", "0s");
    let timings = split_keyword_property(properties, "transition-timing-function", "ease");
    let delays = split_keyword_property(properties, "transition-delay", "0s");
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
            if components.is_empty() {
                "all".to_string()
            } else {
                components.join(" ")
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn split_keyword_property(
    properties: &BTreeMap<String, ComputedValue>,
    name: &str,
    fallback: &str,
) -> Vec<String> {
    split_top_level(property_keyword(properties, name, fallback), ',')
        .unwrap_or_default()
        .into_iter()
        .map(|value| value.trim().to_string())
        .collect()
}

fn transition_longhands() -> [&'static str; 4] {
    [
        "transition-property",
        "transition-duration",
        "transition-timing-function",
        "transition-delay",
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

fn split_top_level_whitespace(input: &str) -> Option<Vec<&str>> {
    let mut values = Vec::new();
    let mut depth = 0usize;
    let mut start = None;
    for (index, ch) in input.char_indices() {
        match ch {
            '(' => {
                depth = depth.checked_add(1)?;
                start.get_or_insert(index);
            }
            ')' => {
                depth = depth.checked_sub(1)?;
                start.get_or_insert(index);
            }
            _ if ch.is_whitespace() && depth == 0 => {
                if let Some(component_start) = start.take() {
                    values.push(input[component_start..index].trim());
                }
            }
            _ => {
                start.get_or_insert(index);
            }
        }
    }
    if depth != 0 {
        return None;
    }
    if let Some(component_start) = start {
        values.push(input[component_start..].trim());
    }
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

fn is_css_wide_keyword(input: &str) -> bool {
    matches!(
        input,
        "inherit" | "initial" | "unset" | "revert" | "revert-layer"
    )
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

    #[test]
    fn expands_shorthand_lists_and_defaults() {
        let declarations = expand_transition_shorthand(
            Value::Keyword(
                "opacity 200ms linear 50ms, transform 1s cubic-bezier(.1, .2, .3, 1)".into(),
            ),
            false,
        );
        assert_eq!(declarations.len(), 4);
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
}
