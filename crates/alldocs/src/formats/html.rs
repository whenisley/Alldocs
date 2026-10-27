//! HTML/XHTML to the document model. Used directly for .html files and by
//! the EPUB converter for chapter content.

use crate::model::*;
use crate::xml::{self, Element, Node};
use crate::{Ctx, ResourceKind, Result};

/// What a link should become in the output.
pub(crate) enum LinkTarget {
    Url(String),
    Footnote(String),
    /// Keep the link text, drop the link (e.g. internal anchors).
    TextOnly,
    /// Drop the link and its text (e.g. footnote back-links).
    Remove,
}

/// Format-specific resolution of images, links and skipped elements.
pub(crate) trait HtmlHost {
    fn image(&mut self, src: &str) -> Option<String>;
    fn link(&mut self, href: &str, el: &Element) -> LinkTarget;
    fn skip(&self, _el: &Element) -> bool {
        false
    }
}

pub(crate) fn convert(data: &[u8], ctx: &mut Ctx) -> Result<Document> {
    let root = xml::parse_bytes(data, true);
    if let Some(title) = root.find("title").map(|t| collapse_whitespace(&t.text()).trim().to_string())
        && !title.is_empty()
    {
        ctx.metadata.title = Some(title);
    }
    if let Some(lang) = root.find("html").and_then(|h| h.attr("lang")) {
        ctx.metadata.language = Some(lang.to_string());
    }
    let body = root.find("body").unwrap_or(&root);
    let mut host = PlainHost { ctx, images: 0 };
    let blocks = HtmlConverter { host: &mut host }.blocks(body);
    Ok(Document { blocks, footnotes: Vec::new() })
}

struct PlainHost<'c, 'o> {
    ctx: &'c mut Ctx<'o>,
    images: usize,
}

impl HtmlHost for PlainHost<'_, '_> {
    fn image(&mut self, src: &str) -> Option<String> {
        // Inline data: URIs become resources; other sources are kept as-is.
        if let Some((mime, data)) = parse_data_uri(src) {
            self.images += 1;
            let ext = mime.rsplit('/').next().unwrap_or("bin").replace("svg+xml", "svg").replace("jpeg", "jpg");
            let name = format!("image{}.{ext}", self.images);
            return Some(self.ctx.add_resource(&format!("data-uri:{}", self.images), &name, data, ResourceKind::Image));
        }
        Some(src.to_string())
    }

    fn link(&mut self, href: &str, _el: &Element) -> LinkTarget {
        if href.starts_with('#') || href.starts_with("javascript:") {
            LinkTarget::TextOnly
        } else {
            LinkTarget::Url(href.to_string())
        }
    }
}

fn parse_data_uri(src: &str) -> Option<(String, Vec<u8>)> {
    let rest = src.strip_prefix("data:")?;
    let (meta, payload) = rest.split_once(',')?;
    let mime = meta.split(';').next().unwrap_or("application/octet-stream").to_string();
    if !meta.ends_with(";base64") {
        return None;
    }
    Some((mime, crate::base64_decode(payload)?))
}

const BLOCK_TAGS: &[&str] = &[
    "address",
    "article",
    "aside",
    "blockquote",
    "body",
    "center",
    "dd",
    "details",
    "dialog",
    "dir",
    "div",
    "dl",
    "dt",
    "fieldset",
    "figcaption",
    "figure",
    "footer",
    "form",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "header",
    "hgroup",
    "hr",
    "html",
    "li",
    "main",
    "menu",
    "nav",
    "ol",
    "p",
    "pre",
    "section",
    "summary",
    "table",
    "tbody",
    "td",
    "tfoot",
    "th",
    "thead",
    "tr",
    "ul",
    "caption",
];

const SKIP_TAGS: &[&str] = &[
    "head", "script", "style", "template", "noscript", "title", "meta", "link", "button", "select", "textarea",
    "iframe", "object", "embed", "audio", "video", "canvas", "map",
];

pub(crate) struct HtmlConverter<'h, H: HtmlHost> {
    pub host: &'h mut H,
}

