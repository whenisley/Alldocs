//! OpenDocument text (.odt) and presentation (.odp).
//!
//! ODF spreadsheets (.ods) are handled by `sheet.rs` via calamine.

use std::collections::HashMap;

use super::{is_monospace_font, style_name_heading_level};
use crate::model::*;
use crate::package::{Package, resolve};
use crate::runs::{Item, Para, ParaKind, RunBuilder, Style, assemble};
use crate::xml::{Element, Node};
use crate::{Ctx, Error, Format, ResourceKind, Result};

pub(crate) fn convert(format: Format, data: &[u8], ctx: &mut Ctx) -> Result<Document> {
    let mut pkg = Package::open(data, format)?;
    pkg.read_odf_meta(&mut ctx.metadata);
    let content = pkg.read_xml("content.xml").ok_or_else(|| Error::invalid(format, "missing content.xml"))?;
    let mut styles = OdfStyles::default();
    if let Some(s) = pkg.read_xml("styles.xml") {
        styles.load(&s);
    }
    styles.load(&content);

    let mut conv = Odf { pkg, ctx, styles, footnotes: Vec::new(), list_stack: Vec::new() };
    let body = content.find("office:body").ok_or_else(|| Error::invalid(format, "document has no body"))?;
    let blocks = match format {
        Format::Odp => {
            let pres = body.child("office:presentation").ok_or_else(|| Error::invalid(format, "not a presentation"))?;
            conv.presentation(pres)
        }
        _ => {
            let text = body.child("office:text").ok_or_else(|| Error::invalid(format, "not a text document"))?;
            conv.blocks(text)
        }
    };
    Ok(Document { blocks, footnotes: conv.footnotes })
}

struct Odf<'a, 'c, 'o> {
    pkg: Package<'a>,
    ctx: &'c mut Ctx<'o>,
    styles: OdfStyles,
    footnotes: Vec<Footnote>,
    /// Enclosing lists: (list style name, nesting depth starting at 1).
    list_stack: Vec<(Option<String>, u32)>,
}

impl<'a, 'c, 'o> Odf<'a, 'c, 'o> {
    fn blocks(&mut self, container: &Element) -> Vec<Block> {
        self.blocks_of(container.elements())
    }

