//! Conservative test for whether generated content can read CSS counters.

use super::{Declaration, Rule, StyleResolver, Value};

impl StyleResolver {
    /// Whether a layout pass may need generated-content counter snapshots.
    ///
    /// This deliberately examines inactive and scoped rules too: media changes,
    /// shadow scopes, and CSSOM edits must not make a cached negative answer
    /// stale. Animation state stores timing and a name, not keyframe values;
    /// each sample retrieves keyframes from the current resolver. Transitions
    /// cannot interpolate `content`, and animated custom values can reach it
    /// only through a current `content: var(...)` declaration. Thus neither
    /// timeline alone can introduce a generated counter reader.
    pub(crate) fn may_read_generated_counters(&self) -> bool {
        self.stylesheets
            .iter()
            .any(|input| input.stylesheet.rules.iter().any(rule_may_read_counters))
    }
}

fn rule_may_read_counters(rule: &Rule) -> bool {
    match rule {
        Rule::Style(rule) => {
            declarations_may_read_counters(&rule.declarations)
                || rule.rules.iter().any(rule_may_read_counters)
        }
        Rule::At(rule) => {
            declarations_may_read_counters(&rule.declarations)
                || rule.declarations.iter().any(|declaration| {
                    if declaration.name != "__keyframes_block" {
                        return false;
                    }
                    let Value::Keyword(block) = &declaration.value else {
                        return true;
                    };
                    super::parse_keyframe_steps(block)
                        .iter()
                        .any(|step| declarations_may_read_counters(&step.declarations))
                })
                || rule
                    .block
                    .as_ref()
                    .is_some_and(|rules| rules.iter().any(rule_may_read_counters))
        }
        Rule::FontFace(_) => false,
    }
}

fn declarations_may_read_counters(declarations: &[Declaration]) -> bool {
    declarations.iter().any(|declaration| {
        (declaration.name.eq_ignore_ascii_case("content")
            || declaration.name.eq_ignore_ascii_case("all"))
            && value_may_read_counters(&declaration.value)
    })
}

