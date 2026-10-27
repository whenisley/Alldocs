//! Word (.docx) — WordprocessingML.

use std::collections::HashMap;

use super::{is_monospace_font, style_name_heading_level};
use crate::model::*;
use crate::package::{Package, Rels};
use crate::runs::{Item, Para, ParaKind, RunBuilder, Style, assemble};
use crate::xml::{Element, Node};
use crate::{Ctx, Error, Format, ResourceKind, Result};

pub(crate) fn convert(data: &[u8], ctx: &mut Ctx) -> Result<Document> {
    let mut pkg = Package::open(data, Format::Docx)?;
    pkg.read_core_properties(&mut ctx.metadata);

    let main = pkg.main_part("officeDocument", "word/document.xml");
    let root = pkg.read_xml(&main).ok_or_else(|| Error::invalid(Format::Docx, "missing word/document.xml"))?;
    let rels = pkg.rels(&main);

    let styles = rels
        .0
        .values()
        .find(|r| r.kind == "styles")
        .and_then(|r| pkg.read_xml(&r.target))
        .map(|x| Styles::parse(&x))
        .unwrap_or_default();
    let numbering = rels
        .0
        .values()
        .find(|r| r.kind == "numbering")
        .and_then(|r| pkg.read_xml(&r.target))
        .map(|x| Numbering::parse(&x))
        .unwrap_or_default();

    let mut notes = HashMap::new();
    let mut note_rels = HashMap::new();
    for (kind, prefix, elem) in [("footnotes", "f", "w:footnote"), ("endnotes", "e", "w:endnote")] {
        let Some(part) = rels.0.values().find(|r| r.kind == kind).map(|r| r.target.clone()) else { continue };
        let Some(mut xml) = pkg.read_xml(&part) else { continue };
        note_rels.insert(prefix, pkg.rels(&part));
        if let Some(Node::Element(list)) = xml.children.iter_mut().find(|n| matches!(n, Node::Element(_))) {
            for node in std::mem::take(&mut list.children) {
                if let Node::Element(el) = node
                    && el.name == elem
                    && let Some(id) = el.attr("w:id").map(str::to_string)
                {
                    notes.insert(format!("{prefix}{id}"), el);
                }
            }
        }
    }

    let mut conv =
        Docx { pkg, ctx, styles, numbering, rels, notes, note_order: Vec::new(), note_labels: HashMap::new() };

    let body = root.find("w:body").ok_or_else(|| Error::invalid(Format::Docx, "document has no body"))?;
    let mut items = Vec::new();
    conv.block_items(body, &mut items);
    let blocks = assemble(items);

    // Footnotes are converted in order of first reference; a note may
    // reference further notes, so the list can grow while iterating.
    let mut footnotes = Vec::new();
    let mut i = 0;
    while i < conv.note_order.len() {
        let (key, label) = conv.note_order[i].clone();
        i += 1;
        let Some(el) = conv.notes.remove(&key) else { continue };
        let prefix = &key[..1];
        if let Some(r) = note_rels.get(prefix) {
            conv.rels = r.clone();
        }
        let mut items = Vec::new();
        conv.block_items(&el, &mut items);
        footnotes.push(Footnote { label, blocks: assemble(items) });
    }

    Ok(Document { blocks, footnotes })
}

struct Docx<'a, 'c, 'o> {
    pkg: Package<'a>,
    ctx: &'c mut Ctx<'o>,
    styles: Styles,
    numbering: Numbering,
    rels: Rels,
    notes: HashMap<String, Element>,
    note_order: Vec<(String, String)>,
    note_labels: HashMap<String, String>,
}

/// An open complex field (`w:fldChar begin ... separate ... end`).
struct Field {
    instr: String,
    result_start: Option<usize>,
}

#[derive(Clone, Copy, Default)]
struct RunProps {
    bold: Option<bool>,
    italic: Option<bool>,
    strike: Option<bool>,
    mono: Option<bool>,
    hidden: Option<bool>,
}

impl RunProps {
    fn merged(self, over: RunProps) -> RunProps {
        RunProps {
            bold: over.bold.or(self.bold),
            italic: over.italic.or(self.italic),
            strike: over.strike.or(self.strike),
            mono: over.mono.or(self.mono),
            hidden: over.hidden.or(self.hidden),
        }
    }

