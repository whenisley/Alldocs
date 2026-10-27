//! EPUB 2/3 e-books: spine documents in reading order, with EPUB
//! footnotes (`epub:type="noteref"` / `doc-noteref`) turned into GFM footnotes.

use std::collections::HashMap;

use super::html::{HtmlConverter, HtmlHost, LinkTarget};
use crate::model::*;
use crate::package::{Package, resolve, split_dir};
use crate::xml::{self, Element};
use crate::{Ctx, Error, Format, ResourceKind, Result};

pub(crate) fn convert(data: &[u8], ctx: &mut Ctx) -> Result<Document> {
    let mut pkg = Package::open(data, Format::Epub)?;
    let opf_path = pkg
        .read_xml("META-INF/container.xml")
        .and_then(|c| c.find_local("rootfile").and_then(|r| r.attr("full-path")).map(str::to_string))
        .or_else(|| pkg.file_names().find(|n| n.to_ascii_lowercase().ends_with(".opf")).map(str::to_string))
        .ok_or_else(|| Error::invalid(Format::Epub, "no package document (.opf) found"))?;
    let opf = pkg.read_xml(&opf_path).ok_or_else(|| Error::invalid(Format::Epub, "unreadable package document"))?;
    let opf_dir = split_dir(&opf_path).0.to_string();

    if let Some(meta) = opf.find_local("metadata") {
        for el in meta.elements() {
            let value = collapse_whitespace(&el.text()).trim().to_string();
            if value.is_empty() {
                continue;
            }
            let slot = match el.local() {
                "title" => &mut ctx.metadata.title,
                "creator" => &mut ctx.metadata.author,
                "subject" => &mut ctx.metadata.subject,
                "description" => &mut ctx.metadata.description,
                "language" => &mut ctx.metadata.language,
                "date" => &mut ctx.metadata.created,
                _ => continue,
            };
            if slot.is_none() {
                *slot = Some(value);
            }
        }
    }

    // id -> (path, media type, properties)
    let mut manifest: HashMap<String, (String, String, String)> = HashMap::new();
    if let Some(m) = opf.find_local("manifest") {
        for item in m.elements().filter(|e| e.local() == "item") {
            let (Some(id), Some(href)) = (item.attr("id"), item.attr("href")) else { continue };
            manifest.insert(
                id.to_string(),
                (
                    resolve(&opf_dir, href),
                    item.attr("media-type").unwrap_or("").to_string(),
                    item.attr("properties").unwrap_or("").to_string(),
                ),
            );
        }
    }
    let mut chapters: Vec<(String, Element)> = Vec::new();
    if let Some(spine) = opf.find_local("spine") {
        for itemref in spine.elements().filter(|e| e.local() == "itemref") {
            let Some((path, media, props)) = itemref.attr("idref").and_then(|id| manifest.get(id)) else { continue };
            let is_html =
                media.contains("html") || path.ends_with(".xhtml") || path.ends_with(".html") || path.ends_with(".htm");
            if !is_html || props.split_whitespace().any(|p| p == "nav") {
                continue;
            }
            if let Some(bytes) = pkg.read(path) {
                chapters.push((path.clone(), xml::parse_bytes(&bytes, true)));
            }
        }
    }
    if chapters.is_empty() {
        return Err(Error::invalid(Format::Epub, "the spine contains no readable documents"));
    }

    // Pre-pass: find every footnote/endnote element so references to them
    // become GFM footnotes and the note bodies are left out of the text.
    let mut notes: HashMap<String, (String, Element)> = HashMap::new();
    for (path, dom) in &chapters {
        collect_notes(dom, path, false, &mut notes);
    }

    let mut host = EpubHost {
        pkg: &mut pkg,
        ctx,
        base_dir: String::new(),
        path: String::new(),
        notes: &notes,
        labels: HashMap::new(),
        order: Vec::new(),
    };
    let mut blocks = Vec::new();
    for (path, dom) in &chapters {
        host.path = path.clone();
        host.base_dir = split_dir(path).0.to_string();
        let body = dom.find_local("body").unwrap_or(dom);
        blocks.extend(HtmlConverter { host: &mut host }.blocks(body));
    }

    let mut footnotes = Vec::new();
    let mut i = 0;
    while i < host.order.len() {
        let key = host.order[i].clone();
        i += 1;
        let Some((path, el)) = notes.get(&key) else { continue };
        host.path = path.clone();
        host.base_dir = split_dir(path).0.to_string();
        let label = host.labels[&key].clone();
        let note_blocks = HtmlConverter { host: &mut host }.blocks(el);
        footnotes.push(Footnote { label, blocks: strip_leading_marker(note_blocks) });
    }
    Ok(Document { blocks, footnotes })
}