impl<H: HtmlHost> HtmlConverter<'_, H> {
    pub fn blocks(&mut self, container: &Element) -> Vec<Block> {
        let mut out = Vec::new();
        let mut buf: Vec<Inline> = Vec::new();
        for node in &container.children {
            match node {
                Node::Text(t) => buf.push(Inline::Text(collapse_whitespace(t))),
                Node::Element(el) => {
                    let tag = el.local();
                    if SKIP_TAGS.contains(&tag) || self.host.skip(el) {
                        continue;
                    }
                    if BLOCK_TAGS.contains(&tag) {
                        flush(&mut buf, &mut out);
                        out.extend(self.block(el));
                    } else {
                        self.inline(el, &mut buf);
                    }
                }
            }
        }
        flush(&mut buf, &mut out);
        out
    }

    fn block(&mut self, el: &Element) -> Vec<Block> {
        let tag = el.local();
        match tag {
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                let mut content = Vec::new();
                self.inline_children(el, &mut content);
                trim_inlines(&mut content);
                if !has_content(&content) {
                    return Vec::new();
                }
                vec![Block::Heading { level: tag.as_bytes()[1] - b'0', content }]
            }
            "ul" | "ol" | "menu" | "dir" => self.list(el).into_iter().collect(),
            "pre" => {
                let code = el.text();
                let code = code.strip_prefix('\n').unwrap_or(&code).to_string();
                let language = code_language(el).or_else(|| el.find("code").and_then(code_language));
                if code.trim().is_empty() { Vec::new() } else { vec![Block::CodeBlock { language, code }] }
            }
            "blockquote" => {
                let inner = self.blocks(el);
                if inner.is_empty() { Vec::new() } else { vec![Block::BlockQuote(inner)] }
            }
            "hr" => vec![Block::ThematicBreak],
            "table" => self.table(el),
            "dt" => {
                let mut content = Vec::new();
                self.inline_children(el, &mut content);
                trim_inlines(&mut content);
                if has_content(&content) { vec![Block::Paragraph(vec![Inline::Strong(content)])] } else { Vec::new() }
            }
            "figcaption" | "caption" => {
                let mut content = Vec::new();
                self.inline_children(el, &mut content);
                trim_inlines(&mut content);
                if has_content(&content) { vec![Block::Paragraph(vec![Inline::Emphasis(content)])] } else { Vec::new() }
            }
            _ => self.blocks(el),
        }
    }

    fn list(&mut self, el: &Element) -> Option<Block> {
        let ordered = el.local() == "ol";
        let start = el.attr("start").and_then(|s| s.trim().parse().ok()).unwrap_or(1);
        let mut items: Vec<ListItem> = Vec::new();
        for child in el.elements() {
            match child.local() {
                "li" => {
                    let checked = child
                        .find_local("input")
                        .filter(|i| i.attr("type").is_some_and(|t| t.eq_ignore_ascii_case("checkbox")))
                        .map(|i| i.attr("checked").is_some());
                    items.push(ListItem { checked, blocks: self.blocks(child) });
                }
                // Lists nested directly in a list belong to the previous item.
                "ul" | "ol" => {
                    if let Some(nested) = self.list(child) {
                        match items.last_mut() {
                            Some(item) => item.blocks.push(nested),
                            None => items.push(ListItem { checked: None, blocks: vec![nested] }),
                        }
                    }
                }
                _ => {}
            }
        }
        (!items.is_empty()).then_some(Block::List(List { ordered, start, items }))
    }

    fn table(&mut self, table: &Element) -> Vec<Block> {
        let mut out = Vec::new();
        let mut head_rows = Vec::new();
        let mut body_rows = Vec::new();
        let mut foot_rows = Vec::new();
        for child in table.elements() {
            match child.local() {
                "caption" => out.extend(self.block(child)),
                "thead" => head_rows.extend(child.elements().filter(|e| e.local() == "tr")),
                "tbody" => body_rows.extend(child.elements().filter(|e| e.local() == "tr")),
                "tfoot" => foot_rows.extend(child.elements().filter(|e| e.local() == "tr")),
                "tr" => body_rows.push(child),
                _ => {}
            }
        }
        let mut rows = Vec::new();
        for tr in head_rows.into_iter().chain(body_rows).chain(foot_rows) {
            let mut row = Vec::new();
            for cell in tr.elements().filter(|e| matches!(e.local(), "td" | "th")) {
                let span = cell.attr("colspan").and_then(|s| s.parse::<usize>().ok()).unwrap_or(1).clamp(1, 64);
                row.push(blocks_to_inlines(&self.blocks(cell)));
                row.extend(std::iter::repeat_n(Vec::new(), span - 1));
            }
            rows.push(row);
        }
        if rows.len() == 1 && rows[0].len() == 1 {
            // Layout table with a single cell: keep its content.
            let cell = table.find_local("td").or_else(|| table.find_local("th"));
            out.extend(cell.map(|c| self.blocks(c)).unwrap_or_default());
        } else if !rows.is_empty() {
            out.push(Block::Table(Table { rows, alignments: Vec::new() }));
        }
        out
    }

    fn inline_children(&mut self, el: &Element, out: &mut Vec<Inline>) {
        for node in &el.children {
            match node {
                Node::Text(t) => out.push(Inline::Text(collapse_whitespace(t))),
                Node::Element(c) => {
                    if !SKIP_TAGS.contains(&c.local()) && !self.host.skip(c) {
                        self.inline(c, out);
                    }
                }
            }
        }
    }

    fn inline(&mut self, el: &Element, out: &mut Vec<Inline>) {
        let wrap = |this: &mut Self, f: fn(Vec<Inline>) -> Inline, out: &mut Vec<Inline>| {
            let mut inner = Vec::new();
            this.inline_children(el, &mut inner);
            out.push(f(inner));
        };
        match el.local() {
            "strong" | "b" => wrap(self, Inline::Strong, out),
            "em" | "i" | "cite" | "dfn" | "var" => wrap(self, Inline::Emphasis, out),
            "del" | "s" | "strike" => wrap(self, Inline::Strikethrough, out),
            "code" | "kbd" | "samp" | "tt" => out.push(Inline::Code(collapse_whitespace(&el.text()))),
            "br" => out.push(Inline::LineBreak),
            "img" => {
                let src = el.attr("src").or_else(|| el.attr("data-src")).unwrap_or("");
                if !src.is_empty()
                    && let Some(src) = self.host.image(src)
                {
                    let alt = el.attr("alt").unwrap_or("").to_string();
                    out.push(Inline::Image { src, alt, title: el.attr("title").map(str::to_string) });
                }
            }
            "svg" => {
                if let Some(img) = el.find_local("image") {
                    let href = img.attr_local("href").unwrap_or("");
                    if let Some(src) = (!href.is_empty()).then(|| self.host.image(href)).flatten() {
                        out.push(Inline::Image { src, alt: String::new(), title: None });
                    }
                }
            }
            "a" => {
                let Some(href) = el.attr("href").map(str::trim).filter(|h| !h.is_empty()) else {
                    self.inline_children(el, out);
                    return;
                };
                match self.host.link(href, el) {
                    LinkTarget::Url(url) => {
                        let mut content = Vec::new();
                        self.inline_children(el, &mut content);
                        out.push(Inline::Link { url, title: el.attr("title").map(str::to_string), content });
                    }
                    LinkTarget::Footnote(label) => out.push(Inline::FootnoteRef(label)),
                    LinkTarget::TextOnly => self.inline_children(el, out),
                    LinkTarget::Remove => {}
                }
            }
            "input" => {}
            _ => self.inline_children(el, out),
        }
    }
}