    fn from_rpr(rpr: Option<&Element>) -> RunProps {
        let Some(rpr) = rpr else { return RunProps::default() };
        let toggle =
            |name: &str| rpr.child(name).map(|e| !matches!(e.attr("w:val"), Some("0" | "false" | "off" | "none")));
        let mono = rpr.child("w:rFonts").and_then(|f| {
            f.attr("w:ascii").or_else(|| f.attr("w:hAnsi")).or_else(|| f.attr("w:cs")).map(is_monospace_font)
        });
        RunProps {
            bold: toggle("w:b"),
            italic: toggle("w:i"),
            strike: match (toggle("w:strike"), toggle("w:dstrike")) {
                (None, None) => None,
                (a, b) => Some(a.unwrap_or(false) || b.unwrap_or(false)),
            },
            mono,
            hidden: toggle("w:vanish"),
        }
    }
}

impl<'a, 'c, 'o> Docx<'a, 'c, 'o> {
    fn block_items(&mut self, container: &Element, items: &mut Vec<Item>) {
        for el in container.elements() {
            match el.name.as_str() {
                "w:p" => self.paragraph(el, items),
                "w:tbl" => {
                    for block in self.table(el) {
                        items.push(Item::Block(block));
                    }
                }
                "w:sdt" => {
                    if let Some(content) = el.child("w:sdtContent") {
                        self.block_items(content, items);
                    }
                }
                "w:customXml" | "w:ins" | "w:moveTo" | "w:txbxContent" => self.block_items(el, items),
                "mc:AlternateContent" => {
                    if let Some(choice) = el.child("mc:Choice") {
                        self.block_items(choice, items);
                    }
                }
                _ => {}
            }
        }
    }

    fn paragraph(&mut self, p: &Element, items: &mut Vec<Item>) {
        let ppr = p.child("w:pPr");
        let style_id = ppr.and_then(|x| x.child("w:pStyle")).and_then(|s| s.attr("w:val"));
        let style = self.styles.resolve(style_id);

        let mut heading = style.heading;
        if let Some(lvl) =
            ppr.and_then(|x| x.child("w:outlineLvl")).and_then(|o| o.attr("w:val")).and_then(|v| v.parse::<u8>().ok())
            && lvl < 9
        {
            heading = Some((lvl + 1).min(6));
        }
        let num = ppr
            .and_then(|x| x.child("w:numPr"))
            .and_then(|n| {
                let id = n.child("w:numId")?.attr("w:val")?.to_string();
                let lvl = n.child("w:ilvl").and_then(|l| l.attr("w:val")).and_then(|v| v.parse().ok()).unwrap_or(0);
                Some((id, lvl))
            })
            .or_else(|| style.num.clone());

        // Bold/italic that comes from a heading or quote style is
        // presentation, not emphasis.
        let mut base = style.run;
        if heading.is_some() || style.quote {
            base.bold = None;
            base.italic = None;
        }
        let mut rb = RunBuilder::new();
        let mut extra = Vec::new();
        let mut fields = Vec::new();
        self.inline_children(p, &mut rb, base, None, &mut fields, &mut extra);

        let list = num.and_then(|(id, lvl)| {
            if id == "0" {
                return None;
            }
            let def = self.numbering.level(&id, lvl)?;
            (!def.none).then_some((id, lvl, def))
        });

        let kind = if let Some(level) = heading {
            ParaKind::Heading(level)
        } else if let Some((id, lvl, def)) = list {
            // "List Bullet 2" etc. imply a nesting level even when ilvl is 0.
            let level = if lvl == 0 { style.list_level_hint } else { lvl };
            ParaKind::ListItem { level, ordered: def.ordered, start: def.start, list_id: id }
        } else if style.code || rb.is_all_code() {
            ParaKind::Code
        } else if style.quote {
            ParaKind::Quote
        } else {
            ParaKind::Normal
        };
        let inlines = if kind == ParaKind::Code { vec![Inline::Text(rb.plain_text())] } else { rb.finish() };
        items.push(Item::Para(Para { kind, inlines }));
        items.extend(extra);
    }

