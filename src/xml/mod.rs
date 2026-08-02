//! A small, strict XML 1.0 parser used for XML sub-documents.

use std::collections::HashMap;
use std::fmt;

use crate::dom::NodeHandle;

const XML_NS: &str = "http://www.w3.org/XML/1998/namespace";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XmlParseError { message: String }

impl XmlParseError {
    fn new(message: impl Into<String>) -> Self { Self { message: message.into() } }
}
impl fmt::Display for XmlParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str(&self.message) }
}
impl std::error::Error for XmlParseError {}

pub fn parse(bytes: &[u8]) -> Result<NodeHandle, XmlParseError> {
    let input = std::str::from_utf8(bytes)
        .map_err(|_| XmlParseError::new("XML input is not valid UTF-8"))?;
    if input.chars().any(|ch| !is_xml_char(ch)) {
        return Err(XmlParseError::new("invalid XML character"));
    }
    Parser::new(input).parse_document()
}

struct OpenElement {
    name: String,
    node: NodeHandle,
    namespaces: HashMap<String, String>,
}

struct Parser<'a> { input: &'a str, pos: usize, document: NodeHandle, stack: Vec<OpenElement> }

impl<'a> Parser<'a> {
    fn new(input: &'a str) -> Self {
        Self { input, pos: 0, document: NodeHandle::document(), stack: Vec::new() }
    }

    fn parse_document(mut self) -> Result<NodeHandle, XmlParseError> {
        if self.input.starts_with('\u{feff}') { self.pos += '\u{feff}'.len_utf8(); }
        if self.starts("<?xml") {
            let declaration = self.take_until("?>")?;
            if let Some(enc) = declaration_encoding(declaration)
                && !enc.eq_ignore_ascii_case("utf-8") && !enc.eq_ignore_ascii_case("utf8") {
                    return Err(XmlParseError::new("XML declaration encoding conflicts with UTF-8 input"));
                }
        }
        let mut root_seen = false;
        while self.pos < self.input.len() {
            if self.starts("<!--") { self.parse_comment()?; }
            else if self.starts("<?") { self.parse_pi()?; }
            else if self.starts("<!DOCTYPE") { self.parse_doctype(root_seen)?; }
            else if self.starts("<![CDATA[") { self.parse_cdata()?; }
            else if self.starts("</") { self.parse_end()?; }
            else if self.starts("<") {
                if self.stack.is_empty() {
                    if root_seen { return self.err("multiple document elements"); }
                    root_seen = true;
                }
                self.parse_start()?;
            } else { self.parse_text()?; }
        }
        if !self.stack.is_empty() { return self.err("unclosed element"); }
        if !root_seen { return self.err("missing document element"); }
        Ok(self.document)
    }

    fn parse_start(&mut self) -> Result<(), XmlParseError> {
        self.pos += 1;
        let name = self.name()?;
        let mut attributes = Vec::new();
        let mut namespaces = self.stack.last().map(|e| e.namespaces.clone()).unwrap_or_default();
        namespaces.insert("xml".into(), XML_NS.into());
        loop {
            self.ws();
            if self.consume("/>") || self.consume(">") { break; }
            let attr_name = self.name()?;
            if attributes.iter().any(|(n, _): &(String, String)| n == &attr_name) {
                return self.err("duplicate attribute");
            }
            self.ws();
            if !self.consume("=") { return self.err("attribute value must follow '='"); }
            self.ws();
            let quote = self.peek().ok_or_else(|| XmlParseError::new("missing attribute value"))?;
            if quote != '\'' && quote != '"' { return self.err("XML attribute values must be quoted"); }
            self.pos += quote.len_utf8();
            let raw = self.take_char_until(quote)?;
            let value = decode_references(raw)?;
            if attr_name == "xmlns" { namespaces.insert(String::new(), value.clone()); }
            else if let Some(prefix) = attr_name.strip_prefix("xmlns:") {
                if prefix.is_empty() { return self.err("empty namespace prefix"); }
                if prefix == "xmlns" { return self.err("the xmlns prefix is reserved"); }
                if prefix == "xml" && value != XML_NS {
                    return self.err("the xml prefix must bind to the XML namespace");
                }
                namespaces.insert(prefix.to_string(), value.clone());
            }
            attributes.push((attr_name, value));
        }
        let empty = self.input[..self.pos].ends_with("/>");
        let prefix = name.split_once(':').map(|(p, _)| p).unwrap_or("");
        let namespace = namespaces.get(prefix).cloned();
        if !prefix.is_empty() && namespace.is_none() { return self.err("undeclared namespace prefix"); }
        let node = NodeHandle::xml_element(&name, namespace);
        for (attr, value) in attributes { node.set_xml_attribute(attr, value); }
        self.parent().append_child(node.clone());
        if !empty { self.stack.push(OpenElement { name, node, namespaces }); }
        Ok(())
    }

