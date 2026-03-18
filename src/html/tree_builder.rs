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
    InRow,
    InCell,
    AfterBody,
    AfterAfterBody,
}

/// Result of HTML tree construction.
#[derive(Debug, Clone)]
pub struct ParseResult {
    document: NodeHandle,
    errors: Vec<HtmlParseError>,
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
}

#[derive(Debug)]
struct Builder {
    document: NodeHandle,
    open_elements: Vec<NodeHandle>,
    active_formatting_elements: Vec<NodeHandle>,
    template_insertion_modes: Vec<InsertionMode>,
    mode: InsertionMode,
}

impl Builder {
    fn new() -> Self {
        Self {
            document: NodeHandle::document(),
            open_elements: Vec::new(),
            active_formatting_elements: Vec::new(),
            template_insertion_modes: Vec::new(),
            mode: InsertionMode::Initial,
        }
    }

    fn process_tokens(&mut self, tokens: Vec<Token>, errors: &mut Vec<HtmlParseError>) {
        for token in tokens {
            self.process_token(token, errors);
        }
    }

    fn process_token(&mut self, token: Token, errors: &mut Vec<HtmlParseError>) {
        match self.mode {
            InsertionMode::Initial => self.handle_initial(token, errors),
            InsertionMode::BeforeHtml => self.handle_before_html(token, errors),
            InsertionMode::BeforeHead => self.handle_before_head(token, errors),
            InsertionMode::InHead => self.handle_in_head(token, errors),
            InsertionMode::InBody => self.handle_in_body(token, errors),
            InsertionMode::InTable => self.handle_in_table(token, errors),
            InsertionMode::InRow => self.handle_in_row(token, errors),
            InsertionMode::InCell => self.handle_in_cell(token, errors),
            InsertionMode::AfterBody => self.handle_after_body(token, errors),
            InsertionMode::AfterAfterBody => self.handle_after_after_body(token, errors),
        }
    }

