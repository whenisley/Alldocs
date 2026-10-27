//! PowerPoint (.pptx) — PresentationML.
//!
//! Each slide becomes `<!-- Slide N -->` followed by a `##` heading (the
//! slide title) and its content in reading order; speaker notes follow under
//! a `### Notes` heading.

use super::is_monospace_font;
use crate::model::*;
use crate::package::{Package, Rels};
use crate::runs::{Item, Para, ParaKind, RunBuilder, Style, assemble};
use crate::xml::Element;
use crate::{Ctx, Error, Format, ResourceKind, Result};

pub(crate) fn convert(data: &[u8], ctx: &mut Ctx) -> Result<Document> {
    let mut pkg = Package::open(data, Format::Pptx)?;
    pkg.read_core_properties(&mut ctx.metadata);
    let main = pkg.main_part("officeDocument", "ppt/presentation.xml");
    let pres = pkg.read_xml(&main).ok_or_else(|| Error::invalid(Format::Pptx, "missing ppt/presentation.xml"))?;
    let rels = pkg.rels(&main);

    let slides: Vec<String> = pres
        .find("p:sldIdLst")
        .map(|list| {
            list.children_named("p:sldId")
                .filter_map(|s| s.attr("r:id"))
                .filter_map(|id| rels.get(id))
                .map(|r| r.target.clone())
                .collect()
        })
        .unwrap_or_default();
    ctx.metadata.pages = Some(slides.len());

    let mut conv = Pptx { pkg, ctx, rels: Rels::default() };
    let mut blocks = Vec::new();
    for (i, part) in slides.iter().enumerate() {
        let Some(slide) = conv.pkg.read_xml(part) else { continue };
        let Some(sld) = slide.child("p:sld") else { continue };
        let hidden = sld.attr("show") == Some("0");
        if hidden && !conv.ctx.opts.include_hidden {
            continue;
        }
        conv.rels = conv.pkg.rels(part);
        let number = i + 1;
        blocks.push(Block::Comment(if hidden {
            format!("Slide {number} (hidden)")
        } else {
            format!("Slide {number}")
        }));

        let shapes = sld.path(&["p:cSld", "p:spTree"]).map(collect_shapes).unwrap_or_default();
        let title = shapes
            .iter()
            .find(|s| matches!(s.placeholder.as_deref(), Some("title" | "ctrTitle")))
            .map(|s| title_text(s.el))
            .filter(|t| !t.is_empty());
        blocks.push(Block::Heading {
            level: 2,
            content: vec![Inline::Text(title.clone().unwrap_or_else(|| format!("Slide {number}")))],
        });

        for shape in &shapes {
            if matches!(shape.placeholder.as_deref(), Some("title" | "ctrTitle")) && title.is_some() {
                continue;
            }
            blocks.extend(conv.shape(shape));
        }

        if conv.ctx.opts.include_notes {
            let notes_part = conv.rels.0.values().find(|r| r.kind == "notesSlide").map(|r| r.target.clone());
            if let Some(notes) = notes_part.and_then(|p| conv.pkg.read_xml(&p)) {
                let note_blocks: Vec<Block> = notes
                    .find("p:spTree")
                    .map(collect_shapes)
                    .unwrap_or_default()
                    .iter()
                    .filter(|s| s.placeholder.as_deref() == Some("body"))
                    .flat_map(|s| s.el.child("p:txBody").map(|b| conv.text_body(b, false, "notes")).unwrap_or_default())
                    .collect();
                if !note_blocks.is_empty() {
                    blocks.push(Block::Heading { level: 3, content: vec![Inline::text("Notes")] });
                    blocks.extend(note_blocks);
                }
            }
        }
    }
    Ok(Document { blocks, footnotes: Vec::new() })
}

struct Pptx<'a, 'c, 'o> {
    pkg: Package<'a>,
    ctx: &'c mut Ctx<'o>,
    rels: Rels,
}

struct Shape<'e> {
    el: &'e Element,
    placeholder: Option<String>,
    pos: Option<(i64, i64)>,
}