    fn parse_end(&mut self) -> Result<(), XmlParseError> {
        self.pos += 2;
        let name = self.name()?;
        self.ws();
        if !self.consume(">") { return self.err("malformed end tag"); }
        let open = self.stack.pop().ok_or_else(|| XmlParseError::new("unexpected end tag"))?;
        if open.name != name { return self.err("mismatched end tag"); }
        Ok(())
    }

    fn parse_text(&mut self) -> Result<(), XmlParseError> {
        let end = self.input[self.pos..].find('<').map(|n| self.pos + n).unwrap_or(self.input.len());
        let raw = &self.input[self.pos..end]; self.pos = end;
        let text = decode_references(raw)?;
        if self.stack.is_empty() {
            if !text.trim().is_empty() { return self.err("text outside document element"); }
        } else if !text.is_empty() { self.parent().append_child(NodeHandle::text(text)); }
        Ok(())
    }

    fn parse_comment(&mut self) -> Result<(), XmlParseError> {
        self.pos += 4;
        let body = self.take_until("-->")?;
        if body.contains("--") || body.ends_with('-') {
            return self.err("'--' and a trailing '-' are forbidden in XML comments");
        }
        self.parent().append_child(NodeHandle::comment(body)); Ok(())
    }
    fn parse_pi(&mut self) -> Result<(), XmlParseError> {
        self.pos += 2;
        let target = self.name()?;
        if target.eq_ignore_ascii_case("xml") {
            return self.err("the processing-instruction target 'xml' is reserved");
        }
        let data = if self.starts("?>") {
            self.pos += 2;
            String::new()
        } else {
            if !self.peek().is_some_and(char::is_whitespace) {
                return self.err("processing-instruction data must follow whitespace");
            }
            self.ws();
            self.take_until("?>")?.to_string()
        };
        self.parent()
            .append_child(NodeHandle::processing_instruction(target, data));
        Ok(())
    }
    fn parse_cdata(&mut self) -> Result<(), XmlParseError> {
        if self.stack.is_empty() { return self.err("CDATA outside document element"); }
        self.pos += 9; let text = self.take_until("]]>")?.to_string();
        self.parent().append_child(NodeHandle::text(text)); Ok(())
    }
    fn parse_doctype(&mut self, root_seen: bool) -> Result<(), XmlParseError> {
        if root_seen || !self.stack.is_empty() { return self.err("misplaced doctype"); }
        self.pos += 9; self.ws(); let name = self.name()?; self.ws();
        let mut public_id = String::new(); let mut system_id = String::new();
        if self.consume("PUBLIC") { self.ws(); public_id = self.quoted_literal()?; self.ws(); system_id = self.quoted_literal()?; self.ws(); }
        else if self.consume("SYSTEM") { self.ws(); system_id = self.quoted_literal()?; self.ws(); }
        if self.peek() == Some('[') { return self.err("internal subsets are unsupported"); }
        if !self.consume(">") { return self.err("malformed doctype"); }
        self.document.append_child(NodeHandle::document_type(name, public_id, system_id)); Ok(())
    }