fn value_may_read_counters(value: &Value) -> bool {
    match value {
        // An untyped attr() with no fallback always yields a string. Even an
        // attribute containing "counter(x)" is literal text, never a counter
        // reader. Typed forms and fallbacks remain conservative below.
        Value::Function { name, arguments }
            if name.eq_ignore_ascii_case("attr")
                && matches!(arguments.as_slice(), [Value::Keyword(_)]) =>
        {
            false
        }
        // Includes counter(), counters(), var(), typed attr() and future functions.
        Value::Function { .. } | Value::TreeCounting { .. } => true,
        Value::List(values) | Value::CommaList(values) => {
            values.iter().any(value_may_read_counters)
        }
        // The cascade can replace these with a counter-reading declaration.
        Value::Keyword(keyword) => matches!(
            keyword.to_ascii_lowercase().as_str(),
            "inherit" | "revert" | "revert-layer"
        ),
        // A quoted `counter(x)` is literal text, not a function call.
        Value::String(_)
        | Value::Length(..)
        | Value::Color(_)
        | Value::Number(_)
        | Value::Percentage(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::css::{Origin, parse_stylesheet};

    fn resolver_with(css: &str) -> StyleResolver {
        let mut resolver = StyleResolver::new();
        resolver.add_stylesheet(Origin::Author, parse_stylesheet(css).unwrap());
        resolver
    }

    #[test]
    fn literal_content_and_ordinary_list_do_not_need_snapshots() {
        let resolver =
            resolver_with("ol { counter-reset: list-item; } li::before { content: 'counter(x)'; }");
        assert!(!resolver.may_read_generated_counters());
        assert!(
            !resolver_with("@keyframes fade { to { opacity: 0; } }").may_read_generated_counters()
        );
    }

    #[test]
    fn reader_and_escaped_function_are_detected() {
        for css in [
            "li::before { content: counter(item); }",
            "li::before { content: counters(item, '.'); }",
            r"li::before { content: c\6f unter(item); }",
        ] {
            assert!(resolver_with(css).may_read_generated_counters(), "{css}");
        }
    }

    #[test]
    fn untyped_attribute_strings_do_not_require_counter_snapshots() {
        for css in [
            "x::before { content: attr(data-title); }",
            "x::after { content: 'Label: ' attr(data-label); }",
            r"x::before { content: a\74tr(data-title); }",
        ] {
            assert!(!resolver_with(css).may_read_generated_counters(), "{css}");
        }
    }

    #[test]
    fn attribute_fallback_and_typed_values_remain_conservative() {
        for css in [
            "x::before { content: attr(data-title, counter(x)); }",
            "x::before { content: attr(data-title type(*)); }",
            "x::before { content: attr(data-title) counter(x); }",
        ] {
            assert!(resolver_with(css).may_read_generated_counters(), "{css}");
        }
    }

    #[test]
    fn dynamic_content_is_conservative_across_rule_trees() {
        for css in [
            "x::before { content: var(--value, counter(x)); }",
            "x { --value: counter(x); } x::before { content: var(--value); }",
            "@property --value { syntax: '*'; initial-value: counter(x); inherits: false; } x::before { content: var(--value); }",
            "@media (min-width: 1px) { x::before { content: counter(x); } }",
            "@keyframes change { to { content: counter(x); } }",
            r"@-webkit-keyframes change { to { c\6f ntent: counter(x); } }",
            "x::before { content: inherit; }",
            "x::before { all: inherit; }",
            "x::before { all: var(--cascade, inherit); }",
        ] {
            assert!(resolver_with(css).may_read_generated_counters(), "{css}");
        }
    }

    #[test]
    fn all_origins_and_added_sheets_are_scanned() {
        let mut resolver = resolver_with("x::before { content: 'x'; }");
        assert!(!resolver.may_read_generated_counters());
        resolver.add_stylesheet(
            Origin::User,
            parse_stylesheet("x::after { content: counter(x); }").unwrap(),
        );
        assert!(resolver.may_read_generated_counters());
    }

    #[test]
    fn shadow_scoped_sheet_is_scanned() {
        let mut resolver = StyleResolver::new();
        resolver.add_scoped_stylesheet(
            Origin::Author,
            parse_stylesheet("span::before { content: counter(x); }").unwrap(),
            crate::dom::NodeHandle::element("shadow-root"),
        );
        assert!(resolver.may_read_generated_counters());
    }

    #[test]
    fn opacity_animation_state_does_not_create_a_counter_reader() {
        let node = crate::dom::NodeHandle::element("div");
        let mut resolver = resolver_with(
            "@keyframes fade { from { opacity: 0; } to { opacity: 1; } } \
             div { animation-name: fade; animation-duration: 0.1s; \
             animation-fill-mode: forwards; }",
        );
        resolver.set_animation_time_ms(0.0);
        let _ = resolver.computed_style_shared(&node);
        assert!(!resolver.animation_node_ids().is_empty());
        assert!(resolver.has_running_animations());
        assert!(!resolver.may_read_generated_counters());
        resolver.set_animation_time_ms(1000.0);
        assert!(!resolver.has_running_animations());
        assert!(!resolver.may_read_generated_counters());

        // The filled clock retains timing, but cannot supply removed keyframes.
        let timeline = resolver.take_animation_timeline();
        let mut without_sheet = StyleResolver::new();
        without_sheet.install_animation_timeline(timeline);
        assert!(!without_sheet.animation_node_ids().is_empty());
        assert!(!without_sheet.may_read_generated_counters());
        let _ = without_sheet.computed_style_shared(&node);
        assert!(without_sheet.animation_node_ids().is_empty());
    }

    #[test]
    fn current_content_keyframes_keep_the_guard_even_with_a_filled_clock() {
        let node = crate::dom::NodeHandle::element("div");
        let mut resolver = resolver_with(
            "@keyframes words { to { content: counter(x); } } \
             div { animation-name: words; animation-duration: 0.1s; \
             animation-fill-mode: forwards; }",
        );
        resolver.set_animation_time_ms(0.0);
        let _ = resolver.computed_style_shared(&node);
        resolver.set_animation_time_ms(1000.0);
        assert!(!resolver.has_running_animations());
        assert!(resolver.may_read_generated_counters());
    }

    #[test]
    fn opacity_transition_does_not_create_a_counter_reader() {
        let node = crate::dom::NodeHandle::element("div");
        node.set_attribute("style", "opacity: 0; transition: opacity 1s");
        let mut resolver = StyleResolver::new();
        let _ = resolver.computed_style_shared(&node);
        node.set_attribute("style", "opacity: 1; transition: opacity 1s");
        resolver.invalidate_style_cache_for_test();
        let _ = resolver.computed_style_shared(&node);
        assert!(resolver.transition_timeline.has_running_transitions());
        assert!(!resolver.may_read_generated_counters());
    }
}
