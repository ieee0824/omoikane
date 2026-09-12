//! HTML tree construction.
//!
//! This is a small HTML5-inspired tree builder that consumes tokenizer output
//! and produces a DOM tree with implicit `html`, `head`, and `body` elements.

use crate::dom::{Node, NodeHandle};

use super::{HtmlParseError, Token, Tokenizer};

/// The current tree-construction insertion mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InsertionMode {
    Initial,
    BeforeHtml,
    BeforeHead,
    InHead,
    InBody,
    InTable,
    /// Collecting column definitions in an explicit or implicit `colgroup`.
    InColumnGroup,
    InTableBody,
    InRow,
    InCell,
    InSelect,
    AfterBody,
    AfterAfterBody,
}

/// Result of HTML tree construction.
#[derive(Debug, Clone)]
pub struct ParseResult {
    document: NodeHandle,
    errors: Vec<HtmlParseError>,
}

/// Result of parsing markup relative to an existing context element.
#[derive(Debug, Clone)]
pub struct FragmentParseResult {
    fragment: NodeHandle,
    errors: Vec<HtmlParseError>,
}

impl FragmentParseResult {
    /// Returns the detached fragment containing the parsed nodes.
    pub fn fragment(&self) -> NodeHandle {
        self.fragment.clone()
    }

    /// Returns recoverable parse errors collected during fragment parsing.
    pub fn errors(&self) -> &[HtmlParseError] {
        &self.errors
    }
}

impl ParseResult {
    /// Returns the parsed document root.
    pub fn document(&self) -> NodeHandle {
        self.document.clone()
    }

    /// Returns recoverable parse errors collected during tokenization/building.
    pub fn errors(&self) -> &[HtmlParseError] {
        &self.errors
    }
}

/// HTML tree builder.
#[derive(Debug, Default)]
pub struct TreeBuilder;

impl TreeBuilder {
    /// Parses an HTML string into a DOM document and collected errors.
    pub fn parse(input: &str) -> ParseResult {
        let (tokens, mut errors) = Tokenizer::new(input).tokenize_with_errors();
        let mut builder = Builder::new();
        builder.process_tokens(tokens, &mut errors);
        ParseResult {
            document: builder.document,
            errors,
        }
    }

    /// Parses HTML relative to `context` and returns only the resulting nodes.
    pub fn parse_fragment(input: &str, context: &NodeHandle) -> FragmentParseResult {
        let context_name = context
            .local_name()
            .or_else(|| context.tag_name())
            .unwrap_or_else(|| "body".to_string());
        let html_context = context
            .namespace_uri()
            .as_deref()
            .is_none_or(|value| value == HTML_NAMESPACE);
        let (tokens, mut errors) = if html_context {
            Tokenizer::new(input).tokenize_fragment_with_errors(&context_name)
        } else {
            Tokenizer::new(input).tokenize_with_errors()
        };
        let (mut builder, container) = Builder::new_fragment(context);
        builder.process_tokens(tokens, &mut errors);
        // Flatten the artificial context element at its position in the
        // synthetic root. Foster-parented nodes can precede it, and a token
        // matching the context's end tag can make later nodes its siblings.
        let root = container.parent_node();
        let source = root.map_or_else(
            || container.template_content().unwrap_or(container.clone()).child_nodes(),
            |root| {
                let mut nodes = Vec::new();
                for child in root.child_nodes() {
                    if child == container {
                        nodes.extend(
                            container
                                .template_content()
                                .unwrap_or(container.clone())
                                .child_nodes(),
                        );
                    } else {
                        nodes.push(child);
                    }
                }
                nodes
            },
        );
        let fragment = NodeHandle::document_fragment();
        for child in source {
            fragment.append_child(child);
        }
        FragmentParseResult { fragment, errors }
    }
}

/// Parser state for one document.write input stream. The same live nodes stay
/// on the open-element stack across writes and script execution.
#[derive(Debug)]
pub(crate) struct WriteParser {
    tokenizer: super::tokenizer::IncrementalTokenizer,
    builder: Builder,
}

impl WriteParser {
    pub(crate) fn new(document: NodeHandle, anchor: Option<NodeHandle>) -> Self {
        let mut builder = Builder::new();
        builder.document = document.clone();
        builder.created_nodes = Some(std::cell::RefCell::new(Vec::new()));
        if let Some(html) = document
            .child_nodes()
            .into_iter()
            .find(|node| node.tag_name().as_deref() == Some("html"))
        {
            let parent = anchor
                .as_ref()
                .and_then(NodeHandle::parent_node)
                .or_else(|| document.query_selector("body"))
                .unwrap_or_else(|| html.clone());
            let reference = anchor.as_ref().and_then(|anchor| {
                let siblings = parent.child_nodes();
                siblings
                    .iter()
                    .position(|node| node == anchor)
                    .and_then(|index| siblings.get(index + 1).cloned())
            });
            let mut ancestors = Vec::new();
            let mut current = Some(parent.clone());
            while let Some(node) = current {
                if node == document {
                    break;
                }
                current = node.parent_node();
                ancestors.push(node);
            }
            ancestors.reverse();
            builder.open_elements = ancestors;
            builder.reset_insertion_mode();
            builder.write_boundary = Some((parent, reference));
        }
        Self {
            tokenizer: super::tokenizer::IncrementalTokenizer::new(),
            builder,
        }
    }

    pub(crate) fn push_input(&mut self, input: &str) {
        self.tokenizer.push_input(input);
    }

    pub(crate) fn take_pending_input(&mut self) -> String {
        self.tokenizer.take_pending_input()
    }

    pub(crate) fn take_created_nodes(&mut self) -> Vec<NodeHandle> {
        std::mem::take(self.builder.created_nodes.as_mut().unwrap().get_mut())
    }

    /// Parse through the next script end tag, leaving later input untouched.
    pub(crate) fn advance(&mut self, eof: bool) -> Option<NodeHandle> {
        let (tokens, mut errors) = self.tokenizer.drain(eof, true);
        let mut script = None;
        for token in tokens {
            if matches!(&token, Token::EndTag { name } if name == "script") {
                script = self.builder.find_open_element("script");
            }
            self.builder.process_token(token, &mut errors);
        }
        script
    }
}

#[derive(Debug)]
struct Builder {
    document: NodeHandle,
    open_elements: Vec<NodeHandle>,
    active_formatting_elements: Vec<NodeHandle>,
    template_insertion_modes: Vec<InsertionMode>,
    mode: InsertionMode,
    write_boundary: Option<(NodeHandle, Option<NodeHandle>)>,
    created_nodes: Option<std::cell::RefCell<Vec<NodeHandle>>>,
    fragment: bool,
}

impl Builder {
    fn new() -> Self {
        Self {
            document: NodeHandle::document(),
            open_elements: Vec::new(),
            active_formatting_elements: Vec::new(),
            template_insertion_modes: Vec::new(),
            mode: InsertionMode::Initial,
            write_boundary: None,
            created_nodes: None,
            fragment: false,
        }
    }

    fn new_fragment(context: &NodeHandle) -> (Self, NodeHandle) {
        let mut builder = Self::new();
        builder.fragment = true;
        let html = NodeHandle::element("html");
        builder.document.append_child(html.clone());

        let context_name = context
            .local_name()
            .or_else(|| context.tag_name())
            .unwrap_or_else(|| "body".to_string());
        let namespace = context.namespace_uri();
        let html_context = namespace.as_deref().is_none_or(|value| value == HTML_NAMESPACE);
        let effective_name = if html_context && context_name.eq_ignore_ascii_case("html") {
            "body".to_string()
        } else {
            context_name
        };
        let container = match namespace {
            Some(namespace) if namespace == HTML_NAMESPACE => {
                NodeHandle::html_element_ns(&effective_name, namespace)
            }
            Some(namespace) => NodeHandle::xml_element(&effective_name, Some(namespace)),
            None => NodeHandle::element(&effective_name),
        };
        if let Some(attributes) = context.attributes() {
            for (name, value) in attributes {
                container.set_attribute(&name, &value);
            }
        }
        html.append_child(container.clone());
        builder.open_elements = vec![html, container.clone()];
        builder.mode = fragment_insertion_mode(&effective_name);
        if effective_name.eq_ignore_ascii_case("template") {
            builder.template_insertion_modes.push(InsertionMode::InBody);
        }
        (builder, container)
    }

    fn process_tokens(&mut self, tokens: Vec<Token>, errors: &mut Vec<HtmlParseError>) {
        for token in tokens {
            self.process_token(token, errors);
        }
    }