    fn blocks_of<'e>(&mut self, elements: impl Iterator<Item = &'e Element>) -> Vec<Block> {
        let mut items = Vec::new();
        for el in elements {
            match el.name.as_str() {
                "text:h" => {
                    let level =
                        el.attr("text:outline-level").and_then(|l| l.parse::<u8>().ok()).unwrap_or(1).clamp(1, 6);
                    let (rb, extra) = self.paragraph_runs(el);
                    items.push(Item::Para(Para { kind: ParaKind::Heading(level), inlines: rb.finish() }));
                    items.extend(extra);
                }
                "text:p" => {
                    let style = self.styles.paragraph(el.attr("text:style-name"));
                    let (rb, extra) = self.paragraph_runs(el);
                    let kind = if let Some(level) = style.heading {
                        ParaKind::Heading(level)
                    } else if style.code || rb.is_all_code() {
                        ParaKind::Code
                    } else if style.quote {
                        ParaKind::Quote
                    } else {
                        ParaKind::Normal
                    };
                    let inlines =
                        if kind == ParaKind::Code { vec![Inline::Text(rb.plain_text())] } else { rb.finish() };
                    items.push(Item::Para(Para { kind, inlines }));
                    items.extend(extra);
                }
                "text:list" => {
                    if contains_heading(el) {
                        // Outline-numbered headings are lists of text:h; keep them as headings.
                        for item in el.elements() {
                            items.extend(self.blocks(item).into_iter().map(Item::Block));
                        }
                    } else if let Some(list) = self.list(el) {
                        items.push(Item::Block(Block::List(list)));
                    }
                }
                "table:table" => items.extend(self.table(el).into_iter().map(Item::Block)),
                "text:section" | "draw:text-box" | "text:index-body" | "text:index-title" | "office:text" => {
                    items.extend(self.blocks(el).into_iter().map(Item::Block))
                }
                "text:table-of-content"
                | "text:alphabetical-index"
                | "text:illustration-index"
                | "text:table-index"
                | "text:object-index"
                | "text:user-index"
                | "text:bibliography" => {
                    if let Some(body) = el.child("text:index-body") {
                        items.extend(self.blocks(body).into_iter().map(Item::Block));
                    }
                }
                "draw:frame" | "draw:a" => {
                    let mut rb = RunBuilder::new();
                    let mut extra = Vec::new();
                    if el.name == "draw:frame" {
                        self.frame(el, &mut rb, None, &mut extra);
                    } else {
                        self.inlines(el, Style::default(), None, &mut rb, &mut extra, true);
                    }
                    items.push(Item::Para(Para { kind: ParaKind::Normal, inlines: rb.finish() }));
                    items.extend(extra);
                }
                "draw:custom-shape" | "draw:g" | "draw:rect" | "draw:ellipse" => {
                    items.extend(self.blocks(el).into_iter().map(Item::Block))
                }
                _ => {}
            }
        }
        assemble(items)
    }

    fn paragraph_runs(&mut self, el: &Element) -> (RunBuilder, Vec<Item>) {
        let base = self.styles.paragraph(el.attr("text:style-name")).run;
        let mut rb = RunBuilder::new();
        let mut extra = Vec::new();
        self.inlines(el, base, None, &mut rb, &mut extra, false);
        (rb, extra)
    }

    fn inlines(
        &mut self,
        el: &Element,
        style: Style,
        link: Option<&str>,
        rb: &mut RunBuilder,
        extra: &mut Vec<Item>,
        block_level: bool,
    ) {
        for node in &el.children {
            let c = match node {
                Node::Text(t) => {
                    if !block_level {
                        rb.text(&collapse_whitespace(t), style, link);
                    }
                    continue;
                }
                Node::Element(c) => c,
            };
            match c.name.as_str() {
                "text:span" => {
                    let s = self.styles.text(c.attr("text:style-name"), style);
                    self.inlines(c, s, link, rb, extra, false);
                }
                "text:a" | "draw:a" => {
                    let href = c.attr("xlink:href").filter(|h| !h.starts_with('#') && !h.is_empty());
                    let url = href.map(str::to_string);
                    self.inlines(c, style, url.as_deref().or(link), rb, extra, block_level && c.name == "draw:a");
                }
                "text:s" => {
                    let n = c.attr("text:c").and_then(|v| v.parse::<usize>().ok()).unwrap_or(1).min(256);
                    rb.text(&" ".repeat(n), style, link);
                }
                "text:tab" => rb.text("\t", style, link),
                "text:line-break" => rb.line_break(),
                "text:note" => {
                    let label = (self.footnotes.len() + 1).to_string();
                    // Reserve the slot first so nested notes number after this one.
                    self.footnotes.push(Footnote { label: label.clone(), blocks: Vec::new() });
                    let idx = self.footnotes.len() - 1;
                    let blocks = c.child("text:note-body").map(|b| self.blocks(b)).unwrap_or_default();
                    self.footnotes[idx].blocks = blocks;
                    rb.inline(Inline::FootnoteRef(label), Style::default(), None);
                }
                "draw:frame" => self.frame(c, rb, link, extra),
                "draw:custom-shape" | "draw:rect" | "draw:ellipse" | "draw:g" => {
                    let inner = self.blocks(c);
                    extra.extend(inner.into_iter().map(Item::Block));
                }
                "text:soft-page-break"
                | "text:bookmark"
                | "text:bookmark-start"
                | "text:bookmark-end"
                | "text:reference-mark"
                | "text:reference-mark-start"
                | "text:reference-mark-end"
                | "office:annotation"
                | "office:annotation-end"
                | "text:change"
                | "text:change-start"
                | "text:change-end"
                | "text:sequence-decls" => {}
                _ => {
                    if !block_level {
                        self.inlines(c, style, link, rb, extra, false);
                    }
                }
            }
        }
    }

    fn frame(&mut self, frame: &Element, rb: &mut RunBuilder, link: Option<&str>, extra: &mut Vec<Item>) {
        let alt = frame
            .child("svg:title")
            .or_else(|| frame.child("svg:desc"))
            .map(|t| t.text().trim().to_string())
            .unwrap_or_default();
        if let Some(obj) = frame.child("draw:object-ole")
            && let Some(href) = obj.attr("xlink:href")
        {
            let path = resolve("", href);
            if let Some(data) = self.pkg.read(&path) {
                let src = self.ctx.add_resource(&path, &path, data, ResourceKind::Object);
                rb.inline(
                    Inline::Link { url: src, title: None, content: vec![Inline::text("Embedded object")] },
                    Style::default(),
                    None,
                );
            }
        }
        if let Some(img) = frame.child("draw:image")
            && let Some(href) = img.attr("xlink:href")
        {
            let src = if href.contains("://") {
                Some(href.to_string())
            } else {
                let path = resolve("", href);
                self.pkg.read(&path).map(|data| self.ctx.add_resource(&path, &path, data, ResourceKind::Image))
            };
            if let Some(src) = src {
                rb.inline(Inline::Image { src, alt, title: None }, Style::default(), link);
            }
        }
        if let Some(text_box) = frame.child("draw:text-box") {
            let inner = self.blocks(text_box);
            extra.extend(inner.into_iter().map(Item::Block));
        }
        if let Some(table) = frame.child("table:table") {
            extra.extend(self.table(table).into_iter().map(Item::Block));
        }
    }

    fn list(&mut self, el: &Element) -> Option<List> {
        let style = el
            .attr("text:style-name")
            .map(str::to_string)
            .or_else(|| self.list_stack.last().and_then(|(s, _)| s.clone()));
        let depth = self.list_stack.last().map_or(1, |(_, d)| d + 1);
        let (ordered, mut start) =
            style.as_deref().and_then(|s| self.styles.list_level(s, depth)).unwrap_or((false, 1));
        self.list_stack.push((style, depth));
        let mut items = Vec::new();
        for li in el.elements().filter(|e| e.name == "text:list-item" || e.name == "text:list-header") {
            if items.is_empty()
                && let Some(s) = li.attr("text:start-value").and_then(|v| v.parse().ok())
            {
                start = s;
            }
            let blocks = self.blocks(li);
            items.push(ListItem { checked: None, blocks });
        }
        self.list_stack.pop();
        (!items.is_empty()).then_some(List { ordered, start, items })
    }

    fn table(&mut self, table: &Element) -> Vec<Block> {
        let mut rows: Vec<Vec<Vec<Block>>> = Vec::new();
        let mut row_elems = Vec::new();
        collect_rows(table, &mut row_elems);
        for row in row_elems {
            let repeat = row.attr("table:number-rows-repeated").and_then(|v| v.parse::<usize>().ok()).unwrap_or(1);
            let mut cells: Vec<Vec<Block>> = Vec::new();
            for cell in row.elements() {
                let n = cell.attr("table:number-columns-repeated").and_then(|v| v.parse::<usize>().ok()).unwrap_or(1);
                let blocks = if cell.name == "table:table-cell" { self.blocks(cell) } else { Vec::new() };
                if blocks.is_empty() {
                    cells.extend(std::iter::repeat_n(Vec::new(), n.min(64)));
                } else {
                    for _ in 0..n.min(64) {
                        cells.push(blocks.clone());
                    }
                }
            }
            while cells.last().is_some_and(Vec::is_empty) {
                cells.pop();
            }
            let empty = cells.is_empty();
            let times = if empty { 1 } else { repeat.min(1000) };
            for _ in 0..times {
                rows.push(cells.clone());
            }
        }
        while rows.last().is_some_and(Vec::is_empty) {
            rows.pop();
        }
        if rows.len() == 1 && rows[0].len() == 1 {
            return rows.pop().and_then(|mut r| r.pop()).unwrap_or_default();
        }
        if rows.is_empty() {
            return Vec::new();
        }
        let rows = rows.iter().map(|r| r.iter().map(|c| blocks_to_inlines(c)).collect()).collect();
        vec![Block::Table(Table { rows, alignments: Vec::new() })]
    }

    fn presentation(&mut self, pres: &Element) -> Vec<Block> {
        let pages: Vec<&Element> = pres.children_named("draw:page").collect();
        self.ctx.metadata.pages = Some(pages.len());
        let mut blocks = Vec::new();
        for (i, page) in pages.into_iter().enumerate() {
            let number = i + 1;
            blocks.push(Block::Comment(format!("Slide {number}")));
            let mut shapes: Vec<&Element> = page.elements().filter(|e| e.name != "presentation:notes").collect();
            shapes.sort_by(|a, b| {
                let key = |e: &Element| (len_mm(e.attr("svg:y")), len_mm(e.attr("svg:x")));
                key(a).partial_cmp(&key(b)).unwrap_or(std::cmp::Ordering::Equal)
            });
            let title = shapes
                .iter()
                .find(|e| e.attr("presentation:class") == Some("title"))
                .map(|e| collapse_whitespace(&e.text()).trim().to_string())
                .filter(|t| !t.is_empty());
            blocks.push(Block::Heading {
                level: 2,
                content: vec![Inline::Text(title.clone().unwrap_or_else(|| format!("Slide {number}")))],
            });
            for shape in shapes {
                let class = shape.attr("presentation:class");
                if class == Some("title") && title.is_some() {
                    continue;
                }
                if matches!(class, Some("page-number" | "footer" | "date-time" | "header")) {
                    continue;
                }
                blocks.extend(self.blocks_of(std::iter::once(shape)));
            }
            if self.ctx.opts.include_notes
                && let Some(notes) = page.child("presentation:notes")
            {
                let mut note_blocks = Vec::new();
                for frame in notes.elements().filter(|e| e.attr("presentation:class") == Some("notes")) {
                    if let Some(tb) = frame.child("draw:text-box") {
                        note_blocks.extend(self.blocks(tb));
                    }
                }
                if !note_blocks.is_empty() {
                    blocks.push(Block::Heading { level: 3, content: vec![Inline::text("Notes")] });
                    blocks.extend(note_blocks);
                }
            }
        }
        blocks
    }
}

