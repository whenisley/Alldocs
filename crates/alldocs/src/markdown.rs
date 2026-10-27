//! Renders the document model as GitHub Flavored Markdown.

use crate::model::*;
use unicode_width::UnicodeWidthStr;

/// Maximum column width used when padding table cells for readability.
const MAX_PAD_WIDTH: usize = 48;

/// Renders a document to GFM. Output ends with exactly one newline (or is empty).
pub fn render(doc: &Document) -> String {
    let mut out = blocks_to_md(&doc.blocks);

    if !doc.footnotes.is_empty() {
        let notes: Vec<String> = doc
            .footnotes
            .iter()
            .map(|note| {
                let body = blocks_to_md(&note.blocks);
                let label = footnote_label(&note.label);
                indent_continuation(&format!("[^{label}]: "), "    ", &body)
            })
            .collect();
        let sep = if notes.iter().all(|n| !n.contains('\n')) { "\n" } else { "\n\n" };
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        out.push_str(&notes.join(sep));
    }

    let trimmed_len = out.trim_end().len();
    out.truncate(trimmed_len);
    if !out.is_empty() {
        out.push('\n');
    }
    out
}

/// Renders inlines as a single Markdown line (no block-level escaping).
pub fn render_inlines(inlines: &[Inline]) -> String {
    let normalized = normalize(inlines.to_vec());
    let mut out = String::new();
    write_inlines(&normalized, false, &mut out);
    out
}

fn blocks_to_md(blocks: &[Block]) -> String {
    blocks_joined(blocks, "\n\n")
}

fn blocks_joined(blocks: &[Block], sep: &str) -> String {
    let mut out = String::new();
    // Two adjacent lists of the same kind would merge in Markdown, so the
    // second one switches marker style (`-` vs `*`, `.` vs `)`).
    let mut prev_list_kind: Option<(bool, bool)> = None;
    for block in blocks {
        let mut alt = false;
        if let Block::List(list) = block
            && let Some((ordered, prev_alt)) = prev_list_kind
            && ordered == list.ordered
        {
            alt = !prev_alt;
        }
        let rendered = block_to_md(block, alt);
        if rendered.is_empty() {
            continue;
        }
        prev_list_kind = match block {
            Block::List(list) => Some((list.ordered, alt)),
            _ => None,
        };
        if !out.is_empty() {
            out.push_str(sep);
        }
        out.push_str(&rendered);
    }
    out
}

fn block_to_md(block: &Block, alt_marker: bool) -> String {
    match block {
        Block::Heading { level, content } => {
            // A heading that is bold as a whole is just styled; drop the markers.
            let content = match normalize(content.clone()).as_slice() {
                [Inline::Strong(inner)] => inner.clone(),
                other => other.to_vec(),
            };
            let text = render_inlines(&content).replace("\\\n", " ").replace('\n', " ");
            let text = text.trim();
            if text.is_empty() {
                return String::new();
            }
            let mut text = text.to_string();
            if text.ends_with('#') {
                let pos = text.trim_end_matches('#').len();
                text.insert(pos, '\\');
            }
            format!("{} {}", "#".repeat((*level).clamp(1, 6) as usize), text)
        }
        Block::Paragraph(content) => {
            let text = render_inlines(content);
            escape_line_starts(&text)
        }
        Block::List(list) => list_to_md(list, alt_marker),
        Block::Table(table) => table_to_md(table),
        Block::CodeBlock { language, code } => code_block_to_md(language.as_deref(), code),
        Block::BlockQuote(inner) => {
            let body = blocks_to_md(inner);
            if body.is_empty() {
                return String::new();
            }
            body.lines()
                .map(|l| if l.is_empty() { ">".to_string() } else { format!("> {l}") })
                .collect::<Vec<_>>()
                .join("\n")
        }
        Block::ThematicBreak => "---".to_string(),
        Block::Comment(text) => {
            let text = text.replace("--", "- -");
            format!("<!-- {} -->", text.trim())
        }
    }
}