struct EpubHost<'p, 'a, 'c, 'o> {
    pkg: &'p mut Package<'a>,
    ctx: &'c mut Ctx<'o>,
    base_dir: String,
    path: String,
    notes: &'p HashMap<String, (String, Element)>,
    labels: HashMap<String, String>,
    order: Vec<String>,
}

impl HtmlHost for EpubHost<'_, '_, '_, '_> {
    fn image(&mut self, src: &str) -> Option<String> {
        if src.contains("://") || src.starts_with("data:") {
            return Some(src.to_string());
        }
        let path = resolve(&self.base_dir, src);
        let data = self.pkg.read(&path)?;
        Some(self.ctx.add_resource(&path, &path, data, ResourceKind::Image))
    }

    fn link(&mut self, href: &str, el: &Element) -> LinkTarget {
        let types = epub_types(el);
        if types.iter().any(|t| *t == "backlink" || *t == "doc-backlink") {
            return LinkTarget::Remove;
        }
        if href.contains("://") || href.starts_with("mailto:") {
            return LinkTarget::Url(href.to_string());
        }
        let (file, frag) = href.split_once('#').unwrap_or((href, ""));
        let target_path = if file.is_empty() { self.path.clone() } else { resolve(&self.base_dir, file) };
        let key = format!("{target_path}#{frag}");
        if !frag.is_empty() && self.notes.contains_key(&key) {
            let label = match self.labels.get(&key) {
                Some(label) => label.clone(),
                None => {
                    let label = (self.order.len() + 1).to_string();
                    self.labels.insert(key.clone(), label.clone());
                    self.order.push(key);
                    label
                }
            };
            return LinkTarget::Footnote(label);
        }
        if types.iter().any(|t| *t == "noteref" || *t == "doc-noteref") {
            return LinkTarget::Remove;
        }
        LinkTarget::TextOnly
    }

    fn skip(&self, el: &Element) -> bool {
        if epub_types(el).iter().any(|t| *t == "pagebreak" || *t == "doc-pagebreak") {
            return true;
        }
        el.attr("id").is_some_and(|id| self.notes.contains_key(&format!("{}#{id}", self.path)))
    }
}

/// `epub:type` tokens plus `role` (DPUB-ARIA) of an element.
fn epub_types(el: &Element) -> Vec<&str> {
    el.attrs.iter().filter(|(k, _)| k == "epub:type" || k == "role").flat_map(|(_, v)| v.split_whitespace()).collect()
}

const NOTE_TYPES: &[&str] = &["footnote", "endnote", "rearnote", "note", "doc-footnote", "doc-endnote"];
const NOTE_LIST_TYPES: &[&str] = &["footnotes", "endnotes", "rearnotes", "doc-endnotes", "doc-footnotes"];

fn collect_notes(el: &Element, path: &str, in_note_list: bool, out: &mut HashMap<String, (String, Element)>) {
    for child in el.elements() {
        let types = epub_types(child);
        let is_note = types.iter().any(|t| NOTE_TYPES.contains(t)) || (in_note_list && child.local() == "li");
        if is_note && let Some(id) = child.attr("id") {
            out.insert(format!("{path}#{id}"), (path.to_string(), child.clone()));
            continue;
        }
        let list = in_note_list || types.iter().any(|t| NOTE_LIST_TYPES.contains(t));
        collect_notes(child, path, list, out);
    }
}

/// Removes a leading "1." / "[1]" / "1)" marker that repeats the note number.
fn strip_leading_marker(mut blocks: Vec<Block>) -> Vec<Block> {
    if let Some(Block::Paragraph(content)) = blocks.first_mut()
        && let Some(Inline::Text(t)) = content.first_mut()
    {
        let s = t.trim_start();
        let inner = s.strip_prefix(['[', '(']).unwrap_or(s);
        let digits = inner.bytes().take_while(u8::is_ascii_digit).count();
        if (1..=3).contains(&digits)
            && let Some(rest) = inner[digits..].strip_prefix(['.', ')', ']'])
            && (rest.is_empty() || rest.starts_with(' '))
        {
            *t = rest.trim_start().to_string();
        }
    }
    blocks
}