fn contains_heading(list: &Element) -> bool {
    list.elements()
        .any(|item| item.elements().any(|c| c.name == "text:h" || (c.name == "text:list" && contains_heading(c))))
}

fn collect_rows<'e>(el: &'e Element, out: &mut Vec<&'e Element>) {
    for child in el.elements() {
        match child.name.as_str() {
            "table:table-row" => out.push(child),
            "table:table-header-rows" | "table:table-rows" | "table:table-row-group" => collect_rows(child, out),
            _ => {}
        }
    }
}

/// Parses an ODF length ("2.5cm", "1in", "12pt") to millimetres.
fn len_mm(value: Option<&str>) -> f64 {
    let Some(v) = value else { return 0.0 };
    let split = v.find(|c: char| c.is_ascii_alphabetic()).unwrap_or(v.len());
    let n: f64 = v[..split].parse().unwrap_or(0.0);
    n * match &v[split..] {
        "cm" => 10.0,
        "in" | "inch" => 25.4,
        "pt" => 0.3528,
        "pc" => 4.233,
        "px" => 0.2646,
        _ => 1.0,
    }
}

// ---------------------------------------------------------------------------
// Styles

#[derive(Default, Clone)]
struct StyleDef {
    parent: Option<String>,
    display: String,
    bold: Option<bool>,
    italic: Option<bool>,
    strike: Option<bool>,
    mono: Option<bool>,
}