    fn quoted_literal(&mut self) -> Result<String, XmlParseError> {
        let q = self.peek().ok_or_else(|| XmlParseError::new("missing quoted literal"))?;
        if q != '\'' && q != '"' { return self.err("doctype identifier must be quoted"); }
        self.pos += 1; Ok(self.take_char_until(q)?.to_string())
    }
    fn parent(&self) -> NodeHandle { self.stack.last().map(|e| e.node.clone()).unwrap_or_else(|| self.document.clone()) }
    fn name(&mut self) -> Result<String, XmlParseError> {
        let start = self.pos;
        while let Some(ch) = self.peek() { if is_name_char(ch, self.pos == start) { self.pos += ch.len_utf8(); } else { break; } }
        if self.pos == start { return self.err("expected XML name"); }
        let name = &self.input[start..self.pos];
        if name.matches(':').count() > 1 || name.starts_with(':') || name.ends_with(':') { return self.err("malformed qualified name"); }
        Ok(name.to_string())
    }
    fn ws(&mut self) { while self.peek().is_some_and(char::is_whitespace) { self.pos += self.peek().unwrap().len_utf8(); } }
    fn peek(&self) -> Option<char> { self.input[self.pos..].chars().next() }
    fn starts(&self, s: &str) -> bool { self.input[self.pos..].starts_with(s) }
    fn consume(&mut self, s: &str) -> bool { if self.starts(s) { self.pos += s.len(); true } else { false } }
    fn take_until(&mut self, end: &str) -> Result<&'a str, XmlParseError> {
        let start = self.pos; let offset = self.input[start..].find(end).ok_or_else(|| XmlParseError::new("unterminated XML construct"))?;
        self.pos = start + offset + end.len(); Ok(&self.input[start..start + offset])
    }
    fn take_char_until(&mut self, end: char) -> Result<&'a str, XmlParseError> {
        let start = self.pos; let offset = self.input[start..].find(end).ok_or_else(|| XmlParseError::new("unterminated quoted value"))?;
        self.pos = start + offset + end.len_utf8(); Ok(&self.input[start..start + offset])
    }
    fn err<T>(&self, msg: &str) -> Result<T, XmlParseError> { Err(XmlParseError::new(format!("{msg} at byte {}", self.pos))) }
}