fn list_to_md(list: &List, alt_marker: bool) -> String {
    // A list is "tight" when every item is a single paragraph, optionally
    // followed by nested lists.
    let tight = list.items.iter().all(|item| {
        item.blocks
            .iter()
            .enumerate()
            .all(|(i, b)| matches!(b, Block::List(_)) || (i == 0 && matches!(b, Block::Paragraph(_))))
    });
    let mut items = Vec::with_capacity(list.items.len());
    for (i, item) in list.items.iter().enumerate() {
        let marker = if list.ordered {
            format!("{}{}", list.start + i as u64, if alt_marker { ')' } else { '.' })
        } else {
            (if alt_marker { "*" } else { "-" }).to_string()
        };
        let mut body = blocks_joined(&item.blocks, if tight { "\n" } else { "\n\n" });
        if let Some(checked) = item.checked {
            body = format!("[{}] {}", if checked { 'x' } else { ' ' }, body);
        }
        let indent = " ".repeat(marker.len() + 1);
        if body.is_empty() {
            items.push(marker);
        } else {
            items.push(indent_continuation(&format!("{marker} "), &indent, &body));
        }
    }
    items.join(if tight { "\n" } else { "\n\n" })
}

/// Prefixes the first line with `first` and the remaining non-empty lines with `rest`.
fn indent_continuation(first: &str, rest: &str, body: &str) -> String {
    let mut out = String::with_capacity(body.len() + first.len());
    for (i, line) in body.split('\n').enumerate() {
        if i == 0 {
            out.push_str(first);
            out.push_str(line);
        } else {
            out.push('\n');
            if !line.is_empty() {
                out.push_str(rest);
                out.push_str(line);
            }
        }
    }
    out
}

fn table_to_md(table: &Table) -> String {
    let ncols = table.rows.iter().map(Vec::len).max().unwrap_or(0);
    if ncols == 0 {
        return String::new();
    }
    let cells: Vec<Vec<String>> = table
        .rows
        .iter()
        .map(|row| {
            (0..ncols)
                .map(|c| {
                    row.get(c).map_or_else(String::new, |cell| {
                        let mut s = String::new();
                        write_inlines(&normalize(cell.clone()), true, &mut s);
                        s.trim().to_string()
                    })
                })
                .collect()
        })
        .collect();

    let widths: Vec<usize> =
        (0..ncols).map(|c| cells.iter().map(|row| row[c].width()).max().unwrap_or(0).clamp(3, MAX_PAD_WIDTH)).collect();

    let mut out = String::new();
    let push_row = |row: &[String], out: &mut String| {
        out.push('|');
        for (c, cell) in row.iter().enumerate() {
            out.push(' ');
            out.push_str(cell);
            let w = cell.width();
            if w < widths[c] {
                out.extend(std::iter::repeat_n(' ', widths[c] - w));
            }
            out.push_str(" |");
        }
        out.push('\n');
    };

    push_row(&cells[0], &mut out);
    out.push('|');
    for (c, width) in widths.iter().enumerate() {
        let align = table.alignments.get(c).copied().unwrap_or_default();
        let dashes = "-".repeat(width.saturating_sub(match align {
            Alignment::Center => 2,
            Alignment::Left | Alignment::Right => 1,
            Alignment::None => 0,
        }));
        let sep = match align {
            Alignment::None => dashes,
            Alignment::Left => format!(":{dashes}"),
            Alignment::Right => format!("{dashes}:"),
            Alignment::Center => format!(":{dashes}:"),
        };
        out.push(' ');
        out.push_str(&sep);
        out.push_str(" |");
    }
    out.push('\n');
    for row in &cells[1..] {
        push_row(row, &mut out);
    }
    out.pop();
    out
}

fn code_block_to_md(language: Option<&str>, code: &str) -> String {
    let longest = longest_run(code, '`');
    let fence = "`".repeat(longest.max(2) + 1);
    let lang: String = language.unwrap_or("").chars().filter(|c| !c.is_whitespace() && *c != '`').collect();
    let code = code.trim_end_matches(['\n', '\r']);
    format!("{fence}{lang}\n{code}\n{fence}")
}

fn longest_run(s: &str, ch: char) -> usize {
    let (mut best, mut cur) = (0, 0);
    for c in s.chars() {
        if c == ch {
            cur += 1;
            best = best.max(cur);
        } else {
            cur = 0;
        }
    }
    best
}

fn footnote_label(label: &str) -> String {
    let cleaned: String =
        label.chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '-' }).collect();
    if cleaned.is_empty() { "note".to_string() } else { cleaned }
}

// ---------------------------------------------------------------------------
// Inline normalization

