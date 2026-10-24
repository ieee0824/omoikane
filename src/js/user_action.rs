//! Owner-local dynamic selector flag updates.
use super::*;

impl HostState {
    /// Composed paths cross shadow hosts; hover also crosses iframe boundaries.
    /// Store only node ids, so a detached subtree is not kept alive by input.
    fn user_action_path(&self, target_id: Option<usize>, cross_iframes: bool) -> Vec<usize> {
        let mut current = target_id
            .and_then(|id| self.get_node(id))
            .filter(|node| self.node_is_in_active_document(node));
        let mut path = Vec::new();
        let mut seen = HashSet::new();
        while let Some(node) = current {
            if !seen.insert(node.identity()) {
                break;
            }
            if node.node_type() == NodeType::Element {
                path.push(node.identity());
            }
            current = node
                .assigned_slot()
                .or_else(|| node.parent_node())
                .or_else(|| node.shadow_host());
            if cross_iframes && current.is_none() && node.node_type() == NodeType::Document {
                current = self
                    .iframe_documents
                    .iter()
                    .find(|(_, entry)| entry.document.identity() == node.identity())
                    .and_then(|(iframe_id, _)| self.get_node(*iframe_id));
            }
        }
        path
    }

    /// Focus also matches shadow hosts whose shadow tree contains the focused
    /// element. Slotted light-DOM children do not focus their host.
    fn focus_subjects(&self, target_id: Option<usize>) -> Vec<usize> {
        let mut current = target_id
            .and_then(|id| self.get_node(id))
            .filter(|node| self.node_is_in_active_document(node));
        let mut subjects = Vec::new();
        let mut seen = HashSet::new();
        if let Some(node) = &current {
            subjects.push(node.identity());
        }
        while let Some(node) = current {
            if !seen.insert(node.identity()) {
                break;
            }
            if let Some(host) = node.shadow_host() {
                subjects.push(host.identity());
            }
            current = node.parent_node().or_else(|| node.shadow_host());
        }
        subjects
    }

    /// Changes dynamic selector flags once per input transition and invalidates
    /// the affected documents without reparsing their stylesheets.
    pub(super) fn update_user_action_target(
        &mut self,
        kind: &str,
        target_id: Option<usize>,
        visible: bool,
    ) {
        let next = self.user_action_path(target_id, kind == "hover");
        let next_subjects = (kind == "focus").then(|| self.focus_subjects(target_id));
        let next_visible = if kind == "focus" && visible {
            next.first().copied()
        } else {
            None
        };
        let unchanged = match kind {
            "hover" => self.hover_path == next,
            "active" => self.active_path == next,
            "focus" => {
                self.focus_path == next
                    && next_subjects
                        .as_ref()
                        .is_some_and(|subjects| &self.focus_subjects == subjects)
                    && self.focus_visible_id == next_visible
            }
            _ => return,
        };
        if unchanged {
            return;
        }
        self.native_scroll.revoke_policy();
        let old_subjects = next_subjects
            .as_ref()
            .map(|subjects| std::mem::replace(&mut self.focus_subjects, subjects.clone()));
        let old = match kind {
            "hover" => std::mem::replace(&mut self.hover_path, next.clone()),
            "active" => std::mem::replace(&mut self.active_path, next.clone()),
            "focus" => std::mem::replace(&mut self.focus_path, next.clone()),
            _ => return,
        };
        let flag = match kind {
            "focus" => "focus-within",
            "hover" => "hover",
            "active" => "active",
            _ => return,
        };
        let old_ids: HashSet<_> = old.iter().copied().collect();
        let next_ids: HashSet<_> = next.iter().copied().collect();
        let previous_visible = self.focus_visible_id;
        let mut changes = ActionChanges::default();
        for id in old_ids.difference(&next_ids) {
            changes.set(self, *id, flag, false);
        }
        for id in next_ids.difference(&old_ids) {
            changes.set(self, *id, flag, true);
        }
        if kind == "focus" {
            let old_focus: HashSet<_> = old_subjects.unwrap_or_default().into_iter().collect();
            let new_focus: HashSet<_> = next_subjects.unwrap_or_default().into_iter().collect();
            for id in old_focus.difference(&new_focus) {
                changes.set(self, *id, "focus", false);
            }
            for id in new_focus.difference(&old_focus) {
                changes.set(self, *id, "focus", true);
            }
            if previous_visible != next_visible {
                if let Some(id) = previous_visible {
                    changes.set(self, id, "focus-visible", false);
                }
                if let Some(id) = next_visible {
                    changes.set(self, id, "focus-visible", true);
                }
            }
        }
        if kind == "focus" {
            self.focus_visible_id = next_visible;
        }
        changes.invalidate(self, kind);
    }
}

#[derive(Default)]
struct ActionChanges {
    documents: HashMap<usize, ChangedDocument>,
}

struct ChangedDocument {
    document: NodeHandle,
    flags: Vec<(NodeHandle, &'static str)>,
}

impl ActionChanges {
    fn set(&mut self, state: &HostState, id: usize, flag: &'static str, enabled: bool) {
        if let Some(node) = state.get_node(id)
            && node.set_user_action_state(flag, enabled)
            && let Some(document) = document_root_for_node(&node)
        {
            self.documents
                .entry(document.identity())
                .or_insert_with(|| ChangedDocument {
                    document,
                    flags: Vec::new(),
                })
                .flags
                .push((node, flag));
        }
    }

    fn invalidate(self, state: &mut HostState, kind: &str) {
        let affected_documents = self.documents.len();
        let mut invalidated_documents = 0;
        let mut local_documents = 0;
        let mut full_documents = 0;
        let mut cache_outcomes = std::collections::BTreeMap::<&'static str, usize>::new();
        for changes in self.documents.into_values() {
            let outcome = state
                .document_styles
                .get_mut(&changes.document.identity())
                .and_then(|entry| entry.resolver.as_mut())
                .map_or(
                    crate::css::style::UserActionStyleInvalidation::Full("deferred_resolver"),
                    |resolver| resolver.invalidate_user_action_styles(&changes.flags),
                );
            if outcome.changed() {
                let preserve_main_layout = outcome.preserves_layout();
                *cache_outcomes.entry(outcome.label()).or_default() += 1;
                if outcome.is_full() {
                    full_documents += 1;
                    task_diagnostics::record_other_full();
                } else {
                    local_documents += 1;
                    task_diagnostics::record_local_user_action_invalidation();
                }
                for (node, _) in &changes.flags {
                    frame_diagnostics::record_style_cache_invalidation(
                        changes.document.identity(),
                        node.identity(),
                        outcome.label(),
                    );
                }
                state.form_validation.invalidate();
                if let Some(entry) = state.document_styles.get_mut(&changes.document.identity()) {
                    entry.needs_full_sample = true;
                    entry.child_layout = None;
                }
                state.invalidate_document_style_outputs(&changes.document, preserve_main_layout);
                invalidated_documents += 1;
            }
        }
        static TRACE_PAINT: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        if affected_documents != 0
            && *TRACE_PAINT.get_or_init(|| std::env::var_os("OMOIKANE_TRACE_PAINT").is_some())
        {
            eprintln!(
                "{}",
                crate::diagnostics::json_record(
                    "OMOIKANE_USER_ACTION",
                    &serde_json::json!({"kind": kind, "affected_documents": affected_documents,
                    "invalidated_documents": invalidated_documents,
                    "local_documents": local_documents, "full_documents": full_documents,
                    "cache_outcomes": cache_outcomes})
                )
            );
        }
    }
}
