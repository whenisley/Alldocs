//! Text-based PDF via pdf_oxide (pure Rust).
//!
//! PDF has no paragraphs or headings, only positioned text. Spans are
//! grouped into lines, lines into paragraphs (by spacing, indentation and
//! style), and structure is inferred: larger fonts become headings, bullet
//! or number markers become lists, monospace runs become code, repeated
//! page headers/footers and page numbers are dropped, and words hyphenated
//! across lines are rejoined. Scanned (image-only) pages have no text layer;
//! OCR is out of scope and they are reported in `warnings`.

use std::collections::HashMap;

use pdf_oxide::PdfDocument;
use pdf_oxide::extractors::ImageData;
use pdf_oxide::layout::TextSpan;

use super::is_monospace_font;
use crate::model::*;
use crate::runs::{Item, Para, ParaKind, RunBuilder, Style, assemble};
use crate::{Ctx, Error, Format, ImageMode, ResourceKind, Result};

#[derive(Debug, Clone)]
struct Span {
    text: String,
    style: Style,
    size: f32,
    right: f32,
}

#[derive(Debug, Clone)]
struct Line {
    spans: Vec<Span>,
    page: usize,
    top: f32,
    bottom: f32,
    left: f32,
    right: f32,
    size: f32,
}

impl Line {
    fn text(&self) -> String {
        self.spans.iter().map(|s| s.text.as_str()).collect()
    }
    fn all(&self, f: impl Fn(&Style) -> bool) -> bool {
        let visible: Vec<&Span> = self.spans.iter().filter(|s| !s.text.trim().is_empty()).collect();
        !visible.is_empty() && visible.iter().all(|s| f(&s.style))
    }
}

struct PageImage {
    page: usize,
    top: Option<f32>,
    src: String,
}

pub(crate) fn convert(data: &[u8], ctx: &mut Ctx) -> Result<Document> {
    let doc = PdfDocument::from_bytes(data.to_vec()).map_err(|e| Error::invalid(Format::Pdf, e))?;
    let page_count = doc.page_count().map_err(|e| Error::invalid(Format::Pdf, e))?;
    ctx.metadata.pages = Some(page_count);

    let mut pages: Vec<Vec<Line>> = Vec::with_capacity(page_count);
    let mut heights = Vec::with_capacity(page_count);
    let mut images = Vec::new();
    let mut empty_pages = Vec::new();
    for p in 0..page_count {
        let (y0, y1) = doc.get_page_media_box(p).map(|(_, y0, _, y1)| (y0.min(y1), y0.max(y1))).unwrap_or((0.0, 792.0));
        heights.push((y0, y1));
        let spans = match doc.extract_spans(p) {
            Ok(s) => s,
            Err(e) => {
                ctx.warn(format!("page {}: text extraction failed: {e}", p + 1));
                Vec::new()
            }
        };
        let lines = group_lines(&spans, p);
        if lines.is_empty() {
            empty_pages.push(p + 1);
        }
        pages.push(lines);

        if ctx.opts.images != ImageMode::Omit {
            match doc.extract_images(p) {
                Ok(list) => {
                    for (k, img) in list.into_iter().enumerate() {
                        if img.width() < 24 || img.height() < 24 {
                            continue;
                        }
                        let (bytes, ext) = match img.data() {
                            ImageData::Jpeg(bytes) => (bytes.clone(), "jpg"),
                            _ => match img.to_png_bytes() {
                                Ok(png) => (png, "png"),
                                Err(_) => continue,
                            },
                        };
                        let name = format!("page{}-image{}.{ext}", p + 1, k + 1);
                        let src = ctx.add_resource(&format!("pdf:{p}:{k}"), &name, bytes, ResourceKind::Image);
                        let top = img.bbox().map(|b| b.y + b.height);
                        images.push(PageImage { page: p, top, src });
                    }
                }
                Err(e) => ctx.warn(format!("page {}: image extraction failed: {e}", p + 1)),
            }
        }
    }

    if page_count > 0 && empty_pages.len() == page_count {
        ctx.warn("no text layer found: this looks like a scanned or image-only PDF (OCR is not included)");
    } else if !empty_pages.is_empty() {
        let list: Vec<String> = empty_pages.iter().map(|p| p.to_string()).collect();
        ctx.warn(format!("pages without text (possibly scanned): {}", list.join(", ")));
    }

    remove_running_lines(&mut pages, &heights);
    let lines: Vec<Line> = pages.into_iter().flatten().collect();
    let items = build_items(&lines, images);
    Ok(Document { blocks: assemble(items), footnotes: Vec::new() })
}