    fn inline_children(
        &mut self,
        container: &Element,
        rb: &mut RunBuilder,
        base: RunProps,
        link: Option<&str>,
        fields: &mut Vec<Field>,
        extra: &mut Vec<Item>,
    ) {
        for child in container.elements() {
            match child.name.as_str() {
                "w:r" => self.run(child, rb, base, link, fields, extra),
                "w:hyperlink" => {
                    let url = child
                        .attr("r:id")
                        .and_then(|id| self.rels.get(id))
                        .filter(|r| r.external)
                        .map(|r| r.target.clone());
                    let url = url.as_deref().or(link);
                    self.inline_children(child, rb, base, url, fields, extra);
                }
                "w:fldSimple" => {
                    let start = rb.mark();
                    self.inline_children(child, rb, base, link, fields, extra);
                    if let Some(url) = child.attr("w:instr").and_then(hyperlink_from_instr) {
                        rb.link_from(start, &url);
                    }
                }
                "w:ins" | "w:moveTo" | "w:smartTag" | "w:customXml" | "w:dir" | "w:bdo" => {
                    self.inline_children(child, rb, base, link, fields, extra)
                }
                "w:sdt" => {
                    if let Some(content) = child.child("w:sdtContent") {
                        self.inline_children(content, rb, base, link, fields, extra);
                    }
                }
                "m:oMathPara" | "m:oMath" => {
                    let mut ts = Vec::new();
                    child.find_all("m:t", &mut ts);
                    let text: String = ts.iter().map(|t| t.text()).collect();
                    rb.text(&text, Style { italic: true, ..Style::default() }, link);
                }
                "mc:AlternateContent" => {
                    if let Some(choice) = child.child("mc:Choice") {
                        self.inline_children(choice, rb, base, link, fields, extra);
                    }
                }
                _ => {}
            }
        }
    }

    fn run(
        &mut self,
        r: &Element,
        rb: &mut RunBuilder,
        base: RunProps,
        link: Option<&str>,
        fields: &mut Vec<Field>,
        extra: &mut Vec<Item>,
    ) {
        let rpr = r.child("w:rPr");
        let char_style = rpr
            .and_then(|x| x.child("w:rStyle"))
            .and_then(|s| s.attr("w:val"))
            .map(|id| {
                let resolved = self.styles.resolve(Some(id));
                let mut run = resolved.run;
                if resolved.code {
                    run.mono = Some(true);
                }
                run
            })
            .unwrap_or_default();
        let props = base.merged(char_style).merged(RunProps::from_rpr(rpr));
        if props.hidden == Some(true) {
            return;
        }
        let style = Style {
            bold: props.bold.unwrap_or(false),
            italic: props.italic.unwrap_or(false),
            strike: props.strike.unwrap_or(false),
            code: props.mono.unwrap_or(false),
        };
        self.run_content(r, rb, style, link, fields, extra);
    }

    fn run_content(
        &mut self,
        r: &Element,
        rb: &mut RunBuilder,
        style: Style,
        link: Option<&str>,
        fields: &mut Vec<Field>,
        extra: &mut Vec<Item>,
    ) {
        let in_instr = |fields: &Vec<Field>| fields.last().is_some_and(|f| f.result_start.is_none());
        for c in r.elements() {
            match c.name.as_str() {
                "w:t" => {
                    if !in_instr(fields) {
                        rb.text(&c.text(), style, link);
                    }
                }
                "w:tab" | "w:ptab" => rb.text("\t", style, link),
                "w:br" => {
                    if !matches!(c.attr("w:type"), Some("page" | "column")) {
                        rb.line_break();
                    }
                }
                "w:cr" => rb.line_break(),
                "w:noBreakHyphen" => rb.text("-", style, link),
                "w:sym" => {
                    if let Some(ch) = c.attr("w:char").and_then(|h| u32::from_str_radix(h, 16).ok()) {
                        let ch = if ch >= 0xF000 { ch - 0xF000 } else { ch };
                        if let Some(ch) = char::from_u32(ch) {
                            rb.text(ch.encode_utf8(&mut [0; 4]), style, link);
                        }
                    }
                }
                "w:drawing" => self.drawing(c, rb, link, extra),
                "w:pict" | "w:object" => self.vml(c, rb, link, extra),
                "w:footnoteReference" | "w:endnoteReference" => {
                    if let Some(id) = c.attr("w:id") {
                        let prefix = if c.name == "w:footnoteReference" { "f" } else { "e" };
                        let label = self.note_label(&format!("{prefix}{id}"));
                        rb.inline(Inline::FootnoteRef(label), Style::default(), None);
                    }
                }
                "w:fldChar" => match c.attr("w:fldCharType") {
                    Some("begin") => fields.push(Field { instr: String::new(), result_start: None }),
                    Some("separate") => {
                        if let Some(f) = fields.last_mut() {
                            f.result_start = Some(rb.mark());
                        }
                    }
                    Some("end") => {
                        if let Some(f) = fields.pop()
                            && let (Some(start), Some(url)) = (f.result_start, hyperlink_from_instr(&f.instr))
                        {
                            rb.link_from(start, &url);
                        }
                    }
                    _ => {}
                },
                "w:instrText" => {
                    if let Some(f) = fields.last_mut()
                        && f.result_start.is_none()
                    {
                        f.instr.push_str(&c.text());
                    }
                }
                "mc:AlternateContent" => {
                    if let Some(choice) = c.child("mc:Choice") {
                        self.run_content(choice, rb, style, link, fields, extra);
                    }
                }
                _ => {}
            }
        }
    }