    fn process_token(&mut self, token: Token, errors: &mut Vec<HtmlParseError>) {
        if self.process_foreign_token(&token, errors) {
            return;
        }
        match self.mode {
            InsertionMode::Initial => self.handle_initial(token, errors),
            InsertionMode::BeforeHtml => self.handle_before_html(token, errors),
            InsertionMode::BeforeHead => self.handle_before_head(token, errors),
            InsertionMode::InHead => self.handle_in_head(token, errors),
            InsertionMode::InBody => self.handle_in_body(token, errors),
            InsertionMode::InTable => self.handle_in_table(token, errors),
            InsertionMode::InColumnGroup => self.handle_in_column_group(token, errors),
            InsertionMode::InTableBody => self.handle_in_table_body(token, errors),
            InsertionMode::InRow => self.handle_in_row(token, errors),
            InsertionMode::InCell => self.handle_in_cell(token, errors),
            InsertionMode::InSelect => self.handle_in_select(token, errors),
            InsertionMode::AfterBody => self.handle_after_body(token, errors),
            InsertionMode::AfterAfterBody => self.handle_after_after_body(token, errors),
        }
    }

    fn handle_initial(&mut self, token: Token, errors: &mut Vec<HtmlParseError>) {
        match token {
            Token::Comment(data) => self.append_node(&self.document, NodeHandle::comment(data)),
            Token::Doctype(doctype) => {
                if let Some(name) = doctype.name() {
                    self.append_node(
                        &self.document,
                        NodeHandle::document_type(
                            name,
                            doctype.public_id().unwrap_or(""),
                            doctype.system_id().unwrap_or(""),
                        ),
                    );
                }
            }
            Token::Character(data) if data.trim().is_empty() => {}
            Token::Eof => {
                self.ensure_html_element();
                self.ensure_head_element();
                self.ensure_body_element();
            }
            other => {
                self.mode = InsertionMode::BeforeHtml;
                self.process_token(other, errors);
            }
        }
    }

    fn handle_before_html(&mut self, token: Token, errors: &mut Vec<HtmlParseError>) {
        match token {
            Token::Comment(data) => self.append_node(&self.document, NodeHandle::comment(data)),
            Token::Character(data) if data.trim().is_empty() => {}
            Token::StartTag {
                name, attributes, ..
            } if name == "html" => {
                let html = self.insert_html_element_with_attributes("html", &attributes);
                self.open_elements.push(html);
                self.mode = InsertionMode::BeforeHead;
            }
            Token::Eof => {
                self.ensure_html_element();
                self.ensure_head_element();
                self.ensure_body_element();
            }
            other => {
                self.ensure_html_element();
                self.mode = InsertionMode::BeforeHead;
                self.process_token(other, errors);
            }
        }
    }

    fn handle_before_head(&mut self, token: Token, errors: &mut Vec<HtmlParseError>) {
        match token {
            Token::Character(data) if data.trim().is_empty() => {}
            Token::Comment(data) => {
                self.append_node(&self.insertion_parent(), NodeHandle::comment(data))
            }
            Token::StartTag {
                name, attributes, ..
            } if name == "head" => {
                let head = self.insert_element_with_attributes("head", &attributes);
                self.open_elements.push(head);
                self.mode = InsertionMode::InHead;
            }
            Token::Eof => {
                self.ensure_head_element();
                self.ensure_body_element();
            }
            other => {
                self.ensure_head_element();
                self.mode = InsertionMode::InHead;
                self.process_token(other, errors);
            }
        }
    }

    fn handle_in_head(&mut self, token: Token, errors: &mut Vec<HtmlParseError>) {
        match token {
            Token::Character(data) => {
                self.insert_text(&data);
            }
            Token::Comment(data) => {
                self.append_node(&self.insertion_parent(), NodeHandle::comment(data))
            }
            Token::Doctype(_) => {}
            Token::StartTag {
                name,
                attributes,
                self_closing,
            } if matches!(
                name.as_str(),
                "base" | "link" | "meta" | "title" | "style" | "script" | "template"
            ) =>
            {
                let element = self.insert_element_with_attributes(&name, &attributes);
                if name == "template" {
                    self.open_elements.push(element.clone());
                    self.template_insertion_modes.push(self.mode);
                    self.mode = InsertionMode::InBody;
                    if self_closing {
                        self.pop_matching("template");
                        self.mode = InsertionMode::InHead;
                    }
                } else if !self_closing && !is_void_head_tag(&name) {
                    self.open_elements.push(element);
                }
            }
            Token::EndTag { name } if matches!(name.as_str(), "title" | "style" | "script") => {
                self.pop_matching(&name);
            }
            Token::EndTag { name } if name == "head" => {
                self.pop_matching("head");
                self.mode = InsertionMode::InBody;
                self.ensure_body_element();
            }
            Token::EndTag { name } if name == "template" => {
                self.pop_matching("template");
                let restored = self
                    .template_insertion_modes
                    .pop()
                    .unwrap_or(InsertionMode::InHead);
                self.mode = restored;
            }
            other => {
                self.pop_matching("head");
                self.mode = InsertionMode::InBody;
                self.ensure_body_element();
                self.process_token(other, errors);
            }
        }
    }

    fn handle_in_body(&mut self, token: Token, errors: &mut Vec<HtmlParseError>) {
        match token {
            Token::Character(data) => {
                if !data.is_empty() {
                    self.insert_text(&data);
                }
            }
            Token::Comment(data) => {
                self.append_node(&self.insertion_parent(), NodeHandle::comment(data))
            }
            Token::Doctype(_) => {}
            Token::StartTag {
                name,
                attributes,
                self_closing,
            } => {
                if should_close_p_before_start_tag(&name) && self.find_open_element("p").is_some() {
                    self.pop_matching("p");
                }

                match name.as_str() {
                    "html" => {
                        if let Some(html) = self.find_open_element("html") {
                            self.merge_missing_attributes(&html, &attributes);
                        }
                    }
                    "head" => {}
                    "body" => {
                        if !self.fragment {
                            if let Some(body) = self.find_open_element("body") {
                                self.merge_missing_attributes(&body, &attributes);
                            } else {
                                let body = self.insert_element_with_attributes("body", &attributes);
                                self.open_elements.push(body);
                            }
                        }
                    }
                    "svg" | "math" => {
                        let namespace = if name == "svg" {
                            SVG_NAMESPACE
                        } else {
                            MATHML_NAMESPACE
                        };
                        let element = self.insert_foreign_element(
                            &self.insertion_parent(),
                            &name,
                            &attributes,
                            namespace,
                        );
                        if !self_closing {
                            self.open_elements.push(element);
                        }
                    }
                    "table" => {
                        let table = self.insert_element_with_attributes("table", &attributes);
                        if !self_closing {
                            self.open_elements.push(table);
                            self.mode = InsertionMode::InTable;
                        }
                    }
                    "tr" | "td" | "th" => {
                        // A table-scoped start tag encountered directly in body
                        // context: open an implicit `<table>` and reprocess so the
                        // "in table" / "in table body" machinery inserts the
                        // implicit `<tbody>` (and `<tr>` for a stray cell).
                        self.ensure_table_element();
                        self.mode = InsertionMode::InTable;
                        self.process_token(
                            Token::StartTag {
                                name: name.clone(),
                                attributes: attributes.clone(),
                                self_closing,
                            },
                            errors,
                        );
                    }
                    "template" => {
                        let template = self.insert_element_with_attributes("template", &attributes);
                        self.open_elements.push(template);
                        self.template_insertion_modes.push(self.mode);
                    }
                    _ => {
                        let element = self.insert_element_with_attributes(&name, &attributes);
                        if !self_closing && !is_void_element(&name) {
                            if is_formatting_element(&name) {
                                self.active_formatting_elements.push(element.clone());
                            }
                            self.open_elements.push(element);
                        }
                    }
                }
            }
            Token::EndTag { name } => match name.as_str() {
                "body" => {
                    self.pop_matching("body");
                    self.mode = InsertionMode::AfterBody;
                }
                "html" => {
                    self.pop_matching("body");
                    self.pop_matching("html");
                    self.mode = InsertionMode::AfterAfterBody;
                }
                "table" => {
                    self.pop_matching("table");
                    self.reset_insertion_mode();
                }
                "tr" => {
                    self.pop_matching("tr");
                    self.reset_insertion_mode();
                }
                "td" | "th" => {
                    self.pop_matching(&name);
                    self.reset_insertion_mode();
                }
                "template" => {
                    self.pop_matching("template");
                    self.mode = self
                        .template_insertion_modes
                        .pop()
                        .unwrap_or(InsertionMode::InBody);
                }
                _ => {
                    self.pop_until(&name);
                    self.active_formatting_elements
                        .retain(|node| node.tag_name().as_deref() != Some(name.as_str()));
                }
            },
            Token::Eof => {
                self.mode = InsertionMode::AfterAfterBody;
            }
        }

        let _ = errors;
    }