fn span_style(span: &TextSpan) -> Style {
    let name = span.font_name.to_ascii_lowercase();
    Style {
        bold: span.font_weight.is_bold() || name.contains("bold") || name.contains("black") || name.contains("heavy"),
        italic: span.is_italic || name.contains("italic") || name.contains("oblique"),
        strike: false,
        code: span.is_monospace || is_monospace_font(&span.font_name),
    }
}

/// Groups spans (already in reading order) into visual lines.
fn group_lines(spans: &[TextSpan], page: usize) -> Vec<Line> {
    let mut lines: Vec<Line> = Vec::new();
    for s in spans {
        if s.artifact_type.is_some() || s.text.is_empty() {
            continue;
        }
        let (bottom, top) = (s.bbox.y, s.bbox.y + s.bbox.height);
        let (left, right) = (s.bbox.x, s.bbox.x + s.bbox.width);
        let size = if s.font_size > 0.0 { s.font_size } else { s.bbox.height.max(1.0) };
        let span = Span { text: clean_text(&s.text), style: span_style(s), size, right };
        if let Some(line) = lines.last_mut() {
            let center = (top + bottom) / 2.0;
            let line_center = (line.top + line.bottom) / 2.0;
            let tolerance = 0.5 * size.max(line.size);
            if (center - line_center).abs() < tolerance && left > line.left - size {
                let prev = line.spans.last().expect("non-empty line");
                let gap = left - prev.right;
                if gap > 0.15 * size
                    && !prev.text.ends_with(char::is_whitespace)
                    && !span.text.starts_with(char::is_whitespace)
                {
                    line.spans.push(Span { text: " ".into(), style: prev.style, size, right: left });
                }
                line.top = line.top.max(top);
                line.bottom = line.bottom.min(bottom);
                line.left = line.left.min(left);
                line.right = line.right.max(right);
                if span.text.trim().len() > 1 {
                    line.size = line.size.max(size);
                }
                line.spans.push(span);
                continue;
            }
        }
        lines.push(Line { spans: vec![span], page, top, bottom, left, right, size });
    }
    lines.retain(|l| !l.text().trim().is_empty());
    for line in &mut lines {
        // Use the dominant (most characters) font size as the line size.
        let mut by_size: HashMap<i32, usize> = HashMap::new();
        for s in &line.spans {
            *by_size.entry((s.size * 2.0).round() as i32).or_default() += s.text.trim().chars().count();
        }
        if let Some((sz, _)) = by_size.into_iter().max_by_key(|(sz, n)| (*n, *sz)) {
            line.size = sz as f32 / 2.0;
        }
    }
    lines
}

/// Maps the codes that WinAnsiEncoding leaves undefined (rendered as a
/// bullet by PDF viewers, per the PDF spec) to `•`, and drops other
/// control characters.
fn clean_text(text: &str) -> String {
    text.chars()
        .filter_map(|c| match c {
            '\u{7f}' | '\u{81}' | '\u{8d}' | '\u{8f}' | '\u{90}' | '\u{9d}' => Some('•'),
            '\t' => Some(' '),
            c if c.is_control() => None,
            c => Some(c),
        })
        .collect()
}

