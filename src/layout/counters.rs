use std::collections::{HashMap, HashSet};

use crate::css::style::counter_pairs;
use crate::css::{
    ComputedStyle, ComputedValue, PseudoElement, StyleResolver, Value, parse_style_attribute,
};
use crate::dom::{Node, NodeHandle, NodeType};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct Target {
    node: usize,
    pseudo: Option<PseudoElement>,
    parent: Option<usize>,
}

#[derive(Debug, Clone)]
struct Counter {
    name: String,
    creator: Target,
    value: i32,
}

type CounterSet = Vec<Counter>;
type Snapshot = HashMap<String, Vec<i32>>;

pub(super) fn prepare_counter_values(node: &NodeHandle, resolver: &mut StyleResolver) {
    // Ordinary list markers use list_item_ordinal and do not need these snapshots.
    // A previous pass may have populated snapshots before a stylesheet was removed.
    let started = std::env::var_os("OMOIKANE_TRACE_PAINT").map(|_| std::time::Instant::now());
    let required = resolver.may_read_generated_counters() || contains_q_element(node);
    let guard_ms = started.map_or(0.0, |start| start.elapsed().as_secs_f64() * 1000.0);
    if !required {
        resolver.replace_counter_values(HashMap::new());
        resolver.replace_quote_values(HashMap::new());
        trace_counter_prepass(started, required, guard_ms);
        return;
    }
    let mut root = node.clone();
    while let Some(parent) = root.parent_node() {
        root = parent;
    }

    let mut processor = Processor {
        resolver,
        snapshots: HashMap::new(),
        quote_snapshots: HashMap::new(),
        quote_depth: 0,
    };
    let empty = CounterSet::new();
    let mut previous = empty.clone();
    if root.node_type() == NodeType::Element {
        processor.process_element(&root, &empty, &previous, false);
    } else {
        let mut previous_sibling = None;
        for child in element_children(&root) {
            let source = previous_sibling.as_ref().unwrap_or(&empty);
            let (own, final_set) = processor.process_element(&child, source, &previous, false);
            previous_sibling = Some(own);
            previous = final_set;
        }
    }
    processor
        .resolver
        .replace_counter_values(processor.snapshots);
    processor
        .resolver
        .replace_quote_values(processor.quote_snapshots);
    trace_counter_prepass(started, required, guard_ms);
}

fn trace_counter_prepass(started: Option<std::time::Instant>, required: bool, guard_ms: f64) {
    if let Some(started) = started {
        eprintln!(
            "{}",
            crate::diagnostics::json_record(
                "OMOIKANE_COUNTER_PREPASS",
                &serde_json::json!({
                    "required": required,
                    "guard_ms": guard_ms,
                    "elapsed_ms": started.elapsed().as_secs_f64() * 1000.0,
                })
            )
        );
    }
}

struct Processor<'a> {
    resolver: &'a mut StyleResolver,
    snapshots: HashMap<(usize, Option<PseudoElement>), Snapshot>,
    quote_snapshots: HashMap<(usize, Option<PseudoElement>), Vec<String>>,
    quote_depth: usize,
}