/// Merges adjacent runs with the same formatting, removes empty containers
/// and moves surrounding whitespace outside emphasis markers, so that
/// `**bold **text` becomes `**bold** text`.
fn normalize(inlines: Vec<Inline>) -> Vec<Inline> {
    let mut out: Vec<Inline> = Vec::with_capacity(inlines.len());
    for inline in inlines {
        match inline {
            Inline::Strong(c) => push_container(&mut out, normalize(c), Inline::Strong),
            Inline::Emphasis(c) => push_container(&mut out, normalize(c), Inline::Emphasis),
            Inline::Strikethrough(c) => push_container(&mut out, normalize(c), Inline::Strikethrough),
            Inline::Link { url, title, content } => {
                let content = normalize(strip_links(content));
                if url.is_empty() {
                    for c in content {
                        push_merge(&mut out, c);
                    }
                } else {
                    let content = if has_content(&content) { content } else { vec![Inline::Text(url.clone())] };
                    out.push(Inline::Link { url, title, content });
                }
            }
            Inline::Text(t) if t.is_empty() => {}
            Inline::Code(c) if c.is_empty() => {}
            other => push_merge(&mut out, other),
        }
    }
    out
}

fn push_container(out: &mut Vec<Inline>, mut content: Vec<Inline>, wrap: fn(Vec<Inline>) -> Inline) {
    // Unwrap directly nested containers of the same kind (**a **b** c**).
    let probe = wrap(Vec::new());
    let mut flat = Vec::with_capacity(content.len());
    for c in content.drain(..) {
        match (&probe, c) {
            (Inline::Strong(_), Inline::Strong(inner))
            | (Inline::Emphasis(_), Inline::Emphasis(inner))
            | (Inline::Strikethrough(_), Inline::Strikethrough(inner)) => flat.extend(inner),
            (_, other) => flat.push(other),
        }
    }
    let mut content = flat;
    if !has_content(&content) {
        for c in content {
            push_merge(out, c);
        }
        return;
    }
    let leading = take_edge_whitespace(&mut content, true);
    let trailing = take_edge_whitespace(&mut content, false);
    if !leading.is_empty() {
        push_merge(out, Inline::Text(leading));
    }
    push_merge(out, wrap(content));
    if !trailing.is_empty() {
        push_merge(out, Inline::Text(trailing));
    }
}

fn take_edge_whitespace(content: &mut Vec<Inline>, start: bool) -> String {
    let idx = if start { 0 } else { content.len().saturating_sub(1) };
    match content.get_mut(idx) {
        Some(Inline::Text(t)) => {
            let trimmed = if start { t.trim_start() } else { t.trim_end() };
            let ws = if start { t[..t.len() - trimmed.len()].to_string() } else { t[trimmed.len()..].to_string() };
            *t = trimmed.to_string();
            if t.is_empty() {
                content.remove(idx);
            }
            ws
        }
        Some(Inline::Strong(c) | Inline::Emphasis(c) | Inline::Strikethrough(c)) => take_edge_whitespace(c, start),
        _ => String::new(),
    }
}

fn push_merge(out: &mut Vec<Inline>, inline: Inline) {
    match (out.last_mut(), inline) {
        (Some(Inline::Text(a)), Inline::Text(b)) => a.push_str(&b),
        (Some(Inline::Code(a)), Inline::Code(b)) => a.push_str(&b),
        (Some(Inline::Strong(a)), Inline::Strong(b))
        | (Some(Inline::Emphasis(a)), Inline::Emphasis(b))
        | (Some(Inline::Strikethrough(a)), Inline::Strikethrough(b)) => {
            for x in b {
                push_merge(a, x);
            }
        }
        (_, inline) => out.push(inline),
    }
}