/// Collects drawable shapes in reading order (top-to-bottom, left-to-right
/// when every shape has a position; document order otherwise).
fn collect_shapes(tree: &Element) -> Vec<Shape<'_>> {
    let mut shapes = Vec::new();
    collect_into(tree, None, &mut shapes);
    let positioned =
        shapes.iter().all(|s| s.pos.is_some() || matches!(s.placeholder.as_deref(), Some("title" | "ctrTitle")));
    if positioned {
        shapes.sort_by_key(|s| {
            let is_title = matches!(s.placeholder.as_deref(), Some("title" | "ctrTitle"));
            let (x, y) = s.pos.unwrap_or((0, 0));
            // Shapes whose tops are within ~0.1" share a row.
            (!is_title, y / 100_000, x)
        });
    }
    shapes
}

fn collect_into<'e>(container: &'e Element, group_pos: Option<(i64, i64)>, out: &mut Vec<Shape<'e>>) {
    for el in container.elements() {
        match el.name.as_str() {
            "p:sp" | "p:pic" | "p:graphicFrame" => {
                let placeholder = el.find("p:ph").map(|ph| ph.attr("type").unwrap_or("body").to_string());
                let pos = group_pos.or_else(|| offset(el));
                out.push(Shape { el, placeholder, pos });
            }
            "p:grpSp" => collect_into(el, group_pos.or_else(|| offset(el)), out),
            "mc:AlternateContent" => {
                if let Some(choice) = el.child("mc:Choice") {
                    collect_into(choice, group_pos, out);
                }
            }
            _ => {}
        }
    }
}

fn offset(shape: &Element) -> Option<(i64, i64)> {
    let xfrm = shape
        .child("p:spPr")
        .or_else(|| shape.child("p:grpSpPr"))
        .and_then(|p| p.child("a:xfrm"))
        .or_else(|| shape.child("p:xfrm"))?;
    let off = xfrm.child("a:off")?;
    Some((off.attr("x")?.parse().ok()?, off.attr("y")?.parse().ok()?))
}

fn title_text(shape: &Element) -> String {
    let Some(body) = shape.child("p:txBody") else { return String::new() };
    let lines: Vec<String> = body
        .children_named("a:p")
        .map(|p| {
            let mut ts = Vec::new();
            p.find_all("a:t", &mut ts);
            ts.iter().map(|t| t.text()).collect::<String>()
        })
        .filter(|t| !t.trim().is_empty())
        .collect();
    crate::model::collapse_whitespace(&lines.join(" ")).trim().to_string()
}

impl<'a, 'c, 'o> Pptx<'a, 'c, 'o> {
    fn shape(&mut self, shape: &Shape<'_>) -> Vec<Block> {
        let el = shape.el;
        match el.name.as_str() {
            "p:sp" => {
                if matches!(shape.placeholder.as_deref(), Some("dt" | "ftr" | "sldNum" | "hdr")) {
                    return Vec::new();
                }
                let bulleted = shape.placeholder.as_deref().is_some_and(|t| matches!(t, "body" | "obj"));
                let id = el.find("p:cNvPr").and_then(|c| c.attr("id")).unwrap_or("0").to_string();
                el.child("p:txBody").map(|b| self.text_body(b, bulleted, &id)).unwrap_or_default()
            }
            "p:pic" => self.picture(el).into_iter().map(|i| Block::Paragraph(vec![i])).collect(),
            "p:graphicFrame" => {
                if let Some(tbl) = el.find("a:tbl") {
                    return vec![self.table(tbl)];
                }
                let mut out = Vec::new();
                if let Some(ole) = el.find("p:oleObj") {
                    if let Some(pic) = ole.find("p:pic") {
                        out.extend(self.picture(pic).into_iter().map(|i| Block::Paragraph(vec![i])));
                    }
                    if let Some(path) = ole.attr("r:id").and_then(|id| self.resource_for(id, ResourceKind::Object)) {
                        let prog = ole.attr("progId").unwrap_or("object");
                        out.push(Block::Paragraph(vec![Inline::Link {
                            url: path,
                            title: None,
                            content: vec![Inline::text(format!("Embedded {prog}"))],
                        }]));
                    }
                }
                out
            }
            _ => Vec::new(),
        }
    }