fn flush(buf: &mut Vec<Inline>, out: &mut Vec<Block>) {
    if buf.is_empty() {
        return;
    }
    let mut content = std::mem::take(buf);
    trim_inlines(&mut content);
    if has_content(&content) {
        out.push(Block::Paragraph(content));
    }
}

fn code_language(el: &Element) -> Option<String> {
    el.attr("class")?.split_whitespace().find_map(|c| {
        c.strip_prefix("language-")
            .or_else(|| c.strip_prefix("lang-"))
            .or_else(|| c.strip_prefix("highlight-source-"))
            .map(str::to_string)
    })
}

#[cfg(test)]
mod tests {
    use crate::{Options, convert_bytes};

    #[test]
    fn converts_html() {
        let html = br#"<!DOCTYPE html><html><head><title>T</title></head><body>
            <h1>Title</h1><p>Hello <b>bold</b> and <a href="https://x.y">link</a>.<br>Next</p>
            <ul><li>one<ul><li>nested</li></ul></li><li><input type="checkbox" checked> done</li></ul>
            <pre><code class="language-rust">fn main() {}
</code></pre>
            <table><tr><th>A</th><th>B</th></tr><tr><td>1</td><td>2</td></tr></table>
            </body></html>"#;
        let out = convert_bytes(html, None, &Options::default()).unwrap();
        assert_eq!(out.metadata.title.as_deref(), Some("T"));
        assert_eq!(
            out.markdown,
            "# Title\n\nHello **bold** and [link](https://x.y).\\\nNext\n\n- one\n  - nested\n- [x] done\n\n```rust\nfn main() {}\n```\n\n| A   | B   |\n| --- | --- |\n| 1   | 2   |\n"
        );
    }
}