fn strip_links(inlines: Vec<Inline>) -> Vec<Inline> {
    let mut out = Vec::with_capacity(inlines.len());
    for inline in inlines {
        match inline {
            Inline::Link { content, .. } => out.extend(strip_links(content)),
            Inline::Strong(c) => out.push(Inline::Strong(strip_links(c))),
            Inline::Emphasis(c) => out.push(Inline::Emphasis(strip_links(c))),
            Inline::Strikethrough(c) => out.push(Inline::Strikethrough(strip_links(c))),
            other => out.push(other),
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Inline writing

fn write_inlines(inlines: &[Inline], table: bool, out: &mut String) {
    for (i, inline) in inlines.iter().enumerate() {
        match inline {
            Inline::Text(t) => escape_text(t, table, out),
            Inline::Strong(c) => wrap(out, "**", c, table),
            Inline::Emphasis(c) => wrap(out, "*", c, table),
            Inline::Strikethrough(c) => wrap(out, "~~", c, table),
            Inline::Code(code) => out.push_str(&code_span(code, table)),
            Inline::Link { url, title, content } => {
                let text = plain_text(content);
                if title.is_none() && text == *url && is_autolink(url) {
                    out.push('<');
                    out.push_str(url);
                    out.push('>');
                } else {
                    out.push('[');
                    write_inlines(content, table, out);
                    out.push_str("](");
                    out.push_str(&link_destination(url));
                    push_title(title.as_deref(), out);
                    out.push(')');
                }
            }
            Inline::Image { src, alt, title } => {
                out.push_str("![");
                let mut a = String::new();
                escape_text(&alt.replace(['\n', '\r'], " "), table, &mut a);
                out.push_str(&a);
                out.push_str("](");
                out.push_str(&link_destination(src));
                push_title(title.as_deref(), out);
                out.push(')');
            }
            Inline::FootnoteRef(label) => {
                out.push_str("[^");
                out.push_str(&footnote_label(label));
                out.push(']');
            }
            Inline::LineBreak => {
                if table {
                    out.push_str("<br>");
                } else if i + 1 < inlines.len() {
                    out.push_str("\\\n");
                }
            }
        }
    }
}

fn wrap(out: &mut String, marker: &str, content: &[Inline], table: bool) {
    out.push_str(marker);
    write_inlines(content, table, out);
    out.push_str(marker);
}

fn push_title(title: Option<&str>, out: &mut String) {
    if let Some(title) = title.filter(|t| !t.is_empty()) {
        out.push_str(" \"");
        out.push_str(&title.replace('"', "\\\"").replace(['\n', '\r'], " "));
        out.push('"');
    }
}

fn is_autolink(url: &str) -> bool {
    (url.starts_with("http://") || url.starts_with("https://") || url.starts_with("mailto:"))
        && !url.contains(|c: char| c.is_whitespace() || c == '<' || c == '>')
}

fn link_destination(url: &str) -> String {
    let mut out = String::with_capacity(url.len());
    for c in url.chars() {
        match c {
            ' ' => out.push_str("%20"),
            '(' => out.push_str("%28"),
            ')' => out.push_str("%29"),
            '<' => out.push_str("%3C"),
            '>' => out.push_str("%3E"),
            '\n' | '\r' | '\t' => {}
            c => out.push(c),
        }
    }
    out
}

fn code_span(code: &str, table: bool) -> String {
    let mut code = code.replace(['\n', '\r'], " ");
    if table {
        code = code.replace('|', "\\|");
    }
    let fence = "`".repeat(longest_run(&code, '`') + 1);
    let pad = code.starts_with('`')
        || code.ends_with('`')
        || (code.starts_with(' ') && code.ends_with(' ') && !code.trim().is_empty());
    if pad { format!("{fence} {code} {fence}") } else { format!("{fence}{code}{fence}") }
}

fn escape_text(text: &str, table: bool, out: &mut String) {
    let chars: Vec<char> = text.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        match c {
            '\\' | '`' | '*' | '[' | ']' | '~' => {
                out.push('\\');
                out.push(c);
            }
            '_' => {
                // Intraword underscores never form emphasis in GFM.
                let prev = i > 0 && chars[i - 1].is_alphanumeric();
                let next = chars.get(i + 1).is_some_and(|n| n.is_alphanumeric());
                if !(prev && next) {
                    out.push('\\');
                }
                out.push('_');
            }
            '<' => {
                if chars.get(i + 1).is_some_and(|n| n.is_ascii_alphabetic() || matches!(n, '/' | '!' | '?')) {
                    out.push('\\');
                }
                out.push('<');
            }
            '&' => {
                if looks_like_entity(&chars[i + 1..]) {
                    out.push('\\');
                }
                out.push('&');
            }
            '|' if table => out.push_str("\\|"),
            '\n' | '\r' | '\t' => out.push(' '),
            c => out.push(c),
        }
    }
}

fn looks_like_entity(rest: &[char]) -> bool {
    let n = rest.iter().take(33).take_while(|c| c.is_ascii_alphanumeric() || **c == '#').count();
    n > 0 && rest.get(n) == Some(&';')
}

/// Escapes characters at the start of each line that would otherwise begin
/// a block construct (headings, lists, quotes, setext underlines).
fn escape_line_starts(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 8);
    for (i, line) in text.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
        }
        let line = line.trim_start();
        out.push_str(&escape_line_start(line));
    }
    out
}

