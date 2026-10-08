use super::*;

/// Namespace and local name are the equality key; prefixes are irrelevant.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub(super) struct Name {
    pub name: String,
    pub namespace: Option<String>,
}

fn compare_namespace(a: &Option<String>, b: &Option<String>) -> std::cmp::Ordering {
    match (a, b) {
        (Some(a), Some(b)) => a.encode_utf16().cmp(b.encode_utf16()),
        _ => a.is_some().cmp(&b.is_some()),
    }
}

impl Ord for Name {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        compare_namespace(&self.namespace, &other.namespace)
            .then_with(|| self.name.encode_utf16().cmp(other.name.encode_utf16()))
    }
}
impl PartialOrd for Name {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Name {
    pub fn new(name: impl Into<String>, namespace: Option<&str>) -> Self {
        Self {
            name: name.into(),
            namespace: namespace.map(str::to_owned),
        }
    }
    pub fn is_data(&self) -> bool {
        self.namespace.is_none() && self.name.starts_with("data-")
    }
    pub fn non_replaceable(&self) -> bool {
        matches!(
            (self.namespace.as_deref(), self.name.as_str()),
            (Some(HTML), "html") | (Some(SVG), "svg") | (Some(MATHML), "math")
        )
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(super) struct Element {
    #[serde(flatten)]
    pub key: Name,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attributes: Option<Vec<Name>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remove_attributes: Option<Vec<Name>>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct Pi {
    pub target: String,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::js) struct Config {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) elements: Option<Vec<Element>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) remove_elements: Option<Vec<Name>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) replace_with_children_elements: Option<Vec<Name>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) attributes: Option<Vec<Name>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) remove_attributes: Option<Vec<Name>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) processing_instructions: Option<Vec<Pi>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) remove_processing_instructions: Option<Vec<Pi>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) comments: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) data_attributes: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "javascriptURLs")]
    pub(super) javascript_urls: Option<bool>,
}

pub(super) fn contains(list: &Option<Vec<Name>>, name: &Name) -> bool {
    list.as_ref().is_some_and(|list| list.contains(name))
}

fn duplicates<T: Eq>(items: &[T]) -> bool {
    items
        .iter()
        .enumerate()
        .any(|(index, item)| items[..index].contains(item))
}
fn intersection(a: &[Name], b: &[Name]) -> bool {
    a.iter().any(|item| b.contains(item))
}

impl Config {
    pub fn canonicalize(&mut self, permissive: bool) {
        if self.elements.is_none() && self.remove_elements.is_none() {
            self.remove_elements = Some(Vec::new());
        }
        if self.attributes.is_none() && self.remove_attributes.is_none() {
            self.remove_attributes = Some(Vec::new());
        }
        if self.processing_instructions.is_none() && self.remove_processing_instructions.is_none() {
            if permissive {
                self.remove_processing_instructions = Some(Vec::new());
            } else {
                self.processing_instructions = Some(Vec::new());
            }
        }
        self.comments.get_or_insert(permissive);
        self.javascript_urls.get_or_insert(permissive);
        if self.attributes.is_some() {
            self.data_attributes.get_or_insert(permissive);
        }
        for element in self.elements.iter_mut().flatten() {
            if element.attributes.is_none() && element.remove_attributes.is_none() {
                element.remove_attributes = Some(Vec::new());
            }
        }
    }

    pub fn validate(&self) -> Result<(), &'static str> {
        if self.elements.is_some() == self.remove_elements.is_some()
            || self.attributes.is_some() == self.remove_attributes.is_some()
            || self.processing_instructions.is_some()
                == self.remove_processing_instructions.is_some()
        {
            return Err("Sanitizer allow and remove lists are exclusive");
        }
        for list in [
            &self.remove_elements,
            &self.replace_with_children_elements,
            &self.attributes,
            &self.remove_attributes,
        ] {
            if list.as_ref().is_some_and(|list| duplicates(list)) {
                return Err("Duplicate sanitizer name");
            }
        }
        for list in [
            &self.processing_instructions,
            &self.remove_processing_instructions,
        ] {
            if list.as_ref().is_some_and(|list| duplicates(list)) {
                return Err("Duplicate processing instruction");
            }
        }
        let elements = self.elements.as_deref().unwrap_or_default();
        let keys: Vec<_> = elements.iter().map(|entry| &entry.key).collect();
        if duplicates(&keys) {
            return Err("Duplicate sanitizer element");
        }
        for key in self.replace_with_children_elements.iter().flatten() {
            if key.non_replaceable()
                || contains(&self.remove_elements, key)
                || elements.iter().any(|element| element.key == *key)
            {
                return Err("Invalid replacement element");
            }
        }
        if self.attributes.is_none() && self.data_attributes.is_some() {
            return Err("dataAttributes requires an attribute allow list");
        }
        if self.data_attributes == Some(true) && self.attributes.iter().flatten().any(Name::is_data)
        {
            return Err("Redundant custom data attribute");
        }
        for element in elements {
            self.validate_element(element)?;
        }
        Ok(())
    }

    fn validate_element(&self, element: &Element) -> Result<(), &'static str> {
        let allowed = element.attributes.as_deref().unwrap_or_default();
        let removed = element.remove_attributes.as_deref().unwrap_or_default();
        if duplicates(allowed) || duplicates(removed) {
            return Err("Duplicate local attribute");
        }
        if let Some(global) = &self.attributes {
            if intersection(global, allowed)
                || removed.iter().any(|item| !global.contains(item))
                || (self.data_attributes == Some(true) && allowed.iter().any(Name::is_data))
            {
                return Err("Redundant local attribute configuration");
            }
        } else if let Some(global) = &self.remove_attributes {
            if (element.attributes.is_some() && element.remove_attributes.is_some())
                || intersection(global, allowed)
                || intersection(global, removed)
            {
                return Err("Redundant local attribute configuration");
            }
        }
        Ok(())
    }

    /// get() returns a fresh, deterministic snapshot; private state is unchanged.
    pub fn json(&self) -> String {
        let mut result = self.clone();
        for list in [
            &mut result.remove_elements,
            &mut result.replace_with_children_elements,
            &mut result.attributes,
            &mut result.remove_attributes,
        ] {
            if let Some(list) = list {
                list.sort();
            }
        }
        for list in [
            &mut result.processing_instructions,
            &mut result.remove_processing_instructions,
        ] {
            if let Some(list) = list {
                list.sort_by(|a, b| a.target.encode_utf16().cmp(b.target.encode_utf16()));
            }
        }
        if let Some(elements) = &mut result.elements {
            elements.sort_by(|a, b| a.key.cmp(&b.key));
            for element in elements {
                if let Some(list) = &mut element.attributes {
                    list.sort();
                }
                if let Some(list) = &mut element.remove_attributes {
                    list.sort();
                }
            }
        }
        serde_json::to_string(&result).expect("owned sanitizer configuration")
    }
}