#[derive(Default, Clone, Copy)]
struct ParaStyle {
    heading: Option<u8>,
    code: bool,
    quote: bool,
    run: Style,
}

#[derive(Default)]
struct OdfStyles {
    paragraph: HashMap<String, StyleDef>,
    text: HashMap<String, StyleDef>,
    fonts: HashMap<String, bool>,
    /// list style -> level -> (ordered, start)
    lists: HashMap<String, HashMap<u32, (bool, u64)>>,
}

impl OdfStyles {
    fn load(&mut self, root: &Element) {
        let Some(doc) = root.elements().next() else { return };
        if let Some(fonts) = doc.child("office:font-face-decls") {
            for f in fonts.children_named("style:font-face") {
                let Some(name) = f.attr("style:name") else { continue };
                let family = f.attr("svg:font-family").unwrap_or(name).trim_matches(['\'', '"']);
                let mono = f.attr("style:font-pitch") == Some("fixed") || is_monospace_font(family);
                self.fonts.insert(name.to_string(), mono);
            }
        }
        for section in ["office:styles", "office:automatic-styles"] {
            let Some(styles) = doc.child(section) else { continue };
            for s in styles.elements() {
                match s.name.as_str() {
                    "style:style" => {
                        let Some(name) = s.attr("style:name") else { continue };
                        let def = self.style_def(s);
                        match s.attr("style:family") {
                            Some("paragraph") => {
                                self.paragraph.insert(name.to_string(), def);
                            }
                            Some("text") => {
                                self.text.insert(name.to_string(), def);
                            }
                            _ => {}
                        }
                    }
                    "text:list-style" => {
                        let Some(name) = s.attr("style:name") else { continue };
                        let mut levels = HashMap::new();
                        for lvl in s.elements() {
                            let Some(level) = lvl.attr("text:level").and_then(|v| v.parse().ok()) else { continue };
                            let ordered = lvl.name == "text:list-level-style-number"
                                && lvl.attr("style:num-format").is_none_or(|f| !f.is_empty());
                            let start = lvl.attr("text:start-value").and_then(|v| v.parse().ok()).unwrap_or(1);
                            levels.insert(level, (ordered, start));
                        }
                        self.lists.insert(name.to_string(), levels);
                    }
                    _ => {}
                }
            }
        }
    }