fn escape_line_start(line: &str) -> String {
    let bytes = line.as_bytes();
    let Some(&first) = bytes.first() else { return String::new() };
    let followed_by_space = |n: usize| bytes.get(n).is_none_or(|b| *b == b' ' || *b == b'\\');
    let needs_escape = match first {
        b'#' => {
            let hashes = bytes.iter().take_while(|b| **b == b'#').count();
            hashes <= 6 && followed_by_space(hashes)
        }
        b'>' => true,
        b'-' | b'+' => followed_by_space(1) || line.trim_end().bytes().all(|b| b == first || b == b' '),
        b'=' => line.trim_end().bytes().all(|b| b == b'='),
        b'0'..=b'9' => {
            let digits = bytes.iter().take_while(|b| b.is_ascii_digit()).count();
            if digits <= 9 && matches!(bytes.get(digits), Some(b'.') | Some(b')')) && followed_by_space(digits + 1) {
                return format!("{}\\{}", &line[..digits], &line[digits..]);
            }
            false
        }
        _ => false,
    };
    if needs_escape { format!("\\{line}") } else { line.to_string() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn para(inlines: Vec<Inline>) -> Document {
        Document { blocks: vec![Block::Paragraph(inlines)], footnotes: vec![] }
    }

    #[test]
    fn hoists_whitespace_out_of_emphasis() {
        let doc = para(vec![Inline::Strong(vec![Inline::text("bold ")]), Inline::text("text")]);
        assert_eq!(render(&doc), "**bold** text\n");
    }

    #[test]
    fn merges_adjacent_runs() {
        let doc = para(vec![Inline::Strong(vec![Inline::text("He")]), Inline::Strong(vec![Inline::text("llo")])]);
        assert_eq!(render(&doc), "**Hello**\n");
    }

    #[test]
    fn escapes_markdown_syntax() {
        let doc = para(vec![Inline::text("# not a heading *star* snake_case _x_ 1. <b>")]);
        assert_eq!(render(&doc), "\\# not a heading \\*star\\* snake_case \\_x\\_ 1. \\<b>\n");
        let doc = para(vec![Inline::text("1. not a list")]);
        assert_eq!(render(&doc), "1\\. not a list\n");
    }

    #[test]
    fn renders_table_with_pipes() {
        let doc = Document {
            blocks: vec![Block::Table(Table {
                rows: vec![
                    vec![vec![Inline::text("a")], vec![Inline::text("b")]],
                    vec![vec![Inline::text("x|y")], vec![Inline::Code("p|q".into())]],
                ],
                alignments: vec![],
            })],
            footnotes: vec![],
        };
        assert_eq!(render(&doc), "| a    | b      |\n| ---- | ------ |\n| x\\|y | `p\\|q` |\n");
    }

    #[test]
    fn renders_nested_tight_list() {
        let item = |t: &str, nested: Option<List>| ListItem {
            checked: None,
            blocks: {
                let mut b = vec![Block::Paragraph(vec![Inline::text(t)])];
                if let Some(n) = nested {
                    b.push(Block::List(n));
                }
                b
            },
        };
        let inner = List { ordered: true, start: 1, items: vec![item("one", None), item("two", None)] };
        let doc = Document {
            blocks: vec![Block::List(List {
                ordered: false,
                start: 1,
                items: vec![item("a", Some(inner)), item("b", None)],
            })],
            footnotes: vec![],
        };
        assert_eq!(render(&doc), "- a\n  1. one\n  2. two\n- b\n");
    }

    #[test]
    fn renders_footnotes_and_code() {
        let doc = Document {
            blocks: vec![
                Block::Paragraph(vec![Inline::text("See"), Inline::FootnoteRef("1".into())]),
                Block::CodeBlock { language: Some("rust".into()), code: "fn main() {}\n".into() },
            ],
            footnotes: vec![Footnote {
                label: "1".into(),
                blocks: vec![Block::Paragraph(vec![Inline::text("Note.")])],
            }],
        };
        assert_eq!(render(&doc), "See[^1]\n\n```rust\nfn main() {}\n```\n\n[^1]: Note.\n");
    }
}
