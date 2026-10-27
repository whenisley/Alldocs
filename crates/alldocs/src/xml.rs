//! A small, fast DOM built on quick-xml.
//!
//! Office formats are easiest to convert with random access to the tree, and
//! a compact DOM of `document.xml` is cheap compared to unzipping it. The
//! parser is deliberately forgiving: mismatched end tags are tolerated and a
//! parse error yields everything read so far instead of failing.

use quick_xml::escape::resolve_html5_entity;
use quick_xml::events::{BytesStart, Event};
use quick_xml::reader::Reader;

#[derive(Debug, Clone)]
pub(crate) struct Element {
    /// Qualified name as written, e.g. `w:p`.
    pub name: String,
    pub attrs: Vec<(String, String)>,
    pub children: Vec<Node>,
}

#[derive(Debug, Clone)]
pub(crate) enum Node {
    Element(Element),
    Text(String),
}

impl Element {
    fn new(name: String, attrs: Vec<(String, String)>) -> Self {
        Element { name, attrs, children: Vec::new() }
    }

    /// Name without namespace prefix.
    pub fn local(&self) -> &str {
        local_name(&self.name)
    }

    /// Attribute by qualified name (`r:id`).
    pub fn attr(&self, qname: &str) -> Option<&str> {
        self.attrs.iter().find(|(k, _)| k == qname).map(|(_, v)| v.as_str())
    }

    /// Attribute by local name, ignoring the prefix.
    pub fn attr_local(&self, local: &str) -> Option<&str> {
        self.attrs.iter().find(|(k, _)| local_name(k) == local).map(|(_, v)| v.as_str())
    }

    /// Child elements in document order.
    pub fn elements(&self) -> impl Iterator<Item = &Element> {
        self.children.iter().filter_map(|n| match n {
            Node::Element(e) => Some(e),
            Node::Text(_) => None,
        })
    }

    pub fn child(&self, qname: &str) -> Option<&Element> {
        self.elements().find(|e| e.name == qname)
    }

    pub fn children_named<'a>(&'a self, qname: &'a str) -> impl Iterator<Item = &'a Element> + 'a {
        self.elements().filter(move |e| e.name == qname)
    }

    /// First descendant (depth-first, excluding self) with the given name.
    pub fn find(&self, qname: &str) -> Option<&Element> {
        for e in self.elements() {
            if e.name == qname {
                return Some(e);
            }
            if let Some(found) = e.find(qname) {
                return Some(found);
            }
        }
        None
    }

    /// All descendants with the given name, in document order.
    pub fn find_all<'a>(&'a self, qname: &str, out: &mut Vec<&'a Element>) {
        for e in self.elements() {
            if e.name == qname {
                out.push(e);
            }
            e.find_all(qname, out);
        }
    }

    /// First descendant whose local name matches.
    pub fn find_local(&self, local: &str) -> Option<&Element> {
        for e in self.elements() {
            if e.local() == local {
                return Some(e);
            }
            if let Some(found) = e.find_local(local) {
                return Some(found);
            }
        }
        None
    }

    /// Concatenated text of all descendant text nodes.
    pub fn text(&self) -> String {
        let mut out = String::new();
        self.push_text(&mut out);
        out
    }

    fn push_text(&self, out: &mut String) {
        for node in &self.children {
            match node {
                Node::Text(t) => out.push_str(t),
                Node::Element(e) => e.push_text(out),
            }
        }
    }

    /// Path-style lookup through direct children: `root.path(&["a", "b"])`.
    pub fn path(&self, names: &[&str]) -> Option<&Element> {
        let mut cur = self;
        for name in names {
            cur = cur.child(name)?;
        }
        Some(cur)
    }
}

pub(crate) fn local_name(qname: &str) -> &str {
    qname.rsplit_once(':').map_or(qname, |(_, l)| l)
}

const HTML_VOID: &[&str] =
    &["area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param", "source", "track", "wbr"];