impl Processor<'_> {
    fn process_element(
        &mut self,
        node: &NodeHandle,
        counter_source: &CounterSet,
        value_source: &CounterSet,
        ancestor_hidden: bool,
    ) -> (CounterSet, CounterSet) {
        let target = Target {
            node: node.identity(),
            pseudo: None,
            parent: node.parent_node().map(|parent| parent.identity()),
        };
        let mut own = inherit_counters(counter_source, value_source, target);
        let style = self.resolver.computed_style_shared(node);
        let hidden = ancestor_hidden || style.is_display_none();
        if !hidden {
            apply_counter_properties(&mut own, target, &style);
        }
        if hidden {
            return (own.clone(), own);
        }

        let mut preceding_own: Option<CounterSet> = None;
        let mut preceding_value = own.clone();
        if let Some(before) =
            self.process_pseudo(node, PseudoElement::Before, &own, &preceding_value)
        {
            preceding_value = before.clone();
            preceding_own = Some(before);
        }

        for child in element_children(node) {
            let source = preceding_own.as_ref().unwrap_or(&own);
            let (child_own, child_final) =
                self.process_element(&child, source, &preceding_value, false);
            preceding_own = Some(child_own);
            preceding_value = child_final;
        }

        let source = preceding_own.as_ref().unwrap_or(&own);
        if let Some(after) =
            self.process_pseudo(node, PseudoElement::After, source, &preceding_value)
        {
            preceding_value = after;
        }
        (own, preceding_value)
    }

    fn process_pseudo(
        &mut self,
        node: &NodeHandle,
        pseudo: PseudoElement,
        counter_source: &CounterSet,
        value_source: &CounterSet,
    ) -> Option<CounterSet> {
        let style = self.resolver.computed_pseudo_style_shared(node, pseudo)?;
        if style.is_display_none() || !generates_pseudo_box(&style) {
            return None;
        }
        let target = Target {
            node: node.identity(),
            pseudo: Some(pseudo),
            parent: Some(node.identity()),
        };
        let mut counters = inherit_counters(counter_source, value_source, target);
        apply_counter_properties(&mut counters, target, &style);
        let content = style.component_value("content").cloned().or_else(|| {
            style.get("content").and_then(|value| match value {
                ComputedValue::Keyword(keyword) => Some(Value::Keyword(keyword.clone())),
                ComputedValue::String(value) => Some(Value::String(value.clone())),
                _ => None,
            })
        });
        if let Some(content) = content.as_ref() {
            let names = referenced_counter_names(content);
            for name in &names {
                ensure_counter(&mut counters, target, &name);
            }
            self.store(target, &counters, &names);
            let quotes = quote_pairs(&style, node);
            let replacements = resolve_quote_tokens(content, &quotes, &mut self.quote_depth);
            if !replacements.is_empty() || content_reads_quotes(content) {
                self.quote_snapshots
                    .insert((target.node, target.pseudo), replacements);
            }
        }
        Some(counters)
    }

    fn store(&mut self, target: Target, counters: &CounterSet, names: &[String]) {
        if names.is_empty() {
            return;
        }
        let names = names.iter().map(String::as_str).collect::<HashSet<_>>();
        let mut snapshot = Snapshot::new();
        for counter in counters
            .iter()
            .filter(|counter| names.contains(counter.name.as_str()))
        {
            snapshot
                .entry(counter.name.clone())
                .or_default()
                .push(counter.value);
        }
        self.snapshots
            .insert((target.node, target.pseudo), snapshot);
    }
}

fn content_reads_quotes(value: &Value) -> bool {
    match value {
        Value::Keyword(keyword) => matches!(
            keyword.to_ascii_lowercase().as_str(),
            "open-quote" | "close-quote" | "no-open-quote" | "no-close-quote"
        ),
        Value::List(values) | Value::CommaList(values) => values.iter().any(content_reads_quotes),
        _ => false,
    }
}