    fn handle_in_table(&mut self, token: Token, errors: &mut Vec<HtmlParseError>) {
        match token {
            Token::Character(data) if data.trim().is_empty() => self.insert_text(&data),
            Token::Character(data) => self.foster_parent_text(&data),
            Token::Comment(data) => {
                self.append_node(&self.insertion_parent(), NodeHandle::comment(data))
            }
            Token::StartTag {
                name,
                attributes,
                self_closing,
            } => match name.as_str() {
                "colgroup" => {
                    self.clear_stack_to_table_context();
                    let table = self
                        .current_table()
                        .unwrap_or_else(|| self.ensure_table_element());
                    let group = self.insert_into(&table, "colgroup", &attributes);
                    self.open_elements.push(group);
                    self.mode = InsertionMode::InColumnGroup;
                }
                "col" => {
                    self.clear_stack_to_table_context();
                    let table = self
                        .current_table()
                        .unwrap_or_else(|| self.ensure_table_element());
                    let group = self.insert_into(&table, "colgroup", &[]);
                    self.open_elements.push(group);
                    self.mode = InsertionMode::InColumnGroup;
                    self.process_token(
                        Token::StartTag {
                            name,
                            attributes,
                            self_closing,
                        },
                        errors,
                    );
                }
                "tbody" | "thead" | "tfoot" => {
                    // Explicit section: insert it under the table and switch to
                    // the "in table body" insertion mode.
                    self.clear_stack_to_table_context();
                    let table = self
                        .current_table()
                        .unwrap_or_else(|| self.ensure_table_element());
                    let section = self.insert_into(&table, &name, &attributes);
                    self.open_elements.push(section);
                    self.mode = InsertionMode::InTableBody;
                }
                "tr" | "td" | "th" => {
                    // No open section: generate an implicit `<tbody>`, then
                    // reprocess the token in the "in table body" mode so a `<tr>`
                    // (or an implicit `<tr>` for a stray cell) is placed inside it.
                    self.clear_stack_to_table_context();
                    let table = self
                        .current_table()
                        .unwrap_or_else(|| self.ensure_table_element());
                    let tbody = self.insert_into(&table, "tbody", &[]);
                    self.open_elements.push(tbody);
                    self.mode = InsertionMode::InTableBody;
                    self.process_token(
                        Token::StartTag {
                            name: name.clone(),
                            attributes: attributes.clone(),
                            self_closing,
                        },
                        errors,
                    );
                }
                "table" => {
                    let table = self.insert_element_with_attributes("table", &attributes);
                    if !self_closing {
                        self.open_elements.push(table);
                    }
                }
                "template" => {
                    let template = self.insert_element_with_attributes("template", &attributes);
                    if !self_closing {
                        self.open_elements.push(template);
                        self.template_insertion_modes.push(self.mode);
                        self.mode = InsertionMode::InBody;
                    }
                }
                _ => self.foster_parent_element(&name, &attributes, self_closing),
            },
            Token::EndTag { name } if name == "table" => {
                self.pop_matching("table");
                self.reset_insertion_mode();
            }
            Token::EndTag { name } if name == "template" => {
                self.pop_matching("template");
                self.mode = self
                    .template_insertion_modes
                    .pop()
                    .unwrap_or(InsertionMode::InTable);
            }
            other => {
                self.mode = InsertionMode::InBody;
                self.process_token(other, errors);
                self.mode = if self.current_table().is_some() {
                    InsertionMode::InTable
                } else {
                    InsertionMode::InBody
                };
            }
        }
    }

    fn handle_in_column_group(&mut self, token: Token, errors: &mut Vec<HtmlParseError>) {
        match token {
            Token::Character(data) if data.trim().is_empty() => self.insert_text(&data),
            Token::Comment(data) => {
                self.append_node(&self.insertion_parent(), NodeHandle::comment(data))
            }
            Token::StartTag {
                name, attributes, ..
            } if name == "col" => {
                self.insert_element_with_attributes("col", &attributes);
            }
            Token::EndTag { name } if name == "col" => {}
            Token::EndTag { name } if name == "colgroup" => {
                if self.current_node().tag_name().as_deref() == Some("colgroup") {
                    self.open_elements.pop();
                    self.mode = InsertionMode::InTable;
                }
            }
            token if matches!(&token, Token::StartTag { name, .. } if name == "html") => {
                self.handle_in_body(token, errors);
            }
            token if matches!(&token, Token::StartTag { name, .. } | Token::EndTag { name } if name == "template") => {
                self.handle_in_table(token, errors)
            }
            Token::Doctype(_) => {}
            Token::Eof => self.handle_in_body(Token::Eof, errors),
            other => {
                if self.current_node().tag_name().as_deref() == Some("colgroup") {
                    self.open_elements.pop();
                    self.mode = InsertionMode::InTable;
                    self.process_token(other, errors);
                }
            }
        }
    }

    /// The HTML "in table body" insertion mode: the current node is a
    /// `<tbody>` / `<thead>` / `<tfoot>` section and we are placing rows.
    fn handle_in_table_body(&mut self, token: Token, errors: &mut Vec<HtmlParseError>) {
        match token {
            Token::StartTag {
                name,
                attributes,
                self_closing,
            } => match name.as_str() {
                "tr" => {
                    self.clear_stack_to_table_body_context();
                    let section = self.current_node();
                    let tr = self.insert_into(&section, "tr", &attributes);
                    self.open_elements.push(tr);
                    self.mode = InsertionMode::InRow;
                }
                "td" | "th" => {
                    // A cell without an open row: generate an implicit `<tr>`,
                    // then reprocess so the cell is placed inside it.
                    self.clear_stack_to_table_body_context();
                    let section = self.current_node();
                    let tr = self.insert_into(&section, "tr", &[]);
                    self.open_elements.push(tr);
                    self.mode = InsertionMode::InRow;
                    self.process_token(
                        Token::StartTag {
                            name: name.clone(),
                            attributes: attributes.clone(),
                            self_closing,
                        },
                        errors,
                    );
                }
                "caption" | "col" | "colgroup" | "tbody" | "tfoot" | "thead" => {
                    // Close the current section and reprocess in "in table".
                    self.clear_stack_to_table_body_context();
                    self.pop_current_section();
                    self.mode = InsertionMode::InTable;
                    self.process_token(
                        Token::StartTag {
                            name: name.clone(),
                            attributes: attributes.clone(),
                            self_closing,
                        },
                        errors,
                    );
                }
                _ => self.handle_in_table(
                    Token::StartTag {
                        name,
                        attributes,
                        self_closing,
                    },
                    errors,
                ),
            },
            Token::EndTag { name } if matches!(name.as_str(), "tbody" | "tfoot" | "thead") => {
                self.clear_stack_to_table_body_context();
                // Per the HTML "in table body" insertion mode, act on the end
                // tag only when an element with the same tag name is in table
                // scope. After clearing to a table body context the current
                // node is the innermost open section, so a name match is the
                // scope check expressed in terms of this stack. A mismatched
                // section end tag (e.g. `</tbody>` while a `<thead>` is open)
                // is a parse error: ignore it, leaving the section open and the
                // insertion mode unchanged.
                if self.current_node().tag_name().as_deref() == Some(name.as_str()) {
                    self.open_elements.pop();
                    self.mode = InsertionMode::InTable;
                }
            }
            Token::EndTag { name } if name == "table" => {
                self.clear_stack_to_table_body_context();
                self.pop_current_section();
                self.mode = InsertionMode::InTable;
                self.process_token(Token::EndTag { name }, errors);
            }
            // Stray end tags that have no effect in this mode.
            Token::EndTag { name }
                if matches!(
                    name.as_str(),
                    "body" | "caption" | "col" | "colgroup" | "html" | "td" | "th" | "tr"
                ) => {}
            other => self.handle_in_table(other, errors),
        }
    }

    fn handle_in_row(&mut self, token: Token, errors: &mut Vec<HtmlParseError>) {
        match token {
            Token::StartTag {
                name,
                attributes,
                self_closing: _,
            } if name == "td" || name == "th" => {
                let row = self.current_node();
                let cell = self.insert_into(&row, &name, &attributes);
                self.open_elements.push(cell);
                self.mode = InsertionMode::InCell;
            }
            Token::EndTag { name } if name == "tr" => {
                self.pop_matching("tr");
                self.reset_insertion_mode();
            }
            Token::EndTag { name } if name == "table" => {
                self.pop_matching("tr");
                self.pop_matching("table");
                self.reset_insertion_mode();
            }
            other => {
                self.mode = InsertionMode::InTable;
                self.process_token(other, errors);
                // Reset mode based on open elements within the current table scope only.
                self.mode = self.insertion_mode_for_current_table_scope();
            }
        }
    }

    fn handle_in_cell(&mut self, token: Token, errors: &mut Vec<HtmlParseError>) {
        match token {
            Token::EndTag { name } if name == "td" || name == "th" => {
                self.pop_matching(&name);
                self.reset_insertion_mode();
            }
            Token::EndTag { name } if name == "tr" => {
                self.pop_matching("td");
                self.pop_matching("th");
                self.pop_matching("tr");
                self.reset_insertion_mode();
            }
            other => {
                self.handle_in_body(other, errors);
                self.reset_insertion_mode();
            }
        }
    }

