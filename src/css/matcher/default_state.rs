//! Default form controls, indexed only for an unchanged selector match pass.

use super::*;
use crate::dom::form_controls::{html_local_name, is_submit_button};

/// Matches content defaults and the first submit button of each form owner.
pub(super) fn matches(node: &NodeHandle, cache: &mut SelectorMatchCache) -> bool {
    match html_local_name(node).as_deref() {
        Some("option") => return node.attribute_value_ns(None, "selected").is_some(),
        Some("input") => {
            let kind = node.attribute_value_ns(None, "type").unwrap_or_default();
            if kind.eq_ignore_ascii_case("checkbox") || kind.eq_ignore_ascii_case("radio") {
                return node.attribute_value_ns(None, "checked").is_some();
            }
        }
        Some("button") => {}
        _ => return false,
    }
    if !is_submit_button(node) {
        return false;
    }
    super::form_state_index::for_node(node, cache)
        .defaults
        .contains(&node.identity())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_index_is_shared_by_candidates_in_the_same_root() {
        let doc = crate::html::TreeBuilder::parse(
            "<form><button id=a>A</button><button id=b>B</button></form>",
        )
        .document();
        let a = doc.query_selector("#a").unwrap();
        let b = doc.query_selector("#b").unwrap();
        let mut cache = SelectorMatchCache::default();
        assert!(matches(&a, &mut cache));
        assert!(!matches(&b, &mut cache));
        assert_eq!(cache.form_states.len(), 1);
        assert_eq!(cache.form_state_builds, 1);
    }

    #[test]
    fn cached_default_controls_do_not_retain_the_tree() {
        let mut cache = SelectorMatchCache::default();
        let weak = {
            let doc = crate::html::TreeBuilder::parse("<form><button>A</button></form>").document();
            let button = doc.query_selector("button").unwrap();
            assert!(matches(&button, &mut cache));
            doc.downgrade()
        };
        assert!(weak.upgrade().is_none());
    }

    #[test]
    fn matching_inside_a_read_view_does_not_update_parser_owner_metadata() {
        let doc = crate::html::TreeBuilder::parse("<form id=f></form><input id=i type=submit>")
            .document();
        let form = doc.query_selector("#f").unwrap();
        let input = doc.query_selector("#i").unwrap();
        input.set_parser_form_owner(&form);
        assert_eq!(input.parser_form_owner(), Some(form.clone()));
        let parent = form.parent_node().unwrap();
        parent.remove_child(&form);
        parent.append_child(form);
        let mut cache = SelectorMatchCache::default();
        assert!(input.with_tag_name(|_| !matches(&input, &mut cache)));
    }
}
