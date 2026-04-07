//! Observer module for DOM diffing and monitoring.
use crate::semantic::{Element, SemanticView};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

/// Result of diffing two SemanticViews.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SemanticDiff {
    pub added: Vec<Element>,
    pub removed: Vec<Element>,
    /// Tuple of (old_element, new_element)
    pub changed: Vec<(Element, Element)>,
    pub notifications: Vec<String>,
}

/// Diff two semantic views to compute added, removed, and changed elements.
pub fn diff(old: &SemanticView, new: &SemanticView) -> SemanticDiff {
    let mut added = Vec::new();
    let mut removed = Vec::new();
    let mut changed = Vec::new();
    let mut notifications = Vec::new();

    // Use lad-id as the primary key. If the DOM structure shifts drastically,
    // this might yield noisy diffs, but it's the best stable anchor available.
    let old_map: HashMap<u32, &Element> = old.elements.iter().map(|e| (e.id, e)).collect();
    let mut new_ids = HashSet::new();

    for new_el in &new.elements {
        new_ids.insert(new_el.id);
        if let Some(old_el) = old_map.get(&new_el.id) {
            let mut is_changed = false;

            // Check value changes (input typing)
            if old_el.value != new_el.value {
                is_changed = true;
                if old_el.value.is_none() && new_el.value.is_some() {
                    notifications.push(format!("Text added to '{}'", new_el.label));
                } else {
                    notifications.push(format!("Text changed in '{}'", new_el.label));
                }
            }

            // Check disabled state changes (e.g. form submitted / submit button enabled)
            if old_el.disabled != new_el.disabled {
                is_changed = true;
                if new_el.disabled {
                    notifications.push(format!("Element '{}' disabled", new_el.label));
                } else {
                    notifications.push(format!("Element '{}' enabled", new_el.label));
                }
            }

            if is_changed {
                changed.push(((*old_el).clone(), new_el.clone()));
            }
        } else {
            added.push(new_el.clone());
            notifications.push(format!("New element appeared: '{}'", new_el.label));
        }
    }

    for old_el in &old.elements {
        if !new_ids.contains(&old_el.id) {
            removed.push(old_el.clone());
            notifications.push(format!("Element removed: '{}'", old_el.label));
        }
    }

    // Additional generic notifications
    if old.visible_text != new.visible_text
        && old.visible_text.len().abs_diff(new.visible_text.len()) > 10
    {
        notifications.push("Visible text on page changed noticeably".to_string());
    }

    if old.title != new.title {
        notifications.push(format!("Title changed to '{}'", new.title));
    }

    SemanticDiff {
        added,
        removed,
        changed,
        notifications,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::semantic::{ElementKind, PageState};

    fn make_view(elements: Vec<Element>) -> SemanticView {
        SemanticView {
            url: "http://test".into(),
            title: "Test".into(),
            page_hint: "".into(),
            elements,
            forms: vec![],
            visible_text: "".into(),
            state: PageState::Ready,
            element_cap: None,
            blocked_reason: None,
            session_context: None,
        }
    }

    fn make_element(id: u32, label: &str, val: Option<&str>) -> Element {
        Element {
            id,
            kind: ElementKind::Input,
            label: label.into(),
            name: None,
            value: val.map(|s| s.to_string()),
            placeholder: None,
            href: None,
            input_type: None,
            disabled: false,
            form_index: None,
            context: None,
            hint: None,
            frame_index: None,
        }
    }

    #[test]
    fn test_diff_semantics() {
        let old = make_view(vec![make_element(1, "E-mail", None)]);
        let new = make_view(vec![
            make_element(1, "E-mail", Some("test@example.com")),
            make_element(2, "Submit", None),
        ]);

        let d = diff(&old, &new);
        assert_eq!(d.added.len(), 1);
        assert_eq!(d.changed.len(), 1);
        assert_eq!(d.removed.len(), 0);
        assert_eq!(d.notifications.len(), 2);
    }
}