    fn handle_in_select(&mut self, token: Token, errors: &mut Vec<HtmlParseError>) {
        match token {
            Token::Character(data) => {
                if !data.is_empty() {
                    self.insert_text(&data);
                }
            }
            Token::Comment(data) => {
                self.append_node(&self.insertion_parent(), NodeHandle::comment(data))
            }
            Token::StartTag {
                name,
                attributes,
                self_closing,
            } if name == "option" => {
                if self.current_node().tag_name().as_deref() == Some("option") {
                    self.open_elements.pop();
                }
                let option = self.insert_element_with_attributes("option", &attributes);
                if !self_closing {
                    self.open_elements.push(option);
                }
            }
            Token::StartTag {
                name,
                attributes,
                self_closing,
            } if name == "optgroup" => {
                if self.current_node().tag_name().as_deref() == Some("option") {
                    self.open_elements.pop();
                }
                if self.current_node().tag_name().as_deref() == Some("optgroup") {
                    self.open_elements.pop();
                }
                let group = self.insert_element_with_attributes("optgroup", &attributes);
                if !self_closing {
                    self.open_elements.push(group);
                }
            }
            Token::StartTag { name, .. } if name == "select" => {}
            token
                if matches!(&token, Token::StartTag { name, .. } if name == "script" || name == "template") =>
            {
                self.handle_in_head(token, errors)
            }
            Token::EndTag { name } if name == "option" => {
                if self.current_node().tag_name().as_deref() == Some("option") {
                    self.open_elements.pop();
                }
            }
            Token::EndTag { name } if name == "optgroup" => {
                if self.current_node().tag_name().as_deref() == Some("option") {
                    self.open_elements.pop();
                }
                if self.current_node().tag_name().as_deref() == Some("optgroup") {
                    self.open_elements.pop();
                }
            }
            Token::EndTag { name } if name == "select" => {}
            Token::Doctype(_) => {}
            Token::Eof => self.mode = InsertionMode::AfterAfterBody,
            _ => {}
        }
    }

    fn process_foreign_token(
        &mut self,
        token: &Token,
        errors: &mut Vec<HtmlParseError>,
    ) -> bool {
        let current = self.current_node();
        let Some(namespace) = current.namespace_uri() else {
            return false;
        };
        if namespace == HTML_NAMESPACE {
            return false;
        }

        if let Token::StartTag { name, .. } = token {
            if foreign_allows_html_start(&current, name) {
                return false;
            }
            if is_foreign_breakout_tag(name) {
                let minimum_depth = if self.fragment { 2 } else { 1 };
                while self.open_elements.len() > minimum_depth
                    && self
                        .current_node()
                        .namespace_uri()
                        .is_some_and(|value| value != HTML_NAMESPACE)
                {
                    self.open_elements.pop();
                }
                self.mode = InsertionMode::InBody;
                self.handle_in_body(token.clone(), errors);
                return true;
            }
        }

        match token {
            Token::Character(data) => {
                if !data.is_empty() {
                    self.insert_text(data);
                }
            }
            Token::Comment(data) => {
                self.append_node(&self.insertion_parent(), NodeHandle::comment(data))
            }
            Token::Doctype(_) => {}
            Token::StartTag {
                name,
                attributes,
                self_closing,
            } => {
                let element = self.insert_foreign_element(
                    &self.insertion_parent(),
                    name,
                    attributes,
                    &namespace,
                );
                if !self_closing {
                    self.open_elements.push(element);
                }
            }
            Token::EndTag { name } => {
                if let Some(index) = self.open_elements.iter().rposition(|element| {
                    element
                        .tag_name()
                        .is_some_and(|tag| tag.eq_ignore_ascii_case(name))
                }) {
                    self.open_elements.truncate(index);
                }
            }
            Token::Eof => self.mode = InsertionMode::AfterAfterBody,
        }
        true
    }

    fn insert_foreign_element(
        &self,
        parent: &NodeHandle,
        name: &str,
        attributes: &[super::Attribute],
        namespace: &str,
    ) -> NodeHandle {
        let adjusted_name = if namespace == SVG_NAMESPACE {
            adjust_svg_tag_name(name)
        } else {
            name
        };
        let element = NodeHandle::xml_element(adjusted_name, Some(namespace.to_string()));
        for attribute in attributes {
            element.set_attribute(attribute.name(), attribute.value());
        }
        self.append_node(parent, element.clone());
        element
    }

    fn handle_after_body(&mut self, token: Token, errors: &mut Vec<HtmlParseError>) {
        match token {
            Token::Character(data) if data.trim().is_empty() => {}
            Token::Comment(data) => self.append_node(&self.document, NodeHandle::comment(data)),
            Token::EndTag { name } if name == "html" => {
                self.pop_matching("html");
                self.mode = InsertionMode::AfterAfterBody;
            }
            Token::Eof => self.mode = InsertionMode::AfterAfterBody,
            other => {
                self.mode = InsertionMode::InBody;
                self.process_token(other, errors);
            }
        }
    }

    fn handle_after_after_body(&mut self, token: Token, errors: &mut Vec<HtmlParseError>) {
        match token {
            Token::Comment(data) => self.append_node(&self.document, NodeHandle::comment(data)),
            Token::Character(data) if data.trim().is_empty() => {}
            Token::Eof => {}
            other => {
                self.mode = InsertionMode::InBody;
                self.process_token(other, errors);
            }
        }
    }

    fn ensure_html_element(&mut self) -> NodeHandle {
        if let Some(existing) = self.find_open_element("html") {
            return existing;
        }

        let html = self.insert_html_element("html");
        self.open_elements.push(html.clone());
        html
    }

    fn ensure_head_element(&mut self) -> NodeHandle {
        if let Some(existing) = self.find_open_element("head") {
            return existing;
        }

        let html = self.ensure_html_element();
        let head = self.insert_into(&html, "head", &[]);
        self.open_elements.push(head.clone());
        head
    }

    fn ensure_body_element(&mut self) -> NodeHandle {
        if let Some(existing) = self.find_open_element("body") {
            return existing;
        }

        self.pop_matching("head");
        let html = self.ensure_html_element();
        let body = self.insert_into(&html, "body", &[]);
        self.open_elements.push(body.clone());
        body
    }

    fn ensure_table_element(&mut self) -> NodeHandle {
        if let Some(table) = self.current_table() {
            return table;
        }

        let body = self.ensure_body_element();
        let table = self.insert_into(&body, "table", &[]);
        self.open_elements.push(table.clone());
        table
    }

    /// Pops open elements until the current node is a `<table>`, `<template>`
    /// or `<html>` (HTML "clear the stack back to a table context").
    fn clear_stack_to_table_context(&mut self) {
        while let Some(node) = self.open_elements.last() {
            if matches!(
                node.tag_name().as_deref(),
                Some("table" | "template" | "html")
            ) {
                break;
            }
            self.open_elements.pop();
        }
    }

    /// Pops open elements until the current node is a table section
    /// (`<tbody>`/`<tfoot>`/`<thead>`), `<template>` or `<html>` (HTML "clear
    /// the stack back to a table body context").
    fn clear_stack_to_table_body_context(&mut self) {
        while let Some(node) = self.open_elements.last() {
            if matches!(
                node.tag_name().as_deref(),
                Some("tbody" | "tfoot" | "thead" | "template" | "html")
            ) {
                break;
            }
            self.open_elements.pop();
        }
    }

    /// Pops the current node if it is an open table section element.
    fn pop_current_section(&mut self) {
        if matches!(
            self.current_node().tag_name().as_deref(),
            Some("tbody" | "tfoot" | "thead")
        ) {
            self.open_elements.pop();
        }
    }

    fn insert_html_element(&mut self, name: &str) -> NodeHandle {
        self.insert_html_element_with_attributes(name, &[])
    }

    fn insert_html_element_with_attributes(
        &mut self,
        name: &str,
        attributes: &[super::Attribute],
    ) -> NodeHandle {
        let node = NodeHandle::element(name);
        for attribute in attributes {
            node.set_attribute(attribute.name(), attribute.value());
        }
        self.append_node(&self.document, node.clone());
        node
    }

    fn insert_element_with_attributes(
        &mut self,
        name: &str,
        attributes: &[super::Attribute],
    ) -> NodeHandle {
        let parent = if name == "body" {
            self.ensure_html_element()
        } else {
            self.current_node_or_document()
        };
        self.insert_into(&parent, name, attributes)
    }

    fn insert_into(
        &self,
        parent: &NodeHandle,
        name: &str,
        attributes: &[super::Attribute],
    ) -> NodeHandle {
        let element = NodeHandle::element(name);
        for attribute in attributes {
            element.set_attribute(attribute.name(), attribute.value());
        }
        self.append_node(parent, element.clone());
        element
    }

    fn merge_missing_attributes(&self, element: &NodeHandle, attributes: &[super::Attribute]) {
        let existing = element.attributes().unwrap_or_default();
        for attribute in attributes {
            if !existing.contains_key(attribute.name()) {
                element.set_attribute(attribute.name(), attribute.value());
            }
        }
    }