fn declaration_encoding(decl: &str) -> Option<&str> {
    let pos = decl.find("encoding")?; let rest = decl[pos + 8..].trim_start(); let rest = rest.strip_prefix('=')?.trim_start();
    let q = rest.chars().next()?; if q != '\'' && q != '"' { return None; } rest[1..].split(q).next()
}
fn is_name_char(ch: char, first: bool) -> bool {
    ch == ':' || ch == '_' || ch.is_alphabetic() || (!first && (ch.is_ascii_digit() || matches!(ch, '-' | '.') || ch == '\u{b7}'))
}
fn is_xml_char(ch: char) -> bool {
    matches!(ch as u32, 0x9 | 0xa | 0xd | 0x20..=0xd7ff | 0xe000..=0xfffd | 0x10000..=0x10ffff)
}
fn decode_references(input: &str) -> Result<String, XmlParseError> {
    let mut out = String::new(); let mut rest = input;
    while let Some(pos) = rest.find('&') {
        out.push_str(&rest[..pos]); rest = &rest[pos + 1..];
        let end = rest.find(';').ok_or_else(|| XmlParseError::new("unterminated entity reference"))?;
        let entity = &rest[..end]; rest = &rest[end + 1..];
        let value = match entity {
            "lt" => '<', "gt" => '>', "amp" => '&', "quot" => '"', "apos" => '\'',
            value if value.starts_with("#x") => char::from_u32(u32::from_str_radix(&value[2..], 16).map_err(|_| XmlParseError::new("invalid numeric character reference"))?).ok_or_else(|| XmlParseError::new("invalid XML character"))?,
            value if value.starts_with('#') => char::from_u32(value[1..].parse().map_err(|_| XmlParseError::new("invalid numeric character reference"))?).ok_or_else(|| XmlParseError::new("invalid XML character"))?,
            _ => return Err(XmlParseError::new("undefined entity reference")),
        };
        if !is_xml_char(value) { return Err(XmlParseError::new("invalid XML character")); }
        out.push(value);
    }
    out.push_str(rest); Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*; use crate::dom::{Node, NodeType};
    fn root(doc: &NodeHandle) -> NodeHandle { doc.child_nodes().into_iter().find(|n| n.node_type() == NodeType::Element).unwrap() }
    #[test] fn parses_namespaces_entities_cdata_and_preserves_case() {
        let doc = parse(br#"<?xml version='1.0'?><Root xmlns='urn:d' xmlns:p='urn:p' A='&lt;&amp;&#65;&#x42;'><!--ok--><p:Child><![CDATA[<raw>]]></p:Child></Root>"#).unwrap();
        let root = root(&doc); assert_eq!(root.tag_name().as_deref(), Some("Root")); assert_eq!(root.namespace_uri().as_deref(), Some("urn:d")); assert_eq!(root.attributes().unwrap().get("A").map(String::as_str), Some("<&AB"));
        let child = root.child_nodes().into_iter().find(|n| n.node_type() == NodeType::Element).unwrap(); assert_eq!(child.tag_name().as_deref(), Some("p:Child")); assert_eq!(child.namespace_uri().as_deref(), Some("urn:p")); assert_eq!(child.local_name().as_deref(), Some("Child"));
    }
    #[test] fn accepts_doctype_comment_pi_and_defined_entities() { assert!(parse(br#"<?x ok?><!DOCTYPE R SYSTEM 'urn:x'><!--c--><R>&gt;&quot;&apos;</R>"#).is_ok()); }
    #[test] fn rejects_processing_instruction_data_without_whitespace() {
        assert!(parse(br#"<r><?x,data?></r>"#).is_err());
        assert!(parse(br#"<r><?x ok?></r>"#).is_ok());
        assert!(parse(br#"<r><?x?></r>"#).is_ok());
    }
    #[test] fn rejects_mismatched_tags_unquoted_attributes_and_unknown_entities() { for xml in ["<a></b>", "<a x=y/>", "<a>&bogus;</a>"] { assert!(parse(xml.as_bytes()).is_err(), "expected error for {xml}"); } }
    #[test] fn rejects_invalid_utf8_and_non_utf8_declaration() { assert!(parse(b"<r>\xff</r>").is_err()); assert!(parse(br#"<?xml version='1.0' encoding='ISO-8859-1'?><r/>"#).is_err()); }
    #[test] fn rejects_xhtml_crossed_tags_as_whole_document_error() { assert!(parse(br#"<html><p><strong/>x</strong></p></html>"#).is_err()); }
    #[test] fn rejects_invalid_reserved_namespace_prefix_bindings() {
        for xml in [
            "<r xmlns:xml='urn:not-xml'/>",
            "<r xmlns:xmlns='http://www.w3.org/2000/xmlns/'/>",
        ] {
            assert!(parse(xml.as_bytes()).is_err(), "expected fatal error for {xml}");
        }
        assert!(parse(format!("<r xmlns:xml='{XML_NS}'/>").as_bytes()).is_ok(),
            "the predefined xml prefix may bind to the XML namespace");
    }
    #[test] fn rejects_reserved_xml_processing_instruction_target_case_insensitively() {
        for xml in ["<r><?xml bad?></r>", "<r><?XmL bad?></r>"] {
            assert!(parse(xml.as_bytes()).is_err(), "expected fatal error for {xml}");
        }
        assert!(parse(b"<r><?xml-stylesheet ok?></r>").is_ok(),
            "a target merely starting with xml remains valid");
    }
    #[test] fn rejects_double_hyphen_and_trailing_hyphen_in_comments() {
        for xml in ["<r><!--a--b--></r>", "<r><!--a---></r>"] {
            assert!(parse(xml.as_bytes()).is_err(), "expected fatal error for {xml}");
        }
        assert!(parse(b"<r><!--a-b--></r>").is_ok(), "a single interior hyphen is valid");
    }
    #[test] fn rejects_xml_1_0_forbidden_characters_directly_and_by_reference() {
        for xml in [
            "<r>\u{1}</r>",
            "<r>\u{b}</r>",
            "<r>\u{fffe}</r>",
            "<r>\u{ffff}</r>",
            "<r>&#1;</r>",
            "<r>&#xB;</r>",
            "<r>&#xFFFE;</r>",
            "<r>&#65535;</r>",
        ] {
            assert!(parse(xml.as_bytes()).is_err(), "expected invalid XML character error for {xml:?}");
        }
        assert!(parse("<r>\t\n\r\u{20}\u{d7ff}\u{e000}\u{fffd}\u{10000}</r>".as_bytes()).is_ok(),
            "XML 1.0 boundary characters must remain accepted");
    }
}