    fn handle_initial(&mut self, token: Token, errors: &mut Vec<HtmlParseError>) {
        match token {
            Token::Comment(data) => self.document.append_child(NodeHandle::comment(data)),
            Token::Doctype(doctype) => {
                if let Some(name) = doctype.name() {
                    self.document.append_child(NodeHandle::document_type(name));
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
            Token::Comment(data) => self.document.append_child(NodeHandle::comment(data)),
            Token::Character(data) if data.trim().is_empty() => {}
            Token::StartTag { name, .. } if name == "html" => {
                let html = self.insert_html_element("html");
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
            Token::Comment(data) => self.current_node().append_child(NodeHandle::comment(data)),
            Token::StartTag { name, .. } if name == "head" => {
                let head = self.insert_element("head");
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
            Token::Character(data) if data.trim().is_empty() => {
                self.insert_text(&data);
            }
            Token::Comment(data) => self.current_node().append_child(NodeHandle::comment(data)),
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
                    self.pop_matching(&name);
                }
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
            Token::Comment(data) => self.current_node().append_child(NodeHandle::comment(data)),
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
                "html" => {}
                "head" => {}
                "body" => {
                    if self.find_open_element("body").is_none() {
                        let body = self.insert_element_with_attributes("body", &attributes);
                        self.open_elements.push(body);
                    }
                }
                "table" => {
                    let table = self.insert_element_with_attributes("table", &attributes);
                    if !self_closing {
                        self.open_elements.push(table);
                        self.mode = InsertionMode::InTable;
                    }
                }
                "tr" => {
                    let table = self.ensure_table_element();
                    let tr = self.insert_into(&table, "tr", &attributes);
                    self.open_elements.push(tr);
                    self.mode = InsertionMode::InRow;
                }
                "td" | "th" => {
                    let table = self.ensure_table_element();
                    let tr = self.ensure_table_row(&table);
                    let cell = self.insert_into(&tr, &name, &attributes);
                    self.open_elements.push(cell);
                    self.mode = InsertionMode::InCell;
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
                    self.mode = InsertionMode::InBody;
                }
                "tr" => {
                    self.pop_matching("tr");
                    self.mode = InsertionMode::InTable;
                }
                "td" | "th" => {
                    self.pop_matching(&name);
                    self.mode = InsertionMode::InRow;
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
            Token::Comment(data) => self.current_node().append_child(NodeHandle::comment(data)),
            Token::StartTag {
                name,
                attributes,
                self_closing,
            } => match name.as_str() {
                "tr" => {
                    let table = self
                        .current_table()
                        .unwrap_or_else(|| self.ensure_table_element());
                    let tr = self.insert_into(&table, "tr", &attributes);
                    self.open_elements.push(tr);
                    self.mode = InsertionMode::InRow;
                }
                "td" | "th" => {
                    let table = self
                        .current_table()
                        .unwrap_or_else(|| self.ensure_table_element());
                    let tr = self.ensure_table_row(&table);
                    let cell = self.insert_into(&tr, &name, &attributes);
                    self.open_elements.push(cell);
                    self.mode = InsertionMode::InCell;
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
                self.mode = InsertionMode::InBody;
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
                self.mode = InsertionMode::InTable;
            }
            Token::EndTag { name } if name == "table" => {
                self.pop_matching("tr");
                self.pop_matching("table");
                self.mode = InsertionMode::InBody;
            }
            other => {
                self.mode = InsertionMode::InTable;
                self.process_token(other, errors);
                self.mode = if self.find_open_element("td").is_some()
                    || self.find_open_element("th").is_some()
                {
                    InsertionMode::InCell
                } else if self.find_open_element("tr").is_some() {
                    InsertionMode::InRow
                } else if self.current_table().is_some() {
                    InsertionMode::InTable
                } else {
                    InsertionMode::InBody
                };
            }
        }
    }

    fn handle_in_cell(&mut self, token: Token, errors: &mut Vec<HtmlParseError>) {
        match token {
            Token::EndTag { name } if name == "td" || name == "th" => {
                self.pop_matching(&name);
                self.mode = InsertionMode::InRow;
            }
            Token::EndTag { name } if name == "tr" => {
                self.pop_matching("td");
                self.pop_matching("th");
                self.pop_matching("tr");
                self.mode = InsertionMode::InTable;
            }
            other => self.handle_in_body(other, errors),
        }
    }

    fn handle_after_body(&mut self, token: Token, errors: &mut Vec<HtmlParseError>) {
        match token {
            Token::Character(data) if data.trim().is_empty() => {}
            Token::Comment(data) => self.document.append_child(NodeHandle::comment(data)),
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
            Token::Comment(data) => self.document.append_child(NodeHandle::comment(data)),
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

    fn ensure_table_row(&mut self, table: &NodeHandle) -> NodeHandle {
        if let Some(row) = self.find_open_element("tr") {
            return row;
        }

        let row = self.insert_into(table, "tr", &[]);
        self.open_elements.push(row.clone());
        row
    }

    fn insert_html_element(&mut self, name: &str) -> NodeHandle {
        let node = NodeHandle::element(name);
        self.document.append_child(node.clone());
        node
    }

    fn insert_element(&mut self, name: &str) -> NodeHandle {
        self.insert_element_with_attributes(name, &[])
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
        parent.append_child(element.clone());
        element
    }

    fn insert_text(&mut self, text: &str) {
        let parent = self.current_node_or_document();
        parent.append_child(NodeHandle::text(text));
    }

    fn foster_parent_text(&mut self, text: &str) {
        if let Some(table) = self.current_table() {
            if let Some(parent) = table.parent_node() {
                let text_node = NodeHandle::text(text);
                let _ = parent.insert_before(text_node.clone(), &table);
                return;
            }
        }

        self.insert_text(text);
    }

    fn foster_parent_element(
        &mut self,
        name: &str,
        attributes: &[super::Attribute],
        self_closing: bool,
    ) {
        if let Some(table) = self.current_table() {
            if let Some(parent) = table.parent_node() {
                let element = self.insert_into(&parent, name, attributes);
                let _ = parent.remove_child(&element);
                let _ = parent.insert_before(element.clone(), &table);
                if !self_closing && !is_void_element(name) {
                    self.open_elements.push(element);
                }
                return;
            }
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

    fn current_node_or_document(&mut self) -> NodeHandle {
        if self.open_elements.is_empty() {
            self.ensure_body_element()
        } else {
            self.current_node()
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
    fn places_doctype_and_comment_under_document() {
        let result = TreeBuilder::parse("<!DOCTYPE html><!--note--><html><body></body></html>");
        let children = result.document().child_nodes();

        assert_eq!(children[0].node_name(), "html");
        assert_eq!(children[1].node_name(), "#comment");
        assert_eq!(children[2].node_name(), "HTML");
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
    fn table_modes_create_rows_and_cells() {
        let result = TreeBuilder::parse("<table><tr><td>A</td><td>B</td></tr></table>");
        let table = result.document().query_selector("table").unwrap();
        let row = table.child_nodes()[0].clone();
        let first_cell = row.child_nodes()[0].clone();
        let second_cell = row.child_nodes()[1].clone();

        assert_eq!(row.tag_name().as_deref(), Some("tr"));
        assert_eq!(first_cell.tag_name().as_deref(), Some("td"));
        assert_eq!(first_cell.child_nodes()[0].data(), Some("A".to_string()));
        assert_eq!(second_cell.child_nodes()[0].data(), Some("B".to_string()));
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
    fn template_content_is_inserted_and_closed() {
        let result = TreeBuilder::parse("<template><div>inside</div></template><p>after</p>");
        let template = result.document().query_selector("template").unwrap();
        let div = template.query_selector("div").unwrap();
        let p = result.document().query_selector("p").unwrap();

        assert_eq!(div.child_nodes()[0].data(), Some("inside".to_string()));
        assert_eq!(p.child_nodes()[0].data(), Some("after".to_string()));
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
}