    fn note_label(&mut self, key: &str) -> String {
        if let Some(label) = self.note_labels.get(key) {
            return label.clone();
        }
        let label = (self.note_order.len() + 1).to_string();
        self.note_labels.insert(key.to_string(), label.clone());
        self.note_order.push((key.to_string(), label.clone()));
        label
    }

    fn drawing(&mut self, drawing: &Element, rb: &mut RunBuilder, link: Option<&str>, extra: &mut Vec<Item>) {
        let doc_pr = drawing.find("wp:docPr");
        let alt = doc_pr
            .and_then(|d| d.attr("descr").filter(|s| !s.is_empty()).or_else(|| d.attr("title")))
            .unwrap_or("")
            .to_string();
        let mut blips = Vec::new();
        drawing.find_all("a:blip", &mut blips);
        for blip in blips {
            let rel_id = blip.attr("r:embed").or_else(|| blip.attr("r:link"));
            if let Some(src) = rel_id.and_then(|id| self.resource_for(id, ResourceKind::Image)) {
                rb.inline(Inline::Image { src, alt: alt.clone(), title: None }, Style::default(), link);
            }
        }
        let mut boxes = Vec::new();
        drawing.find_all("w:txbxContent", &mut boxes);
        for content in boxes {
            self.block_items(content, extra);
        }
    }

    fn vml(&mut self, pict: &Element, rb: &mut RunBuilder, link: Option<&str>, extra: &mut Vec<Item>) {
        let ole = pict.find("o:OLEObject");
        if let Some(img) = pict.find("v:imagedata") {
            let rel_id = img.attr("r:id").or_else(|| img.attr("o:relid"));
            if let Some(src) = rel_id.and_then(|id| self.resource_for(id, ResourceKind::Image)) {
                let alt = img.attr("o:title").unwrap_or("").to_string();
                rb.inline(Inline::Image { src, alt, title: None }, Style::default(), link);
            }
        }
        if let Some(ole) = ole
            && let Some(path) = ole.attr("r:id").and_then(|id| self.resource_for(id, ResourceKind::Object))
        {
            let prog = ole.attr("ProgID").unwrap_or("object");
            rb.inline(
                Inline::Link { url: path, title: None, content: vec![Inline::text(format!("Embedded {prog}"))] },
                Style::default(),
                None,
            );
        }
        let mut boxes = Vec::new();
        pict.find_all("w:txbxContent", &mut boxes);
        for content in boxes {
            self.block_items(content, extra);
        }
    }

    /// Resolves a relationship to an image/object and registers it as a resource.
    fn resource_for(&mut self, rel_id: &str, kind: ResourceKind) -> Option<String> {
        let rel = self.rels.get(rel_id)?.clone();
        if rel.external {
            return Some(rel.target);
        }
        let data = self.pkg.read(&rel.target)?;
        Some(self.ctx.add_resource(&rel.target, &rel.target, data, kind))
    }

