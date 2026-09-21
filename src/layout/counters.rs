use std::collections::{HashMap, HashSet};

use crate::css::style::counter_pairs;
use crate::css::{ComputedStyle, ComputedValue, PseudoElement, StyleResolver, Value};
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
    let mut root = node.clone();
    while let Some(parent) = root.parent_node() {
        root = parent;
    }

    let mut processor = Processor {
        resolver,
        snapshots: HashMap::new(),
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
}

struct Processor<'a> {
    resolver: &'a mut StyleResolver,
    snapshots: HashMap<(usize, Option<PseudoElement>), Snapshot>,
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
        let style = self.resolver.computed_style(node);
        let hidden = ancestor_hidden || display_none(&style);
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
        let style = self.resolver.computed_pseudo_style(node, pseudo)?;
        if display_none(&style) || !generates_pseudo_box(&style) {
            return None;
        }
        let target = Target {
            node: node.identity(),
            pseudo: Some(pseudo),
            parent: Some(node.identity()),
        };
        let mut counters = inherit_counters(counter_source, value_source, target);
        apply_counter_properties(&mut counters, target, &style);
        if let Some(content) = style.component_value("content") {
            let names = referenced_counter_names(content);
            for name in &names {
                ensure_counter(&mut counters, target, &name);
            }
            self.store(target, &counters, &names);
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

fn element_children(node: &NodeHandle) -> Vec<NodeHandle> {
    node.layout_child_nodes()
        .into_iter()
        .filter(|child| child.node_type() == NodeType::Element)
        .collect()
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
    if let Some(value) = style.component_value("counter-reset")
        && let Some(resets) = counter_pairs(value, 0)
    {
        for (name, value) in resets {
            instantiate_counter(counters, target, name, value);
        }
    }
    if let Some(value) = style.component_value("counter-increment")
        && let Some(increments) = counter_pairs(value, 1)
    {
        for (name, amount) in increments {
            ensure_counter(counters, target, &name);
            if let Some(counter) = counters.iter_mut().rfind(|counter| counter.name == name) {
                counter.value = counter.value.saturating_add(amount);
            }
        }
    }
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

fn display_none(style: &ComputedStyle) -> bool {
    matches!(style.get("display"), Some(ComputedValue::Keyword(value)) if value.eq_ignore_ascii_case("none"))
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