/// Parses XML (or XHTML when `html` is set) into a synthetic `#document` root.
pub(crate) fn parse(xml: &str, html: bool) -> Element {
    let mut reader = Reader::from_str(xml);
    {
        let config = reader.config_mut();
        config.check_end_names = false;
        config.allow_unmatched_ends = true;
        config.check_comments = false;
        config.expand_empty_elements = false;
    }

    let mut stack: Vec<Element> = vec![Element::new("#document".into(), Vec::new())];
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                let el = start_element(&e, html);
                if html && HTML_VOID.contains(&el.local()) {
                    push_node(&mut stack, Node::Element(el));
                } else {
                    stack.push(el);
                }
            }
            Ok(Event::Empty(e)) => {
                let el = start_element(&e, html);
                push_node(&mut stack, Node::Element(el));
            }
            Ok(Event::End(e)) => {
                let name = e.name();
                let name = if html { name.0.to_ascii_lowercase() } else { name.0.to_string() };
                if let Some(pos) = stack.iter().rposition(|el| el.name == name)
                    && pos > 0
                {
                    while stack.len() > pos {
                        close_top(&mut stack);
                    }
                }
            }
            Ok(Event::Text(t)) => push_text(&mut stack, &t.xml10_content()),
            Ok(Event::CData(t)) => push_text(&mut stack, &t.into_inner()),
            Ok(Event::GeneralRef(r)) => {
                if r.is_char_ref() {
                    if let Ok(Some(ch)) = r.resolve_char_ref() {
                        push_text(&mut stack, ch.encode_utf8(&mut [0; 4]));
                    }
                } else {
                    let name = r.into_inner();
                    match resolve_html5_entity(&name) {
                        Some(value) => push_text(&mut stack, value),
                        None => push_text(&mut stack, &format!("&{name};")),
                    }
                }
            }
            Ok(Event::Eof) | Err(_) => break,
            Ok(_) => {}
        }
    }
    while stack.len() > 1 {
        close_top(&mut stack);
    }
    stack.pop().expect("root element")
}

fn start_element(e: &BytesStart<'_>, html: bool) -> Element {
    let raw_name = e.name().0;
    let name = if html { raw_name.to_ascii_lowercase() } else { raw_name.to_string() };
    let mut attrs = Vec::new();
    // HTML allows valueless (`checked`) and unquoted attributes.
    let mut iter = if html { e.html_attributes() } else { e.attributes() };
    for attr in iter.with_checks(false).flatten() {
        let key = attr.key.0;
        let key = if html { key.to_ascii_lowercase() } else { key.to_string() };
        let value = quick_xml::escape::unescape_with(&attr.value, resolve_html5_entity)
            .map(|v| v.into_owned())
            .unwrap_or_else(|_| attr.value.to_string());
        attrs.push((key, value));
    }
    Element::new(name, attrs)
}

fn push_node(stack: &mut [Element], node: Node) {
    stack.last_mut().expect("root element").children.push(node);
}

fn push_text(stack: &mut [Element], text: &str) {
    if text.is_empty() {
        return;
    }
    let top = stack.last_mut().expect("root element");
    if let Some(Node::Text(prev)) = top.children.last_mut() {
        prev.push_str(text);
    } else {
        top.children.push(Node::Text(text.to_string()));
    }
}

fn close_top(stack: &mut Vec<Element>) {
    let el = stack.pop().expect("non-root element");
    push_node(stack, Node::Element(el));
}

/// Parses raw bytes (decoding them first) into a DOM.
pub(crate) fn parse_bytes(data: &[u8], html: bool) -> Element {
    parse(&crate::text::decode_markup(data), html)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_entities_and_tolerates_html() {
        let root = parse("<p>a &amp; b&nbsp;c &#x41;<br>d</P><img src='x.png'>", true);
        let p = root.child("p").unwrap();
        assert_eq!(p.text(), "a & b\u{a0}c Ad");
        assert!(p.child("br").is_some());
        assert_eq!(root.child("img").unwrap().attr("src"), Some("x.png"));
    }

    #[test]
    fn parses_namespaced_xml() {
        let root = parse(r#"<w:p xmlns:w="x"><w:r><w:t xml:space="preserve"> hi </w:t></w:r></w:p>"#, false);
        let t = root.find("w:t").unwrap();
        assert_eq!(t.text(), " hi ");
        assert_eq!(t.local(), "t");
        assert_eq!(t.attr_local("space"), Some("preserve"));
    }
}
