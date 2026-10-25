//! Owned form-control indices shared for one unchanged selector match pass.
use super::*;
pub(super) use crate::dom::form_controls::state_index::FormStateIndex;
use crate::dom::form_controls::state_index::{build, tree_root};

/// Borrows only the match-pass cache; no DOM handle survives index construction.
pub(super) fn for_node<'cache>(
    node: &NodeHandle,
    cache: &'cache mut SelectorMatchCache,
) -> &'cache FormStateIndex {
    use std::collections::hash_map::Entry;
    let root = tree_root(node);
    match cache.form_states.entry(root.identity()) {
        Entry::Occupied(entry) => entry.into_mut(),
        Entry::Vacant(entry) => {
            #[cfg(test)]
            {
                cache.form_state_builds += 1;
            }
            entry.insert(build(&root))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_radio_groups_share_one_owned_index() {
        let doc = crate::html::TreeBuilder::parse("<form><button>B</button><input id=a type=radio name=g><input id=b type=radio name=g></form>").document();
        let mut cache = SelectorMatchCache::default();
        let button = doc.query_selector("button").unwrap();
        let a = doc.query_selector("#a").unwrap();
        let b = doc.query_selector("#b").unwrap();
        assert!(super::super::default_state::matches(&button, &mut cache));
        assert!(super::super::indeterminate_state::matches(&a, &mut cache));
        assert!(super::super::indeterminate_state::matches(&b, &mut cache));
        assert_eq!(cache.form_states.len(), 1);
        assert_eq!(cache.form_state_builds, 1);
    }

    #[test]
    fn radio_membership_does_not_retain_removed_dom() {
        let mut cache = SelectorMatchCache::default();
        let weak = {
            let doc = crate::html::TreeBuilder::parse("<form><input type=radio name=g></form>")
                .document();
            let radio = doc.query_selector("input").unwrap();
            assert!(super::super::indeterminate_state::matches(
                &radio, &mut cache
            ));
            doc.downgrade()
        };
        assert!(weak.upgrade().is_none());
    }

    #[test]
    fn radio_names_keep_exact_utf16_group_identity() {
        let doc = crate::html::TreeBuilder::parse("<form><input id=a type=radio name=g><input id=b type=radio name=g><input id=c type=radio name=g></form>").document();
        let a = doc.query_selector("#a").unwrap();
        let b = doc.query_selector("#b").unwrap();
        let c = doc.query_selector("#c").unwrap();
        a.set_attribute_value_ns_utf16(None, "name", &[0xd800]);
        b.set_attribute_value_ns_utf16(None, "name", &[0xd800]);
        c.set_attribute_value_ns_utf16(None, "name", &[0xd801]);
        a.set_checked(true);
        let mut cache = SelectorMatchCache::default();
        assert!(!super::super::indeterminate_state::matches(&a, &mut cache));
        assert!(!super::super::indeterminate_state::matches(&b, &mut cache));
        assert!(super::super::indeterminate_state::matches(&c, &mut cache));
    }
}