/// Drops page headers/footers that repeat across pages, and page numbers.
fn remove_running_lines(pages: &mut [Vec<Line>], heights: &[(f32, f32)]) {
    let n = pages.len();
    let in_margin = |line: &Line| {
        let (y0, y1) = heights[line.page];
        let h = (y1 - y0).max(1.0);
        line.bottom > y1 - 0.08 * h || line.top < y0 + 0.08 * h
    };
    let key = |line: &Line| -> String {
        line.text()
            .chars()
            .map(|c| if c.is_ascii_digit() { '#' } else { c.to_ascii_lowercase() })
            .filter(|c| !c.is_whitespace())
            .collect()
    };
    let mut counts: HashMap<String, usize> = HashMap::new();
    for page in pages.iter() {
        let mut seen = std::collections::HashSet::new();
        for line in page.iter().filter(|l| in_margin(l)) {
            let k = key(line);
            if seen.insert(k.clone()) {
                *counts.entry(k).or_default() += 1;
            }
        }
    }
    let threshold = (n as f64 * 0.5).ceil().max(2.0) as usize;
    for page in pages.iter_mut() {
        page.retain(|line| {
            if !in_margin(line) {
                return true;
            }
            let text = line.text();
            if is_page_number(text.trim()) {
                return false;
            }
            !(n >= 3 && counts.get(&key(line)).is_some_and(|c| *c >= threshold))
        });
    }
}

fn is_page_number(text: &str) -> bool {
    let lower = text.to_lowercase();
    let stripped = lower
        .trim_matches(|c: char| c == '-' || c == '–' || c == '|' || c.is_whitespace())
        .trim_start_matches("page")
        .trim_start_matches("página")
        .trim_start_matches("pág.")
        .trim();
    let mut parts =
        stripped.split(|c: char| c == '/' || c.is_whitespace()).filter(|p| !p.is_empty() && *p != "of" && *p != "de");
    let first = parts.next();
    first.is_some_and(|p| p.chars().all(|c| c.is_ascii_digit()) || is_roman(p))
        && parts.all(|p| p.chars().all(|c| c.is_ascii_digit()))
}

fn is_roman(s: &str) -> bool {
    !s.is_empty() && s.len() <= 6 && s.chars().all(|c| "ivxlcdm".contains(c))
}

const BULLETS: &[char] = &[
    '•', '●', '○', '◦', '▪', '■', '□', '▫', '–', '-', '*', '➢', '►', '▶', '✓', '✔', '·', '\u{f0b7}', '\u{f0a7}',
    '\u{f076}', '\u{f0d8}',
];

/// Returns (ordered, start, marker byte length) if the line starts with a list marker.
fn list_marker(text: &str) -> Option<(bool, u64, usize)> {
    let t = text.trim_start();
    let offset = text.len() - t.len();
    let first = t.chars().next()?;
    if BULLETS.contains(&first) {
        let rest = &t[first.len_utf8()..];
        if rest.starts_with(char::is_whitespace) || (!matches!(first, '-' | '*' | '–') && !rest.is_empty()) {
            return Some((false, 1, offset + first.len_utf8()));
        }
        return None;
    }
    let digits = t.bytes().take_while(u8::is_ascii_digit).count();
    let (ordered_len, start) = if (1..=3).contains(&digits) {
        (digits, t[..digits].parse().unwrap_or(1))
    } else if t.len() > 1 && t.as_bytes()[0].is_ascii_lowercase() && matches!(t.as_bytes()[1], b'.' | b')') {
        (1, 1)
    } else {
        return None;
    };
    let after = &t[ordered_len..];
    if (after.starts_with(". ") || after.starts_with(") ")) && after.len() > 2 {
        return Some((true, start, offset + ordered_len + 1));
    }
    None
}

fn round_half(x: f32) -> i32 {
    (x * 2.0).round() as i32
}