    fn append_node(&self, parent: &NodeHandle, child: NodeHandle) {
        if let Some(created) = &self.created_nodes {
            created.borrow_mut().push(child.clone());
        }
        if let Some((boundary, Some(reference))) = &self.write_boundary
            && boundary == parent
            && reference.parent_node().as_ref() == Some(parent)
        {
            let _ = parent.insert_before(child, reference);
        } else {
            parent.append_child(child);
        }
    }

    fn insert_text(&mut self, text: &str) {
        let parent = self.current_node_or_document();
        // Writes can split a character run at arbitrary input boundaries.
        // Keep the live Text node when more characters arrive at the same point.
        if self.created_nodes.is_some() {
            let children = parent.child_nodes();
            let index = self
                .write_boundary
                .as_ref()
                .filter(|(boundary, _)| boundary == &parent)
                .and_then(|(_, reference)| reference.as_ref())
                .and_then(|reference| children.iter().position(|node| node == reference))
                .unwrap_or(children.len());
            if let Some(previous) = index.checked_sub(1).and_then(|index| children.get(index))
                && previous.node_type() == crate::dom::NodeType::Text
            {
                previous.set_data(&(previous.data().unwrap_or_default() + text));
                return;
            }
        }
        self.append_node(&parent, NodeHandle::text(text));
    }

    fn foster_parent_text(&mut self, text: &str) {
        if let Some(table) = self.current_table()
            && let Some(parent) = table.parent_node()
        {
            let text_node = NodeHandle::text(text);
            if let Some(created) = &self.created_nodes {
                created.borrow_mut().push(text_node.clone());
            }
            let _ = parent.insert_before(text_node.clone(), &table);
            return;
        }

        self.insert_text(text);
    }

    fn foster_parent_element(
        &mut self,
        name: &str,
        attributes: &[super::Attribute],
        self_closing: bool,
    ) {
        if let Some(table) = self.current_table()
            && let Some(parent) = table.parent_node()
        {
            let element = self.insert_into(&parent, name, attributes);
            let _ = parent.remove_child(&element);
            let _ = parent.insert_before(element.clone(), &table);
            if !self_closing && !is_void_element(name) {
                self.open_elements.push(element);
            }
            return;
        }

        let element = self.insert_element_with_attributes(name, attributes);
        if !self_closing && !is_void_element(name) {
            self.open_elements.push(element);
        }
    }

    fn current_node(&self) -> NodeHandle {
        self.open_elements
            .last()
            .cloned()
            .unwrap_or_else(|| self.document.clone())
    }

    /// Returns the node that receives newly parsed children.
    ///
    /// An HTML `<template>` stays on the stack of open elements for parsing
    /// scope, but its tokens are inserted into the separate template contents
    /// DocumentFragment rather than becoming children of the element.
    fn insertion_parent(&self) -> NodeHandle {
        let current = self.current_node();
        current.template_content().unwrap_or(current)
    }

    fn current_node_or_document(&mut self) -> NodeHandle {
        if self.open_elements.is_empty() {
            self.ensure_body_element()
        } else {
            self.insertion_parent()
        }
    }

    fn pop_matching(&mut self, tag_name: &str) {
        if let Some(index) = self
            .open_elements
            .iter()
            .rposition(|node| node.tag_name().as_deref() == Some(tag_name))
        {
            self.open_elements.truncate(index);
        }
    }

    fn pop_until(&mut self, tag_name: &str) {
        if let Some(index) = self
            .open_elements
            .iter()
            .rposition(|node| node.tag_name().as_deref() == Some(tag_name))
        {
            self.open_elements.truncate(index);
        }
    }

    fn find_open_element(&self, tag_name: &str) -> Option<NodeHandle> {
        self.open_elements
            .iter()
            .rev()
            .find(|node| node.tag_name().as_deref() == Some(tag_name))
            .cloned()
    }

    fn current_table(&self) -> Option<NodeHandle> {
        self.find_open_element("table")
    }

    fn reset_insertion_mode(&mut self) {
        self.mode = self
            .open_elements
            .iter()
            .rev()
            .find_map(|node| match node.tag_name().as_deref() {
                Some("td" | "th") => Some(InsertionMode::InCell),
                Some("tr") => Some(InsertionMode::InRow),
                Some("tbody" | "thead" | "tfoot") => Some(InsertionMode::InTableBody),
                Some("colgroup") => Some(InsertionMode::InColumnGroup),
                Some("table") => Some(InsertionMode::InTable),
                Some("select" | "optgroup" | "option") => Some(InsertionMode::InSelect),
                Some("body") => Some(InsertionMode::InBody),
                Some("head") => Some(InsertionMode::InHead),
                Some("html") => Some(InsertionMode::BeforeHead),
                _ => None,
            })
            .unwrap_or(InsertionMode::Initial);
    }

    /// Determine the insertion mode by scanning open elements only within
    /// the innermost table scope. This prevents an outer table's td/tr
    /// from affecting mode decisions inside a nested table.
    fn insertion_mode_for_current_table_scope(&self) -> InsertionMode {
        for node in self.open_elements.iter().rev() {
            match node.tag_name().as_deref() {
                Some("td" | "th") => return InsertionMode::InCell,
                Some("tr") => return InsertionMode::InRow,
                Some("tbody" | "thead" | "tfoot") => return InsertionMode::InTableBody,
                Some("colgroup") => return InsertionMode::InColumnGroup,
                Some("table") => return InsertionMode::InTable,
                _ => continue,
            }
        }
        InsertionMode::InBody
    }
}

const HTML_NAMESPACE: &str = "http://www.w3.org/1999/xhtml";
const SVG_NAMESPACE: &str = "http://www.w3.org/2000/svg";
const MATHML_NAMESPACE: &str = "http://www.w3.org/1998/Math/MathML";

fn fragment_insertion_mode(context_name: &str) -> InsertionMode {
    match context_name.to_ascii_lowercase().as_str() {
        "head" => InsertionMode::InHead,
        "table" => InsertionMode::InTable,
        "tbody" | "thead" | "tfoot" => InsertionMode::InTableBody,
        "colgroup" => InsertionMode::InColumnGroup,
        "tr" => InsertionMode::InRow,
        "td" | "th" => InsertionMode::InCell,
        "select" | "optgroup" | "option" => InsertionMode::InSelect,
        _ => InsertionMode::InBody,
    }
}

fn foreign_allows_html_start(current: &NodeHandle, token_name: &str) -> bool {
    let local_name = current.local_name().unwrap_or_default();
    match current.namespace_uri().as_deref() {
        Some(SVG_NAMESPACE) => matches!(
            local_name.to_ascii_lowercase().as_str(),
            "foreignobject" | "desc" | "title"
        ),
        Some(MATHML_NAMESPACE) => {
            let math_text_integration = matches!(
                local_name.to_ascii_lowercase().as_str(),
                "mi" | "mo" | "mn" | "ms" | "mtext"
            ) && !matches!(token_name, "mglyph" | "malignmark");
            let annotation_html = local_name.eq_ignore_ascii_case("annotation-xml")
                && current.get_attribute("encoding").is_some_and(|encoding| {
                    encoding.eq_ignore_ascii_case("text/html")
                        || encoding.eq_ignore_ascii_case("application/xhtml+xml")
                });
            math_text_integration || annotation_html
        }
        _ => false,
    }
}

fn is_foreign_breakout_tag(name: &str) -> bool {
    matches!(
        name,
        "b" | "big" | "blockquote" | "body" | "br" | "center" | "code" | "dd"
            | "div" | "dl" | "dt" | "em" | "embed" | "h1" | "h2" | "h3" | "h4"
            | "h5" | "h6" | "head" | "hr" | "i" | "img" | "li" | "listing"
            | "menu" | "meta" | "nobr" | "ol" | "p" | "pre" | "ruby" | "s"
            | "small" | "span" | "strong" | "strike" | "sub" | "sup" | "table"
            | "tt" | "u" | "ul" | "var"
    )
}

fn adjust_svg_tag_name(name: &str) -> &str {
    match name {
        "altglyph" => "altGlyph",
        "altglyphdef" => "altGlyphDef",
        "altglyphitem" => "altGlyphItem",
        "animatecolor" => "animateColor",
        "animatemotion" => "animateMotion",
        "animatetransform" => "animateTransform",
        "clippath" => "clipPath",
        "feblend" => "feBlend",
        "fecolormatrix" => "feColorMatrix",
        "fecomponenttransfer" => "feComponentTransfer",
        "fecomposite" => "feComposite",
        "feconvolvematrix" => "feConvolveMatrix",
        "fediffuselighting" => "feDiffuseLighting",
        "fedisplacementmap" => "feDisplacementMap",
        "fedistantlight" => "feDistantLight",
        "fedropshadow" => "feDropShadow",
        "feflood" => "feFlood",
        "fefunca" => "feFuncA",
        "fefuncb" => "feFuncB",
        "fefuncg" => "feFuncG",
        "fefuncr" => "feFuncR",
        "fegaussianblur" => "feGaussianBlur",
        "feimage" => "feImage",
        "femerge" => "feMerge",
        "femergenode" => "feMergeNode",
        "femorphology" => "feMorphology",
        "feoffset" => "feOffset",
        "fepointlight" => "fePointLight",
        "fespecularlighting" => "feSpecularLighting",
        "fespotlight" => "feSpotLight",
        "fetile" => "feTile",
        "feturbulence" => "feTurbulence",
        "foreignobject" => "foreignObject",
        "glyphref" => "glyphRef",
        "lineargradient" => "linearGradient",
        "radialgradient" => "radialGradient",
        "textpath" => "textPath",
        _ => name,
    }
}

