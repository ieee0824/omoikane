use super::*;
use configuration::{Element, Pi};

fn remove<T: PartialEq>(list: &mut Option<Vec<T>>, item: &T) -> bool {
    let Some(list) = list else {
        return false;
    };
    let before = list.len();
    list.retain(|value| value != item);
    list.len() != before
}
fn deduplicate<T: PartialEq>(list: &mut Vec<T>) {
    let mut index = 0;
    while index < list.len() {
        if list[..index].contains(&list[index]) {
            list.remove(index);
        } else {
            index += 1;
        }
    }
}
fn boolean(slot: &mut Option<bool>, value: bool) -> bool {
    if *slot == Some(value) {
        return false;
    }
    *slot = Some(value);
    true
}

impl Config {
    pub fn modify(
        &mut self,
        operation: &str,
        value: serde_json::Value,
    ) -> Result<bool, &'static str> {
        let changed = match operation {
            "get" => false,
            "allowElement" => {
                self.allow_element(serde_json::from_value(value).map_err(|_| "Invalid element")?)
            }
            "removeElement" => {
                self.remove_element(&serde_json::from_value(value).map_err(|_| "Invalid element")?)
            }
            "replaceElementWithChildren" => {
                self.replace_element(&serde_json::from_value(value).map_err(|_| "Invalid element")?)
            }
            "allowAttribute" => self
                .allow_attribute(serde_json::from_value(value).map_err(|_| "Invalid attribute")?),
            "removeAttribute" => self
                .remove_attribute(&serde_json::from_value(value).map_err(|_| "Invalid attribute")?),
            "setComments" => boolean(
                &mut self.comments,
                value.as_bool().ok_or("Invalid boolean")?,
            ),
            "setJavascriptURLs" => boolean(
                &mut self.javascript_urls,
                value.as_bool().ok_or("Invalid boolean")?,
            ),
            "setDataAttributes" => {
                self.set_data_attributes(value.as_bool().ok_or("Invalid boolean")?)
            }
            "allowProcessingInstruction" => self.modify_pi(
                serde_json::from_value(value).map_err(|_| "Invalid processing instruction")?,
                true,
            ),
            "removeProcessingInstruction" => self.modify_pi(
                serde_json::from_value(value).map_err(|_| "Invalid processing instruction")?,
                false,
            ),
            "removeUnsafe" => self.remove_unsafe(),
            _ => return Err("Unknown sanitizer operation"),
        };
        Ok(changed)
    }

    fn remove_element(&mut self, key: &Name) -> bool {
        let changed = remove(&mut self.replace_with_children_elements, key);
        if let Some(elements) = &mut self.elements {
            let before = elements.len();
            elements.retain(|entry| entry.key != *key);
            changed || before != elements.len()
        } else {
            let removed = self
                .remove_elements
                .as_mut()
                .expect("canonical remove list");
            if removed.contains(key) {
                return changed;
            }
            removed.push(key.clone());
            true
        }
    }

    fn replace_element(&mut self, key: &Name) -> bool {
        if key.non_replaceable() {
            return false;
        }
        let mut changed = remove(&mut self.remove_elements, key);
        if let Some(elements) = &mut self.elements {
            let before = elements.len();
            elements.retain(|entry| entry.key != *key);
            changed |= before != elements.len();
        }
        let replacements = self
            .replace_with_children_elements
            .get_or_insert_with(Vec::new);
        if !replacements.contains(key) {
            replacements.push(key.clone());
            return true;
        }
        changed
    }

    fn allow_element(&mut self, mut element: Element) -> bool {
        if element.attributes.is_none() && element.remove_attributes.is_none() {
            element.remove_attributes = Some(Vec::new());
        }
        if self.elements.is_none() {
            if element.attributes.is_some()
                || element
                    .remove_attributes
                    .as_ref()
                    .is_some_and(|list| !list.is_empty())
            {
                return false;
            }
            let changed = remove(&mut self.replace_with_children_elements, &element.key);
            return remove(&mut self.remove_elements, &element.key) || changed;
        }
        self.normalize_local_attributes(&mut element);
        let changed = remove(&mut self.replace_with_children_elements, &element.key);
        let elements = self.elements.as_mut().expect("allow list");
        if let Some(current) = elements.iter_mut().find(|entry| entry.key == element.key) {
            if *current == element {
                return changed;
            }
            *current = element;
        } else {
            elements.push(element);
        }
        true
    }

    fn normalize_local_attributes(&self, element: &mut Element) {
        if let Some(global) = &self.attributes {
            if let Some(allowed) = &mut element.attributes {
                deduplicate(allowed);
                allowed.retain(|key| {
                    !global.contains(key) && !(self.data_attributes == Some(true) && key.is_data())
                });
            }
            if let Some(removed) = &mut element.remove_attributes {
                deduplicate(removed);
                removed.retain(|key| global.contains(key));
            }
        } else {
            let global = self.remove_attributes.as_deref().unwrap_or_default();
            if let Some(allowed) = &mut element.attributes {
                deduplicate(allowed);
                let removed = element.remove_attributes.take().unwrap_or_default();
                allowed.retain(|key| !global.contains(key) && !removed.contains(key));
            }
            if let Some(removed) = &mut element.remove_attributes {
                deduplicate(removed);
                removed.retain(|key| !global.contains(key));
            }
        }
    }

    fn allow_attribute(&mut self, key: Name) -> bool {
        if let Some(global) = &mut self.attributes {
            if (self.data_attributes == Some(true) && key.is_data()) || global.contains(&key) {
                return false;
            }
            for element in self.elements.iter_mut().flatten() {
                remove(&mut element.attributes, &key);
            }
            global.push(key);
            true
        } else {
            remove(&mut self.remove_attributes, &key)
        }
    }

    fn remove_attribute(&mut self, key: &Name) -> bool {
        if self.attributes.is_some() {
            let mut changed = remove(&mut self.attributes, key);
            for element in self.elements.iter_mut().flatten() {
                changed |= remove(&mut element.attributes, key);
                remove(&mut element.remove_attributes, key);
            }
            changed
        } else {
            if self
                .remove_attributes
                .as_ref()
                .is_some_and(|list| list.contains(key))
            {
                return false;
            }
            for element in self.elements.iter_mut().flatten() {
                remove(&mut element.attributes, key);
                remove(&mut element.remove_attributes, key);
            }
            self.remove_attributes
                .as_mut()
                .expect("canonical remove list")
                .push(key.clone());
            true
        }
    }

    fn set_data_attributes(&mut self, allow: bool) -> bool {
        let Some(attributes) = &mut self.attributes else {
            return false;
        };
        if self.data_attributes == Some(allow) {
            return false;
        }
        if allow {
            attributes.retain(|key| !key.is_data());
            for element in self.elements.iter_mut().flatten() {
                if let Some(local) = &mut element.attributes {
                    local.retain(|key| !key.is_data());
                }
            }
        }
        self.data_attributes = Some(allow);
        true
    }

    fn modify_pi(&mut self, pi: Pi, allow: bool) -> bool {
        match (
            &mut self.processing_instructions,
            &mut self.remove_processing_instructions,
        ) {
            (Some(list), _) if allow => {
                if list.contains(&pi) {
                    false
                } else {
                    list.push(pi);
                    true
                }
            }
            (Some(list), _) => {
                let before = list.len();
                list.retain(|value| *value != pi);
                before != list.len()
            }
            (_, removed) if allow => remove(removed, &pi),
            (_, Some(list)) => {
                if list.contains(&pi) {
                    false
                } else {
                    list.push(pi);
                    true
                }
            }
            _ => unreachable!("canonical processing instruction lists"),
        }
    }

    pub fn remove_unsafe(&mut self) -> bool {
        let mut changed = false;
        for key in &CONSTANTS.unsafe_elements {
            changed |= self.remove_element(key);
        }
        for name in &CONSTANTS.event_attributes {
            changed |= self.remove_attribute(&Name::new(name, None));
        }
        changed | boolean(&mut self.javascript_urls, false)
    }
}