fn build_items(lines: &[Line], mut images: Vec<PageImage>) -> Vec<Item> {
    if lines.is_empty() {
        return images.into_iter().map(|img| image_item(img.src)).collect();
    }

    // Body text size: the size covering the most characters.
    let mut size_chars: HashMap<i32, usize> = HashMap::new();
    for l in lines {
        *size_chars.entry(round_half(l.size)).or_default() += l.text().chars().count();
    }
    let body = size_chars.iter().max_by_key(|(sz, n)| (**n, -**sz)).map(|(sz, _)| *sz as f32 / 2.0).unwrap_or(10.0);

    // Heading levels: distinct sizes clearly larger than body text, largest first.
    let mut heading_sizes: Vec<i32> =
        size_chars.keys().copied().filter(|sz| (*sz as f32 / 2.0) >= body * 1.15 + 0.5).collect();
    heading_sizes.sort_unstable_by(|a, b| b.cmp(a));
    let heading_level = |l: &Line| -> Option<u8> {
        let text = l.text();
        if text.chars().count() > 200 {
            return None;
        }
        let pos = heading_sizes.iter().position(|sz| *sz == round_half(l.size))?;
        Some((pos as u8 + 1).min(4))
    };
    let bold_heading_level = (heading_sizes.len().min(3) as u8 + 1).min(6);

    // Typical baseline-to-baseline distance, for paragraph break detection.
    let mut spacing: HashMap<i32, usize> = HashMap::new();
    for w in lines.windows(2) {
        if w[0].page == w[1].page && round_half(w[0].size) == round_half(w[1].size) {
            let dy = w[0].bottom - w[1].bottom;
            if dy > 0.0 && dy < w[0].size * 3.0 {
                *spacing.entry(round_half(dy)).or_default() += 1;
            }
        }
    }
    let leading = spacing.into_iter().max_by_key(|(d, n)| (*n, -*d)).map(|(d, _)| d as f32 / 2.0).unwrap_or(body * 1.2);

    // Right edge of the text column per page (to spot short last lines).
    let mut page_right: HashMap<usize, f32> = HashMap::new();
    for l in lines {
        let r = page_right.entry(l.page).or_insert(0.0);
        *r = r.max(l.right);
    }

    // Split lines into paragraphs.
    let mut paras: Vec<Vec<&Line>> = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let new_para = match i.checked_sub(1).map(|p| &lines[p]) {
            None => true,
            Some(prev) => is_break(prev, line, &heading_level, leading, &page_right),
        };
        if new_para {
            paras.push(vec![line]);
        } else {
            paras.last_mut().expect("paragraph").push(line);
        }
    }

    images.sort_by_key(|img| img.page);
    let mut images = images.into_iter().peekable();
    let mut items = Vec::new();
    let mut list_base_left: Option<f32> = None;

    for para in paras {
        let first = para[0];
        // Emit images that sit above this paragraph on the same page (or on earlier pages).
        while let Some(img) = images.peek() {
            let before = img.page < first.page || (img.page == first.page && img.top.is_some_and(|t| t >= first.top));
            if !before {
                break;
            }
            let img = images.next().expect("peeked");
            items.push(image_item(img.src));
        }

        let text = first.text();
        if let Some(level) = heading_level(first) {
            list_base_left = None;
            items.push(Item::Para(Para { kind: ParaKind::Heading(level), inlines: lines_inlines(&para, 0) }));
            continue;
        }
        if para.iter().all(|l| l.all(|s| s.code)) {
            list_base_left = None;
            let min_left = para.iter().map(|l| l.left).fold(f32::MAX, f32::min);
            for l in &para {
                let char_w = (l.size * 0.6).max(1.0);
                let indent = ((l.left - min_left) / char_w).round().max(0.0) as usize;
                items.push(Item::Para(Para {
                    kind: ParaKind::Code,
                    inlines: vec![Inline::Text(format!("{}{}", " ".repeat(indent), l.text().trim_end()))],
                }));
            }
            continue;
        }
        if let Some((ordered, start, marker_len)) = list_marker(&text) {
            let base = *list_base_left.get_or_insert(first.left);
            let level = ((first.left - base) / (body * 1.5)).round().clamp(0.0, 6.0) as usize;
            if first.left < base {
                list_base_left = Some(first.left);
            }
            items.push(Item::Para(Para {
                kind: ParaKind::ListItem { level, ordered, start, list_id: "pdf".into() },
                inlines: lines_inlines(&para, marker_len),
            }));
            continue;
        }
        list_base_left = None;
        let short_bold = para.len() == 1
            && first.all(|s| s.bold)
            && text.trim().chars().count() < 80
            && !text.trim_end().ends_with(['.', ',', ';', ':']);
        let kind = if short_bold { ParaKind::Heading(bold_heading_level) } else { ParaKind::Normal };
        let inlines = lines_inlines(&para, 0);
        let inlines = if short_bold { strip_strong(inlines) } else { inlines };
        items.push(Item::Para(Para { kind, inlines }));
    }
    items.extend(images.map(|img| image_item(img.src)));
    items
}