    fn picture(&mut self, pic: &Element) -> Option<Inline> {
        let rel_id = pic.find("a:blip").and_then(|b| b.attr("r:embed").or_else(|| b.attr("r:link")))?;
        let src = self.resource_for(rel_id, ResourceKind::Image)?;
        let alt = pic
            .find("p:cNvPr")
            .and_then(|c| c.attr("descr").filter(|d| !d.is_empty()).or_else(|| c.attr("title")))
            .unwrap_or("")
            .to_string();
        Some(Inline::Image { src, alt, title: None })
    }

    fn resource_for(&mut self, rel_id: &str, kind: ResourceKind) -> Option<String> {
        let rel = self.rels.get(rel_id)?.clone();
        if rel.external {
            return Some(rel.target);
        }
        let data = self.pkg.read(&rel.target)?;
        Some(self.ctx.add_resource(&rel.target, &rel.target, data, kind))
    }

    fn text_body(&mut self, body: &Element, bulleted: bool, shape_id: &str) -> Vec<Block> {
        let mut items = Vec::new();
        for p in body.children_named("a:p") {
            let ppr = p.child("a:pPr");
            let level: usize = ppr.and_then(|x| x.attr("lvl")).and_then(|v| v.parse().ok()).unwrap_or(0);
            let auto_num = ppr.and_then(|x| x.child("a:buAutoNum"));
            let bullet = if ppr.is_some_and(|x| x.child("a:buNone").is_some()) {
                None
            } else if let Some(num) = auto_num {
                let start = num.attr("startAt").and_then(|s| s.parse().ok()).unwrap_or(1);
                Some((true, start))
            } else if bulleted || ppr.is_some_and(|x| x.child("a:buChar").is_some() || x.child("a:buBlip").is_some()) {
                Some((false, 1))
            } else {
                None
            };

            let mut rb = RunBuilder::new();
            for child in p.elements() {
                match child.name.as_str() {
                    "a:r" | "a:fld" => {
                        let rpr = child.child("a:rPr");
                        let style = Style {
                            bold: rpr.and_then(|r| r.attr("b")).is_some_and(|v| v == "1" || v == "true"),
                            italic: rpr.and_then(|r| r.attr("i")).is_some_and(|v| v == "1" || v == "true"),
                            strike: rpr.and_then(|r| r.attr("strike")).is_some_and(|v| v != "noStrike"),
                            code: rpr
                                .and_then(|r| r.child("a:latin"))
                                .and_then(|l| l.attr("typeface"))
                                .is_some_and(is_monospace_font),
                        };
                        let url = rpr
                            .and_then(|r| r.child("a:hlinkClick"))
                            .and_then(|h| h.attr("r:id"))
                            .and_then(|id| self.rels.get(id))
                            .filter(|r| r.external)
                            .map(|r| r.target.clone());
                        let text = child.child("a:t").map(|t| t.text()).unwrap_or_default();
                        rb.text(&text, style, url.as_deref());
                    }
                    "a:br" => rb.line_break(),
                    _ => {}
                }
            }
            let kind = match bullet {
                Some((ordered, start)) if rb.has_content() => {
                    ParaKind::ListItem { level, ordered, start, list_id: format!("{shape_id}:{level}") }
                }
                _ if rb.is_all_code() => ParaKind::Code,
                _ => ParaKind::Normal,
            };
            let inlines = if kind == ParaKind::Code { vec![Inline::Text(rb.plain_text())] } else { rb.finish() };
            items.push(Item::Para(Para { kind, inlines }));
        }
        assemble(items)
    }

    fn table(&mut self, tbl: &Element) -> Block {
        let mut rows = Vec::new();
        for tr in tbl.children_named("a:tr") {
            let mut row = Vec::new();
            for tc in tr.children_named("a:tc") {
                let merged = tc.attr("hMerge") == Some("1") || tc.attr("vMerge") == Some("1");
                let content = if merged {
                    Vec::new()
                } else {
                    tc.child("a:txBody")
                        .map(|b| blocks_to_inlines(&self.text_body(b, false, "table")))
                        .unwrap_or_default()
                };
                row.push(content);
            }
            rows.push(row);
        }
        Block::Table(Table { rows, alignments: Vec::new() })
    }
}