    fn table(&mut self, tbl: &Element) -> Vec<Block> {
        let mut rows: Vec<Vec<Vec<Block>>> = Vec::new();
        for tr in unwrap_sdt(tbl, "w:tr") {
            let mut row = Vec::new();
            for tc in unwrap_sdt(tr, "w:tc") {
                let tcpr = tc.child("w:tcPr");
                let span = tcpr
                    .and_then(|p| p.child("w:gridSpan"))
                    .and_then(|g| g.attr("w:val"))
                    .and_then(|v| v.parse::<usize>().ok())
                    .unwrap_or(1)
                    .clamp(1, 64);
                let merged = tcpr
                    .and_then(|p| p.child("w:vMerge"))
                    .is_some_and(|m| m.attr("w:val").is_none_or(|v| v == "continue"));
                let blocks = if merged {
                    Vec::new()
                } else {
                    let mut items = Vec::new();
                    self.block_items(tc, &mut items);
                    assemble(items)
                };
                row.push(blocks);
                row.extend(std::iter::repeat_n(Vec::new(), span - 1));
            }
            rows.push(row);
        }
        // Single-cell tables are layout boxes; keep their content as blocks.
        if rows.len() == 1 && rows[0].len() == 1 {
            return rows.pop().and_then(|mut r| r.pop()).unwrap_or_default();
        }
        if rows.is_empty() {
            return Vec::new();
        }
        let rows = rows.into_iter().map(|r| r.iter().map(|cell| blocks_to_inlines(cell)).collect()).collect();
        vec![Block::Table(Table { rows, alignments: Vec::new() })]
    }
}

/// Children named `name`, looking through `w:sdt`/`w:customXml` wrappers.
fn unwrap_sdt<'e>(parent: &'e Element, name: &str) -> Vec<&'e Element> {
    let mut out = Vec::new();
    for el in parent.elements() {
        if el.name == name {
            out.push(el);
        } else if el.name == "w:sdt" {
            if let Some(content) = el.child("w:sdtContent") {
                out.extend(unwrap_sdt(content, name));
            }
        } else if el.name == "w:customXml" {
            out.extend(unwrap_sdt(el, name));
        }
    }
    out
}

/// Extracts the URL of a `HYPERLINK "url"` field instruction.
fn hyperlink_from_instr(instr: &str) -> Option<String> {
    let rest = instr.trim().strip_prefix("HYPERLINK")?.trim();
    let mut url = String::new();
    if let Some(stripped) = rest.strip_prefix('"') {
        url = stripped.split('"').next().unwrap_or("").to_string();
    } else if !rest.starts_with('\\') {
        url = rest.split_whitespace().next().unwrap_or("").to_string();
    }
    (!url.is_empty()).then_some(url)
}

// ---------------------------------------------------------------------------
// Styles

#[derive(Default, Clone)]
struct StyleDef {
    name: String,
    based_on: Option<String>,
    heading: Option<u8>,
    num: Option<(String, usize)>,
    run: RunProps,
}

#[derive(Default, Clone)]
struct ResolvedStyle {
    heading: Option<u8>,
    code: bool,
    quote: bool,
    list_level_hint: usize,
    num: Option<(String, usize)>,
    run: RunProps,
}

#[derive(Default)]
struct Styles {
    defs: HashMap<String, StyleDef>,
    default_para: Option<String>,
}

impl Styles {
    fn parse(root: &Element) -> Styles {
        let mut styles = Styles::default();
        let Some(list) = root.child("w:styles") else { return styles };
        for s in list.children_named("w:style") {
            let Some(id) = s.attr("w:styleId") else { continue };
            let name = s.child("w:name").and_then(|n| n.attr("w:val")).unwrap_or(id).to_string();
            let ppr = s.child("w:pPr");
            let heading = style_name_heading_level(&name).or_else(|| {
                ppr.and_then(|p| p.child("w:outlineLvl"))
                    .and_then(|o| o.attr("w:val"))
                    .and_then(|v| v.parse::<u8>().ok())
                    .filter(|l| *l < 9)
                    .map(|l| (l + 1).min(6))
            });
            let num = ppr.and_then(|p| p.child("w:numPr")).and_then(|n| {
                let id = n.child("w:numId")?.attr("w:val")?.to_string();
                let lvl = n.child("w:ilvl").and_then(|l| l.attr("w:val")).and_then(|v| v.parse().ok()).unwrap_or(0);
                Some((id, lvl))
            });
            if s.attr("w:type") == Some("paragraph") && matches!(s.attr("w:default"), Some("1" | "true")) {
                styles.default_para = Some(id.to_string());
            }
            styles.defs.insert(
                id.to_string(),
                StyleDef {
                    name,
                    based_on: s.child("w:basedOn").and_then(|b| b.attr("w:val")).map(str::to_string),
                    heading,
                    num,
                    run: RunProps::from_rpr(s.child("w:rPr")),
                },
            );
        }
        styles
    }