fn image_item(src: String) -> Item {
    Item::Para(Para { kind: ParaKind::Normal, inlines: vec![Inline::Image { src, alt: String::new(), title: None }] })
}

fn strip_strong(inlines: Vec<Inline>) -> Vec<Inline> {
    inlines
        .into_iter()
        .flat_map(|i| match i {
            Inline::Strong(c) => c,
            other => vec![other],
        })
        .collect()
}

fn ends_sentence(text: &str) -> bool {
    text.trim_end().ends_with(['.', '!', '?', ':', ';', '"', '”', ')'])
}

fn is_break(
    prev: &Line,
    line: &Line,
    heading_level: &dyn Fn(&Line) -> Option<u8>,
    leading: f32,
    page_right: &HashMap<usize, f32>,
) -> bool {
    let prev_text = prev.text();
    let text = line.text();
    let starts_lower = text.trim_start().chars().next().is_some_and(char::is_lowercase);

    if heading_level(prev) != heading_level(line) {
        return true;
    }
    if heading_level(line).is_some() {
        // Multi-line headings: same size, close together.
        return line.page != prev.page || prev.bottom - line.bottom > leading * 1.6;
    }
    if list_marker(&text).is_some() {
        return true;
    }
    if prev.all(|s| s.code) != line.all(|s| s.code) {
        return true;
    }
    if line.page != prev.page {
        // A paragraph continues across the page break when it was cut mid-sentence.
        return ends_sentence(&prev_text) || !starts_lower;
    }
    let dy = prev.bottom - line.bottom;
    if dy <= 0.0 {
        return true; // new column
    }
    if dy > leading * 1.35 + 0.5 {
        return true;
    }
    if (round_half(prev.size) - round_half(line.size)).abs() > 2 {
        return true;
    }
    let right = page_right.get(&prev.page).copied().unwrap_or(prev.right);
    let width = (right - prev.left).max(1.0);
    let short = prev.right < right - width * 0.15;
    if short && ends_sentence(&prev_text) && !starts_lower {
        return true;
    }
    if line.left > prev.left + line.size * 1.5 && ends_sentence(&prev_text) {
        return true; // indented first line
    }
    false
}

/// Joins the lines of a paragraph into inlines, removing a leading list
/// marker and rejoining words hyphenated across line ends.
fn lines_inlines(para: &[&Line], marker_len: usize) -> Vec<Inline> {
    let mut rb = RunBuilder::new();
    // Span boundaries are char boundaries of the line text, so cutting
    // `marker_len` bytes span by span never splits a character.
    let mut skip = marker_len;
    for (i, line) in para.iter().enumerate() {
        let next_starts_lower =
            para.get(i + 1).and_then(|n| n.text().trim_start().chars().next()).is_some_and(char::is_lowercase);
        let last = line.spans.len() - 1;
        for (j, span) in line.spans.iter().enumerate() {
            let mut text = span.text.as_str();
            if skip > 0 {
                let cut = skip.min(text.len());
                text = text[cut..].trim_start();
                skip -= cut;
            }
            if j == last && i + 1 < para.len() {
                let trimmed = text.trim_end();
                let hyphenated = trimmed.ends_with('-')
                    && trimmed.chars().rev().nth(1).is_some_and(char::is_alphabetic)
                    && next_starts_lower;
                let joined = if hyphenated { trimmed[..trimmed.len() - 1].to_string() } else { format!("{trimmed} ") };
                rb.text(&joined, span.style, None);
            } else {
                rb.text(text, span.style, None);
            }
        }
    }
    rb.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_markers_and_page_numbers() {
        assert_eq!(list_marker("• item"), Some((false, 1, 3)));
        assert_eq!(list_marker("2. second"), Some((true, 2, 2)));
        assert_eq!(list_marker("a) alpha"), Some((true, 1, 2)));
        assert_eq!(list_marker("2024. A year"), None);
        assert_eq!(list_marker("-5 degrees"), None);
        assert!(is_page_number("12"));
        assert!(is_page_number("Page 3 of 10"));
        assert!(is_page_number("- iv -"));
        assert!(!is_page_number("Chapter 1"));
    }
}