fn is_void_element(tag_name: &str) -> bool {
    matches!(
        tag_name,
        "area"
            | "base"
            | "br"
            | "col"
            | "embed"
            | "hr"
            | "img"
            | "input"
            | "link"
            | "meta"
            | "source"
            | "track"
            | "wbr"
    )
}

fn is_void_head_tag(tag_name: &str) -> bool {
    matches!(tag_name, "base" | "link" | "meta")
}

fn is_formatting_element(tag_name: &str) -> bool {
    matches!(
        tag_name,
        "a" | "b" | "em" | "i" | "small" | "span" | "strong" | "u"
    )
}

fn should_close_p_before_start_tag(tag_name: &str) -> bool {
    matches!(
        tag_name,
        "address"
            | "article"
            | "aside"
            | "blockquote"
            | "div"
            | "dl"
            | "fieldset"
            | "footer"
            | "form"
            | "h1"
            | "h2"
            | "h3"
            | "h4"
            | "h5"
            | "h6"
            | "header"
            | "hgroup"
            | "hr"
            | "main"
            | "nav"
            | "ol"
            | "p"
            | "pre"
            | "section"
            | "table"
            | "ul"
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn table_columns_stay_in_explicit_or_implicit_column_groups() {
        for columns in [
            "<colgroup><col id='a' style='width:100px'><col id='b'></colgroup>",
            "<col id='a' style='width:100px'><col id='b'>",
            "<colgroup> \n<!--columns--><col id='a' style='width:100px'><col id='b'>",
        ] {
            let document = super::TreeBuilder::parse(&format!(
                "<table>{columns}<tr><td>Cell</td></tr></table>"
            ))
            .document();
            let table = document.query_selector("table").unwrap();
            let group = table.query_selector("colgroup").unwrap();
            assert_eq!(group.parent_node().unwrap(), table);
            for id in ["a", "b"] {
                let column = document.query_selector(&format!("#{id}")).unwrap();
                assert_eq!(column.parent_node().unwrap(), group);
            }
            let cell = document.query_selector("td").unwrap();
            assert_eq!(
                cell.parent_node()
                    .unwrap()
                    .parent_node()
                    .unwrap()
                    .parent_node()
                    .unwrap(),
                table
            );
        }
    }

    use crate::dom::Node;

    use super::*;

    #[test]
    fn inserts_implicit_html_head_and_body() {
        let result = TreeBuilder::parse("<p>Hello</p>");
        let document = result.document();

        let html = document.query_selector("html").unwrap();
        let head = document.query_selector("head").unwrap();
        let body = document.query_selector("body").unwrap();
        let p = document.query_selector("p").unwrap();

        assert_eq!(html.parent_node(), Some(document.clone()));
        assert_eq!(head.parent_node(), Some(html.clone()));
        assert_eq!(body.parent_node(), Some(html));
        assert_eq!(p.parent_node(), Some(body));
        assert!(result.errors().is_empty());
    }

    #[test]
    fn preserves_body_attributes_when_body_was_inserted_implicitly() {
        let result =
            TreeBuilder::parse("<meta charset=\"utf-8\"><body bgcolor=\"#f0f0ff\"></body>");
        let body = result.document().query_selector("body").unwrap();
        let attrs = body.attributes().unwrap_or_default();
        assert_eq!(
            attrs.get("bgcolor").map(|value| value.as_str()),
            Some("#f0f0ff")
        );
    }

    #[test]
    fn preserves_explicit_head_attributes() {
        let result = TreeBuilder::parse("<html><head id='head' data-kind='primary'></head></html>");
        let head = result.document().query_selector("head").unwrap();
        let attrs = head.attributes().unwrap_or_default();
        assert_eq!(attrs.get("id").map(String::as_str), Some("head"));
        assert_eq!(
            attrs.get("data-kind").map(String::as_str),
            Some("primary")
        );
    }

    #[test]
    fn places_doctype_and_comment_under_document() {
        let result = TreeBuilder::parse("<!DOCTYPE html><!--note--><html><body></body></html>");
        let children = result.document().child_nodes();

        assert_eq!(children[0].node_name(), "html");
        assert_eq!(children[1].node_name(), "#comment");
        assert_eq!(children[2].node_name(), "HTML");
    }

    #[test]
    fn full_parse_builds_doctype_and_head_body_split() {
        // Acid3 test 71's first write: a public-id doctype followed by a title
        // (which belongs in <head>) and body-level <span>/<script>.
        let result = TreeBuilder::parse(
            "<!DOCTYPE HTML PUBLIC \"-//W3C//DTD HTML 4.0 Transitional//EN\"><title></title><span></span><script type=\"text/javascript\"></script>",
        );
        let document = result.document();

        // The document's children are exactly [doctype, html].
        let children = document.child_nodes();
        assert_eq!(children.len(), 2, "document children = [doctype, html]");
        assert_eq!(children[0].node_type(), crate::dom::NodeType::DocumentType);
        assert_eq!(children[1].tag_name().as_deref(), Some("html"));

        // The doctype carries the (lowercased) name and its public identifier;
        // no system identifier was given.
        let doctype = &children[0];
        assert_eq!(doctype.node_name(), "html");
        assert_eq!(
            doctype.public_id().as_deref(),
            Some("-//W3C//DTD HTML 4.0 Transitional//EN")
        );
        assert_eq!(doctype.system_id(), None);

        // <html> holds <head> then <body>; <title> is in <head>; <span> and
        // <script> land in <body> in document order.
        let html = &children[1];
        let html_children: Vec<String> = html
            .child_nodes()
            .iter()
            .filter_map(|n| n.tag_name())
            .collect();
        assert_eq!(html_children, vec!["head".to_string(), "body".to_string()]);

        let head = document.query_selector("head").unwrap();
        let head_children: Vec<String> = head
            .child_nodes()
            .iter()
            .filter_map(|n| n.tag_name())
            .collect();
        assert_eq!(head_children, vec!["title".to_string()]);

        let body = document.query_selector("body").unwrap();
        let body_children: Vec<String> = body
            .child_nodes()
            .iter()
            .filter_map(|n| n.tag_name())
            .collect();
        assert_eq!(
            body_children,
            vec!["span".to_string(), "script".to_string()]
        );
    }

    #[test]
    fn full_parse_reads_public_and_system_doctype_identifiers() {
        // Acid3 test 71's second write: a PUBLIC + SYSTEM doctype, and a
        // <script> nested inside the <span> (so <body> has a single child).
        let result = TreeBuilder::parse(
            "<!DOCTYPE HTML PUBLIC \"-//W3C//DTD HTML 4.01 Transitional//EN\" \"http://www.w3.org/TR/html4/loose.dtd\"><title></title><span><script type=\"text/javascript\"></script></span>",
        );
        let document = result.document();
        let doctype = &document.child_nodes()[0];
        assert_eq!(
            doctype.public_id().as_deref(),
            Some("-//W3C//DTD HTML 4.01 Transitional//EN")
        );
        assert_eq!(
            doctype.system_id().as_deref(),
            Some("http://www.w3.org/TR/html4/loose.dtd")
        );

        let body = document.query_selector("body").unwrap();
        let body_children: Vec<String> = body
            .child_nodes()
            .iter()
            .filter_map(|n| n.tag_name())
            .collect();
        assert_eq!(body_children, vec!["span".to_string()]);
        let span = document.query_selector("span").unwrap();
        let span_children: Vec<String> = span
            .child_nodes()
            .iter()
            .filter_map(|n| n.tag_name())
            .collect();
        assert_eq!(span_children, vec!["script".to_string()]);
    }

    #[test]
    fn builds_text_inside_body() {
        let result = TreeBuilder::parse("<html><body>Hello <b>world</b></body></html>");
        let body = result.document().query_selector("body").unwrap();
        let children = body.child_nodes();

        assert_eq!(children[0].node_name(), "#text");
        assert_eq!(children[0].data(), Some("Hello ".to_string()));
        assert_eq!(children[1].node_name(), "B");
        assert_eq!(
            children[1].child_nodes()[0].data(),
            Some("world".to_string())
        );
    }

    #[test]
    fn keeps_title_text_inside_head() {
        let result = TreeBuilder::parse(
            "<html><head><title>The Second Acid Test</title></head><body><p>visible</p></body></html>",
        );
        let document = result.document();
        let head = document.query_selector("head").unwrap();
        let title = document.query_selector("title").unwrap();
        let body = document.query_selector("body").unwrap();

        assert_eq!(title.parent_node(), Some(head));
        assert_eq!(
            title.child_nodes()[0].data(),
            Some("The Second Acid Test".to_string())
        );
        assert_eq!(body.child_nodes()[0].tag_name().as_deref(), Some("p"));
    }

    #[test]
    fn table_modes_create_rows_and_cells() {
        let result = TreeBuilder::parse("<table><tr><td>A</td><td>B</td></tr></table>");
        let table = result.document().query_selector("table").unwrap();
        // A `<tr>` directly under `<table>` is placed inside an implicitly
        // generated `<tbody>` per the HTML "in table" insertion mode.
        let tbody = table.child_nodes()[0].clone();
        assert_eq!(tbody.tag_name().as_deref(), Some("tbody"));
        let row = tbody.child_nodes()[0].clone();
        let first_cell = row.child_nodes()[0].clone();
        let second_cell = row.child_nodes()[1].clone();

        assert_eq!(row.tag_name().as_deref(), Some("tr"));
        assert_eq!(first_cell.tag_name().as_deref(), Some("td"));
        assert_eq!(first_cell.child_nodes()[0].data(), Some("A".to_string()));
        assert_eq!(second_cell.child_nodes()[0].data(), Some("B".to_string()));
    }

    #[test]
    fn implicit_tbody_wraps_direct_row() {
        let result = TreeBuilder::parse("<table><tr><td>x</td></tr></table>");
        let table = result.document().query_selector("table").unwrap();
        let children = table.child_nodes();
        assert_eq!(children.len(), 1, "table should contain exactly one tbody");

        let tbody = children[0].clone();
        assert_eq!(tbody.tag_name().as_deref(), Some("tbody"));
        assert_eq!(tbody.parent_node(), Some(table));

        let tr = tbody.child_nodes()[0].clone();
        assert_eq!(tr.tag_name().as_deref(), Some("tr"));
        assert_eq!(tr.parent_node(), Some(tbody));

        let td = tr.child_nodes()[0].clone();
        assert_eq!(td.tag_name().as_deref(), Some("td"));
        assert_eq!(td.child_nodes()[0].data(), Some("x".to_string()));
    }

    #[test]
    fn implicit_tbody_and_row_for_direct_cell() {
        // `<table><td>` with no intervening `<tr>` generates BOTH a `<tbody>`
        // and a `<tr>` (HTML "in table" then "in table body" insertion modes).
        let result = TreeBuilder::parse("<table><td>x</td></table>");
        let table = result.document().query_selector("table").unwrap();
        let tbody = table.child_nodes()[0].clone();
        assert_eq!(tbody.tag_name().as_deref(), Some("tbody"));

        let tr = tbody.child_nodes()[0].clone();
        assert_eq!(tr.tag_name().as_deref(), Some("tr"));

        let td = tr.child_nodes()[0].clone();
        assert_eq!(td.tag_name().as_deref(), Some("td"));
        assert_eq!(td.child_nodes()[0].data(), Some("x".to_string()));
    }

    #[test]
    fn explicit_table_sections_are_not_double_wrapped() {
        let html = "<table>\
            <thead><tr><td>h</td></tr></thead>\
            <tbody><tr><td>b</td></tr></tbody>\
            <tfoot><tr><td>f</td></tr></tfoot>\
            </table>";
        let result = TreeBuilder::parse(html);
        let table = result.document().query_selector("table").unwrap();

        let sections: Vec<String> = table
            .child_nodes()
            .into_iter()
            .filter_map(|node| node.tag_name())
            .collect();
        assert_eq!(
            sections,
            vec![
                "thead".to_string(),
                "tbody".to_string(),
                "tfoot".to_string()
            ],
            "explicit sections must be preserved in order without extra wrappers"
        );

        // Each explicit section directly holds its `<tr>` (no implicit tbody).
        for section in table.child_nodes() {
            let tr = section.child_nodes()[0].clone();
            assert_eq!(
                tr.tag_name().as_deref(),
                Some("tr"),
                "section {:?} must hold its row directly",
                section.tag_name()
            );
        }

        // Exactly one <tbody> (the explicit one) and one <tr> per section.
        assert_eq!(count_elements(&table, "tbody"), 1);
        assert_eq!(count_elements(&table, "tr"), 3);
    }

    #[test]
    fn mismatched_section_end_tag_is_ignored_in_table_body() {
        // `</tbody>` while a `<thead>` is the open section does not match any
        // element in table scope, so per the HTML "in table body" insertion
        // mode it is a parse error and must be ignored. The `<thead>` therefore
        // stays open and the following row lands inside it (matching Chrome /
        // Firefox), rather than the section being wrongly closed and the row
        // dropped into a spurious implicit `<tbody>`.
        let result = TreeBuilder::parse("<table><thead></tbody><tr><td>x</table>");
        let table = result.document().query_selector("table").unwrap();

        // The section is not double-created: exactly one <thead>, no <tbody>.
        assert_eq!(count_elements(&table, "thead"), 1);
        assert_eq!(count_elements(&table, "tbody"), 0);

        let children = table.child_nodes();
        assert_eq!(
            children.len(),
            1,
            "the ignored </tbody> must not create a sibling section"
        );
        let thead = children[0].clone();
        assert_eq!(thead.tag_name().as_deref(), Some("thead"));

        let tr = thead.child_nodes()[0].clone();
        assert_eq!(tr.tag_name().as_deref(), Some("tr"));
        let td = tr.child_nodes()[0].clone();
        assert_eq!(td.tag_name().as_deref(), Some("td"));
        assert_eq!(td.child_nodes()[0].data(), Some("x".to_string()));
    }

    #[test]
    fn matching_section_end_tag_closes_section_in_table_body() {
        // Regression guard for the mismatch fix above: a *matching* section end
        // tag must still close the section. `</thead>` closes the open (empty)
        // `<thead>`, so the subsequent row is placed in a fresh implicit
        // `<tbody>` sibling.
        let result = TreeBuilder::parse("<table><thead></thead><tr><td>x</table>");
        let table = result.document().query_selector("table").unwrap();

        let sections: Vec<String> = table
            .child_nodes()
            .into_iter()
            .filter_map(|node| node.tag_name())
            .collect();
        assert_eq!(sections, vec!["thead".to_string(), "tbody".to_string()]);

        let thead = table.child_nodes()[0].clone();
        assert_eq!(
            thead.child_nodes().len(),
            0,
            "the closed <thead> must stay empty"
        );

        let tbody = table.child_nodes()[1].clone();
        let tr = tbody.child_nodes()[0].clone();
        assert_eq!(tr.tag_name().as_deref(), Some("tr"));
        let td = tr.child_nodes()[0].clone();
        assert_eq!(td.child_nodes()[0].data(), Some("x".to_string()));
    }

    #[test]
    fn whitespace_after_implicit_section_close_stays_in_table() {
        // Mirrors the exact Acid3 fragment `<table><tr><td><p></tbody> </table>`:
        // after `</tbody>` closes the cell/row, the trailing whitespace text
        // must land in the `<table>` (not inside the cell), and the `<p>` stays
        // empty. This is the parser precondition for Acid3 test 29.
        let result = TreeBuilder::parse("<table><tr><td><p></tbody> </table>");
        let table = result.document().query_selector("table").unwrap();
        let children = table.child_nodes();
        assert_eq!(
            children.len(),
            2,
            "table must have a <tbody> plus a trailing whitespace text node"
        );
        assert_eq!(children[0].tag_name().as_deref(), Some("tbody"));
        assert_eq!(children[1].node_name(), "#text");
        assert_eq!(children[1].data(), Some(" ".to_string()));

        let p = table.query_selector("p").unwrap();
        assert_eq!(p.child_nodes().len(), 0, "the <p> must remain empty");
        let td = p.parent_node().unwrap();
        assert_eq!(td.tag_name().as_deref(), Some("td"));
        let tr = td.parent_node().unwrap();
        assert_eq!(tr.tag_name().as_deref(), Some("tr"));
        let tbody = tr.parent_node().unwrap();
        assert_eq!(tbody.tag_name().as_deref(), Some("tbody"));
    }

    #[test]
    fn foster_parents_text_before_table() {
        let result = TreeBuilder::parse("<body><table>hello<tr><td>x</td></tr></table></body>");
        let body = result.document().query_selector("body").unwrap();
        let children = body.child_nodes();

        assert_eq!(children[0].node_name(), "#text");
        assert_eq!(children[0].data(), Some("hello".to_string()));
        assert_eq!(children[1].tag_name().as_deref(), Some("table"));
    }

    #[test]
    fn nested_table_close_keeps_following_rows_in_outer_table() {
        let html = "<table><tr><td rowspan='2'><table><tr><td>a</td><td>b</tr></table></td></tr><tr><td>c</td><td>d</td></tr></table>";
        let result = TreeBuilder::parse(html);
        let table = result.document().query_selector("table").unwrap();
        // The outer table's rows now live under an implicit `<tbody>`.
        let tbody = table
            .child_nodes()
            .into_iter()
            .find(|node| node.tag_name().as_deref() == Some("tbody"))
            .expect("outer table has an implicit tbody");
        let rows: Vec<_> = tbody
            .child_nodes()
            .into_iter()
            .filter(|node| node.tag_name().as_deref() == Some("tr"))
            .collect();

        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].child_nodes().len(), 1);
        assert_eq!(rows[1].child_nodes().len(), 2);
        assert_eq!(
            rows[1].child_nodes()[0].child_nodes()[0].data(),
            Some("c".to_string())
        );
        assert_eq!(
            rows[1].child_nodes()[1].child_nodes()[0].data(),
            Some("d".to_string())
        );
    }

    #[test]
    fn template_content_is_inserted_and_closed() {
        let result = TreeBuilder::parse("<template><div>inside</div></template><p>after</p>");
        let template = result.document().query_selector("template").unwrap();
        let content = template.template_content().unwrap();
        let div = content.query_selector("div").unwrap();
        let p = result.document().query_selector("p").unwrap();

        assert!(template.child_nodes().is_empty());
        assert!(result.document().query_selector("div").is_none());
        assert_eq!(div.child_nodes()[0].data(), Some("inside".to_string()));
        assert_eq!(p.child_nodes()[0].data(), Some("after".to_string()));
    }

    #[test]
    fn fragment_parsing_uses_table_and_select_contexts() {
        let table = NodeHandle::element("table");
        let table_fragment =
            TreeBuilder::parse_fragment("<tr><td>cell</td></tr>", &table).fragment();
        let tbody = table_fragment.child_nodes().into_iter().next().unwrap();
        assert_eq!(tbody.tag_name().as_deref(), Some("tbody"));
        let row = tbody.child_nodes().into_iter().next().unwrap();
        assert_eq!(row.tag_name().as_deref(), Some("tr"));
        assert_eq!(row.child_nodes()[0].tag_name().as_deref(), Some("td"));

        let select = NodeHandle::element("select");
        let select_fragment =
            TreeBuilder::parse_fragment("<option>one<option>two<div>ignored", &select).fragment();
        let options = select_fragment.child_nodes();
        assert_eq!(options.len(), 2);
        assert!(options
            .iter()
            .all(|option| option.tag_name().as_deref() == Some("option")));
    }

    #[test]
    fn fragment_parsing_preserves_foreign_namespaces_and_html_integration() {
        let svg = NodeHandle::xml_element("svg", Some(SVG_NAMESPACE.to_string()));
        let fragment = TreeBuilder::parse_fragment(
            "<circle><title>x</title></circle><foreignObject><div>html</div></foreignObject>",
            &svg,
        )
        .fragment();
        let children = fragment.child_nodes();
        assert_eq!(children[0].namespace_uri().as_deref(), Some(SVG_NAMESPACE));
        assert_eq!(children[1].tag_name().as_deref(), Some("foreignObject"));
        assert_eq!(children[1].namespace_uri().as_deref(), Some(SVG_NAMESPACE));
        assert_eq!(children[1].child_nodes()[0].namespace_uri(), None);

        let math = NodeHandle::xml_element("math", Some(MATHML_NAMESPACE.to_string()));
        let fragment = TreeBuilder::parse_fragment("<mi><span>html</span></mi>", &math).fragment();
        let mi = &fragment.child_nodes()[0];
        assert_eq!(mi.namespace_uri().as_deref(), Some(MATHML_NAMESPACE));
        assert_eq!(mi.child_nodes()[0].namespace_uri(), None);
    }

    #[test]
    fn fragment_parsing_places_template_nodes_in_the_returned_fragment() {
        let template = NodeHandle::element("template");
        let fragment =
            TreeBuilder::parse_fragment("text<!--marker--><span>inside</span>", &template)
                .fragment();
        let children = fragment.child_nodes();
        assert_eq!(children.len(), 3);
        assert_eq!(children[0].data().as_deref(), Some("text"));
        assert_eq!(children[1].node_type(), crate::dom::NodeType::Comment);
        assert_eq!(children[2].tag_name().as_deref(), Some("span"));
    }

    #[test]
    fn fragment_tokenizer_starts_in_the_context_content_model() {
        let textarea = NodeHandle::element("textarea");
        let fragment = TreeBuilder::parse_fragment(
            "a<b>&amp;</textarea><i>end</i>",
            &textarea,
        )
        .fragment();
        let children = fragment.child_nodes();
        assert_eq!(children[0].data().as_deref(), Some("a<b>&"));
        assert_eq!(children[1].tag_name().as_deref(), Some("i"));

        let script = NodeHandle::element("script");
        let fragment = TreeBuilder::parse_fragment("if (a < b) c();", &script).fragment();
        assert_eq!(fragment.child_nodes()[0].data().as_deref(), Some("if (a < b) c();"));

        let plaintext = NodeHandle::element("plaintext");
        let fragment = TreeBuilder::parse_fragment("<b>&amp;</b>", &plaintext).fragment();
        assert_eq!(fragment.child_nodes()[0].data().as_deref(), Some("<b>&amp;</b>"));
    }

    #[test]
    fn closes_paragraph_before_block_elements_in_body() {
        let result = TreeBuilder::parse(
            "<div class=\"picture\"><p><table><tr><td></table><p class=\"bad\"><div class=\"forehead\"></div></div>",
        );
        let document = result.document();
        let bad = find_by_class(&document, "bad").unwrap();
        let forehead = find_by_class(&document, "forehead").unwrap();

        assert_ne!(forehead.parent_node(), Some(bad));
        assert_eq!(
            forehead.parent_node().and_then(|node| node.tag_name()),
            Some("div".to_string())
        );
    }

    fn find_by_class(node: &NodeHandle, class: &str) -> Option<NodeHandle> {
        if node
            .attributes()
            .and_then(|attributes| attributes.get("class").cloned())
            .map(|value| value.split_whitespace().any(|candidate| candidate == class))
            .unwrap_or(false)
        {
            return Some(node.clone());
        }

        for child in node.child_nodes() {
            if let Some(found) = find_by_class(&child, class) {
                return Some(found);
            }
        }

        None
    }

    /// Counts every element in the tree whose tag name equals `tag`.
    fn count_elements(node: &NodeHandle, tag: &str) -> usize {
        let mut count = if node.tag_name().as_deref() == Some(tag) {
            1
        } else {
            0
        };
        for child in node.child_nodes() {
            count += count_elements(&child, tag);
        }
        count
    }

    #[test]
    fn script_with_angle_brackets_is_a_single_element_with_full_text() {
        let result =
            TreeBuilder::parse("<body><script>if (a < b) { doc('</p>'); }</script></body>");
        let document = result.document();

        assert_eq!(count_elements(&document, "script"), 1);
        // The `<` and the string literal `</p>` must not spawn extra elements.
        assert_eq!(count_elements(&document, "p"), 0);

        let script = document.query_selector("script").unwrap();
        let children = script.child_nodes();
        assert_eq!(children.len(), 1);
        assert_eq!(
            children[0].data(),
            Some("if (a < b) { doc('</p>'); }".to_string())
        );
    }

    #[test]
    fn textarea_content_stays_text_and_does_not_build_elements() {
        let result = TreeBuilder::parse("<body><textarea><div>x</div></textarea></body>");
        let document = result.document();

        assert_eq!(count_elements(&document, "textarea"), 1);
        assert_eq!(count_elements(&document, "div"), 0);

        let textarea = document.query_selector("textarea").unwrap();
        assert_eq!(
            textarea.child_nodes()[0].data(),
            Some("<div>x</div>".to_string())
        );
    }

    #[test]
    fn style_content_with_angle_brackets_is_verbatim_text() {
        let result =
            TreeBuilder::parse("<head><style>a > b { content: '</style>fake'; }</style></head>");
        let document = result.document();

        assert_eq!(count_elements(&document, "style"), 1);
        let style = document.query_selector("style").unwrap();
        // `a > b { content: '` precedes the first `</style>` which closes the
        // element (RAWTEXT has no string awareness), so the text stops there.
        assert_eq!(
            style.child_nodes()[0].data(),
            Some("a > b { content: '".to_string())
        );
    }

    #[test]
    fn acid3_fixture_parses_to_ten_script_elements() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/acid3/acid3.html"
        );
        let html = std::fs::read_to_string(path).expect("read acid3.html fixture");
        let document = TreeBuilder::parse(&html).document();

        // The real Acid3 page has exactly 10 <script> elements (4 inline in the
        // head, 5 external data: scripts, 1 inline in the body). Before the
        // RAWTEXT/script-data states existed, the `<` characters inside inline
        // JS split the body script into extra spurious elements.
        assert_eq!(
            count_elements(&document, "script"),
            10,
            "acid3.html must tokenize into exactly 10 script elements"
        );
    }
}