fn resolve_quote_tokens(
    value: &Value,
    pairs: &[(String, String)],
    depth: &mut usize,
) -> Vec<String> {
    let values = match value {
        Value::List(values) => values.as_slice(),
        value => std::slice::from_ref(value),
    };
    let mut replacements = Vec::new();
    for value in values {
        let Value::Keyword(keyword) = value else {
            continue;
        };
        match keyword.to_ascii_lowercase().as_str() {
            "open-quote" => {
                replacements.push(quote_at_depth(pairs, *depth, true));
                *depth = depth.saturating_add(1);
            }
            "close-quote" => {
                if *depth > 0 {
                    *depth -= 1;
                    replacements.push(quote_at_depth(pairs, *depth, false));
                } else {
                    replacements.push(String::new());
                }
            }
            "no-open-quote" => *depth = depth.saturating_add(1),
            "no-close-quote" => *depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    replacements
}

fn quote_at_depth(pairs: &[(String, String)], depth: usize, opening: bool) -> String {
    let Some((open, close)) = pairs.get(depth).or_else(|| pairs.last()) else {
        return String::new();
    };
    if opening { open.clone() } else { close.clone() }
}

fn quote_pairs(style: &ComputedStyle, node: &NodeHandle) -> Vec<(String, String)> {
    match style.component_value("quotes") {
        Some(Value::List(values)) => values
            .chunks_exact(2)
            .filter_map(|pair| match pair {
                [Value::String(open), Value::String(close)] => Some((open.clone(), close.clone())),
                _ => None,
            })
            .collect(),
        Some(Value::Keyword(keyword)) if keyword.eq_ignore_ascii_case("none") => Vec::new(),
        _ => auto_quote_pairs(node),
    }
}

fn auto_quote_pairs(node: &NodeHandle) -> Vec<(String, String)> {
    let mut current = Some(node.clone());
    let mut language = None;
    while let Some(element) = current {
        if let Some(value) = element.get_attribute("lang")
            && !value.is_empty()
        {
            language = Some(value.to_ascii_lowercase());
            break;
        }
        current = element.parent_node();
    }
    let pairs = match language.as_deref().and_then(|lang| lang.split('-').next()) {
        Some("fr") => [("«\u{a0}", "\u{a0}»"), ("“", "”")],
        Some("de") => [("„", "“"), ("‚", "‘")],
        Some("ja") | Some("zh") => [("「", "」"), ("『", "』")],
        _ => [("“", "”"), ("‘", "’")],
    };
    pairs
        .into_iter()
        .map(|(open, close)| (open.to_string(), close.to_string()))
        .collect()
}

fn element_children(node: &NodeHandle) -> Vec<NodeHandle> {
    node.layout_child_nodes()
        .into_iter()
        .filter(|child| child.node_type() == NodeType::Element)
        .collect()
}

fn contains_q_element(node: &NodeHandle) -> bool {
    node.has_tag_name("q") || element_children(node).iter().any(contains_q_element)
}

fn inherit_counters(
    counter_source: &CounterSet,
    value_source: &CounterSet,
    target: Target,
) -> CounterSet {
    let outer_names = counter_source
        .iter()
        .filter(|counter| counter.creator.parent != target.parent)
        .map(|counter| counter.name.as_str())
        .collect::<HashSet<_>>();
    let inherited_source = counter_source
        .iter()
        .filter(|counter| {
            counter.creator.parent != target.parent || !outer_names.contains(counter.name.as_str())
        })
        .map(|counter| (counter.name.as_str(), counter.creator))
        .collect::<HashSet<_>>();
    value_source
        .iter()
        .filter(|counter| inherited_source.contains(&(counter.name.as_str(), counter.creator)))
        .cloned()
        .collect()
}

fn apply_counter_properties(counters: &mut CounterSet, target: Target, style: &ComputedStyle) {
    if let Some(resets) = computed_counter_pairs(style, "counter-reset", 0) {
        for (name, value) in resets {
            instantiate_counter(counters, target, name, value);
        }
    }
    if let Some(increments) = computed_counter_pairs(style, "counter-increment", 1) {
        for (name, amount) in increments {
            ensure_counter(counters, target, &name);
            if let Some(counter) = counters.iter_mut().rfind(|counter| counter.name == name) {
                counter.value = counter.value.saturating_add(amount);
            }
        }
    }
    if let Some(sets) = computed_counter_pairs(style, "counter-set", 0) {
        for (name, value) in sets {
            ensure_counter(counters, target, &name);
            if let Some(counter) = counters.iter_mut().rfind(|counter| counter.name == name) {
                counter.value = value;
            }
        }
    }
}

fn computed_counter_pairs(
    style: &ComputedStyle,
    property: &str,
    default: i32,
) -> Option<Vec<(String, i32)>> {
    let computed = style.get(property)?.css_text();
    let declarations = parse_style_attribute(&format!("{property}: {computed}"));
    let declaration = declarations
        .first()
        .filter(|_| declarations.len() == 1 && declarations[0].name == property)?;
    counter_pairs(&declaration.value, default)
}

fn instantiate_counter(counters: &mut CounterSet, target: Target, name: String, value: i32) {
    if let Some(index) = counters.iter().rposition(|counter| counter.name == name)
        && (counters[index].creator == target || counters[index].creator.parent == target.parent)
    {
        counters.remove(index);
    }
    counters.push(Counter {
        name,
        creator: target,
        value,
    });
}

fn ensure_counter(counters: &mut CounterSet, target: Target, name: &str) {
    if !counters.iter().any(|counter| counter.name == name) {
        counters.push(Counter {
            name: name.to_string(),
            creator: target,
            value: 0,
        });
    }
}

fn referenced_counter_names(value: &Value) -> Vec<String> {
    let mut names = Vec::new();
    collect_counter_names(value, &mut names);
    names
}

fn collect_counter_names(value: &Value, names: &mut Vec<String>) {
    match value {
        Value::Function { name, arguments }
            if name.eq_ignore_ascii_case("counter") || name.eq_ignore_ascii_case("counters") =>
        {
            if let Some(Value::Keyword(counter_name)) = arguments.first()
                && !names.iter().any(|name| name == counter_name)
            {
                names.push(counter_name.clone());
            }
        }
        Value::Function { arguments, .. }
        | Value::List(arguments)
        | Value::CommaList(arguments) => {
            for argument in arguments {
                collect_counter_names(argument, names);
            }
        }
        _ => {}
    }
}

fn generates_pseudo_box(style: &ComputedStyle) -> bool {
    match style.get("content") {
        None => false,
        Some(ComputedValue::Keyword(value)) => {
            !value.eq_ignore_ascii_case("none") && !value.eq_ignore_ascii_case("normal")
        }
        Some(_) => true,
    }
}