    fn resolve(&self, id: Option<&str>) -> ResolvedStyle {
        let mut out = ResolvedStyle::default();
        let mut cur = id.map(str::to_string).or_else(|| self.default_para.clone());
        let mut first = true;
        for _ in 0..16 {
            let Some(def) = cur.as_ref().and_then(|c| self.defs.get(c)) else { break };
            let lower = def.name.to_lowercase();
            if first {
                // Code/quote semantics come from the style's own name only.
                out.code = ["code", "source", "preformatted", "macro text", "plain text", "verbatim", "listing"]
                    .iter()
                    .any(|k| lower.contains(k));
                out.quote = lower.contains("quote") || lower.contains("citação") || lower.contains("block text");
                if lower.starts_with("list") {
                    let n = lower.rsplit(' ').next().and_then(|d| d.parse::<usize>().ok()).unwrap_or(1);
                    out.list_level_hint = n.clamp(1, 9) - 1;
                }
                first = false;
            }
            out.heading = out.heading.or(def.heading);
            out.num = out.num.clone().or_else(|| def.num.clone());
            out.run = def.run.merged(out.run);
            cur = def.based_on.clone();
        }
        out
    }
}

// ---------------------------------------------------------------------------
// Numbering

#[derive(Clone, Copy, Default)]
struct LevelDef {
    ordered: bool,
    none: bool,
    start: u64,
}

#[derive(Default)]
struct Numbering {
    nums: HashMap<String, (String, HashMap<usize, u64>)>,
    abstracts: HashMap<String, HashMap<usize, LevelDef>>,
}

impl Numbering {
    fn parse(root: &Element) -> Numbering {
        let mut n = Numbering::default();
        let Some(list) = root.child("w:numbering") else { return n };
        for abs in list.children_named("w:abstractNum") {
            let Some(id) = abs.attr("w:abstractNumId") else { continue };
            let mut levels = HashMap::new();
            for lvl in abs.children_named("w:lvl") {
                let Some(ilvl) = lvl.attr("w:ilvl").and_then(|v| v.parse().ok()) else { continue };
                let fmt = lvl.child("w:numFmt").and_then(|f| f.attr("w:val")).unwrap_or("decimal");
                let start =
                    lvl.child("w:start").and_then(|s| s.attr("w:val")).and_then(|v| v.parse().ok()).unwrap_or(1);
                levels.insert(ilvl, LevelDef { ordered: fmt != "bullet" && fmt != "none", none: fmt == "none", start });
            }
            n.abstracts.insert(id.to_string(), levels);
        }
        for num in list.children_named("w:num") {
            let Some(id) = num.attr("w:numId") else { continue };
            let Some(abs) = num.child("w:abstractNumId").and_then(|a| a.attr("w:val")) else { continue };
            let mut overrides = HashMap::new();
            for o in num.children_named("w:lvlOverride") {
                let lvl = o.attr("w:ilvl").and_then(|v| v.parse().ok());
                let start = o.child("w:startOverride").and_then(|s| s.attr("w:val")).and_then(|v| v.parse().ok());
                if let (Some(lvl), Some(start)) = (lvl, start) {
                    overrides.insert(lvl, start);
                }
            }
            n.nums.insert(id.to_string(), (abs.to_string(), overrides));
        }
        n
    }

    fn level(&self, num_id: &str, ilvl: usize) -> Option<LevelDef> {
        let (abs, overrides) = self.nums.get(num_id)?;
        let mut def = self.abstracts.get(abs).and_then(|l| l.get(&ilvl).copied()).unwrap_or(LevelDef {
            ordered: false,
            none: false,
            start: 1,
        });
        if let Some(start) = overrides.get(&ilvl) {
            def.start = *start;
        }
        Some(def)
    }
}

#[cfg(test)]
mod tests {
    use super::hyperlink_from_instr;

    #[test]
    fn parses_hyperlink_fields() {
        assert_eq!(hyperlink_from_instr(" HYPERLINK \"https://a.b/c\" \\o \"tip\""), Some("https://a.b/c".into()));
        assert_eq!(hyperlink_from_instr("HYPERLINK \\l \"_Toc1\""), None);
        assert_eq!(hyperlink_from_instr("PAGE"), None);
    }
}