    fn style_def(&self, s: &Element) -> StyleDef {
        let tp = s.child("style:text-properties");
        let attr = |name: &str| tp.and_then(|t| t.attr(name));
        let bold = attr("fo:font-weight").map(|w| w == "bold" || w.parse::<u32>().is_ok_and(|n| n >= 600));
        let italic = attr("fo:font-style").map(|v| v == "italic" || v == "oblique");
        let strike = attr("style:text-line-through-style").map(|v| v != "none");
        let mono = attr("style:font-name")
            .map(|f| self.fonts.get(f).copied().unwrap_or_else(|| is_monospace_font(f)))
            .or_else(|| attr("fo:font-family").map(is_monospace_font));
        let display = s
            .attr("style:display-name")
            .map(str::to_string)
            .unwrap_or_else(|| s.attr("style:name").unwrap_or("").replace("_20_", " "));
        StyleDef { parent: s.attr("style:parent-style-name").map(str::to_string), display, bold, italic, strike, mono }
    }

    fn resolve<'m>(map: &'m HashMap<String, StyleDef>, name: Option<&str>) -> (Vec<&'m StyleDef>, Style) {
        let mut chain = Vec::new();
        let mut cur = name.map(str::to_string);
        for _ in 0..16 {
            let Some(def) = cur.as_ref().and_then(|c| map.get(c)) else { break };
            chain.push(def);
            cur = def.parent.clone();
        }
        let pick = |f: fn(&StyleDef) -> Option<bool>| chain.iter().find_map(|d| f(d)).unwrap_or(false);
        let style = Style {
            bold: pick(|d| d.bold),
            italic: pick(|d| d.italic),
            strike: pick(|d| d.strike),
            code: pick(|d| d.mono),
        };
        (chain, style)
    }

    fn paragraph(&self, name: Option<&str>) -> ParaStyle {
        let (chain, run) = Self::resolve(&self.paragraph, name);
        let mut out = ParaStyle { run, ..ParaStyle::default() };
        for def in &chain {
            let lower = def.display.to_lowercase();
            if out.heading.is_none() {
                out.heading = style_name_heading_level(&lower);
            }
            out.code |= ["preformatted", "source text", "code", "verbatim"].iter().any(|k| lower.contains(k));
            out.quote |= lower.contains("quotation") || lower.contains("quote");
        }
        out
    }

    fn text(&self, name: Option<&str>, base: Style) -> Style {
        let (chain, _) = Self::resolve(&self.text, name);
        let pick = |f: fn(&StyleDef) -> Option<bool>, default: bool| chain.iter().find_map(|d| f(d)).unwrap_or(default);
        let mut style = Style {
            bold: pick(|d| d.bold, base.bold),
            italic: pick(|d| d.italic, base.italic),
            strike: pick(|d| d.strike, base.strike),
            code: pick(|d| d.mono, base.code),
        };
        if chain.iter().any(|d| {
            let l = d.display.to_lowercase();
            l.contains("source text") || l.contains("code") || l.contains("teletype")
        }) {
            style.code = true;
        }
        style
    }

    fn list_level(&self, style: &str, level: u32) -> Option<(bool, u64)> {
        self.lists.get(style)?.get(&level).copied()
    }
}
