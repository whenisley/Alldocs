//! Format-independent document model.
//!
//! Every converter produces a [`Document`]; the Markdown renderer only ever
//! sees this model, which is what keeps output consistent across formats.

/// A converted document: body blocks plus footnote definitions.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Document {
    pub blocks: Vec<Block>,
    pub footnotes: Vec<Footnote>,
}

/// A footnote (or endnote) definition, rendered as `[^label]: ...`.
#[derive(Debug, Clone, PartialEq)]
pub struct Footnote {
    pub label: String,
    pub blocks: Vec<Block>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Block {
    Heading {
        level: u8,
        content: Vec<Inline>,
    },
    Paragraph(Vec<Inline>),
    List(List),
    Table(Table),
    CodeBlock {
        language: Option<String>,
        code: String,
    },
    BlockQuote(Vec<Block>),
    ThematicBreak,
    /// Rendered as an HTML comment (`<!-- ... -->`), e.g. slide markers.
    Comment(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct List {
    pub ordered: bool,
    pub start: u64,
    pub items: Vec<ListItem>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ListItem {
    /// `Some(_)` renders a GFM task-list checkbox.
    pub checked: Option<bool>,
    pub blocks: Vec<Block>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Alignment {
    #[default]
    None,
    Left,
    Center,
    Right,
}

/// A table. The first row is rendered as the GFM header row.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Table {
    pub rows: Vec<Vec<Vec<Inline>>>,
    pub alignments: Vec<Alignment>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Inline {
    Text(String),
    Strong(Vec<Inline>),
    Emphasis(Vec<Inline>),
    Strikethrough(Vec<Inline>),
    Code(String),
    Link { url: String, title: Option<String>, content: Vec<Inline> },
    Image { src: String, alt: String, title: Option<String> },
    FootnoteRef(String),
    LineBreak,
}

impl Inline {
    pub fn text(s: impl Into<String>) -> Inline {
        Inline::Text(s.into())
    }
}

/// Concatenated text of `inlines`, with line breaks as `\n`.
pub fn plain_text(inlines: &[Inline]) -> String {
    let mut out = String::new();
    push_plain_text(inlines, &mut out);
    out
}

fn push_plain_text(inlines: &[Inline], out: &mut String) {
    for inline in inlines {
        match inline {
            Inline::Text(t) | Inline::Code(t) => out.push_str(t),
            Inline::Strong(c) | Inline::Emphasis(c) | Inline::Strikethrough(c) => push_plain_text(c, out),
            Inline::Link { content, .. } => push_plain_text(content, out),
            Inline::Image { alt, .. } => out.push_str(alt),
            Inline::FootnoteRef(_) => {}
            Inline::LineBreak => out.push('\n'),
        }
    }
}

/// True if the inlines contain anything visible (non-blank text, images, notes).
pub fn has_content(inlines: &[Inline]) -> bool {
    inlines.iter().any(|inline| match inline {
        Inline::Text(t) | Inline::Code(t) => !t.trim().is_empty(),
        Inline::Strong(c) | Inline::Emphasis(c) | Inline::Strikethrough(c) => has_content(c),
        Inline::Link { content, .. } => has_content(content),
        Inline::Image { .. } | Inline::FootnoteRef(_) => true,
        Inline::LineBreak => false,
    })
}

/// Removes leading/trailing whitespace and line breaks from a run of inlines.
pub fn trim_inlines(inlines: &mut Vec<Inline>) {
    while matches!(inlines.first(), Some(Inline::LineBreak)) {
        inlines.remove(0);
    }
    while matches!(inlines.last(), Some(Inline::LineBreak)) {
        inlines.pop();
    }
    trim_edge(inlines, true);
    trim_edge(inlines, false);
}

fn trim_edge(inlines: &mut Vec<Inline>, start: bool) {
    loop {
        let idx = if start { 0 } else { inlines.len().wrapping_sub(1) };
        let Some(inline) = inlines.get_mut(idx) else { return };
        match inline {
            Inline::Text(t) => {
                let trimmed = if start { t.trim_start() } else { t.trim_end() };
                if trimmed.is_empty() {
                    inlines.remove(idx);
                    continue;
                }
                if trimmed.len() != t.len() {
                    *t = trimmed.to_string();
                }
                return;
            }
            Inline::Strong(c) | Inline::Emphasis(c) | Inline::Strikethrough(c) => {
                trim_edge(c, start);
                if c.is_empty() {
                    inlines.remove(idx);
                    continue;
                }
                return;
            }
            Inline::LineBreak => {
                inlines.remove(idx);
            }
            _ => return,
        }
    }
}

/// Collapses runs of whitespace (as in HTML/ODF text) to single spaces.
pub fn collapse_whitespace(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_space = false;
    for c in s.chars() {
        if c.is_whitespace() && c != '\u{a0}' {
            if !in_space {
                out.push(' ');
                in_space = true;
            }
        } else {
            out.push(c);
            in_space = false;
        }
    }
    out
}

/// Flattens blocks into a single line of inlines (used for table cells,
/// which GFM restricts to inline content).
pub fn blocks_to_inlines(blocks: &[Block]) -> Vec<Inline> {
    let mut out = Vec::new();
    for block in blocks {
        let start = out.len();
        if start > 0 {
            out.push(Inline::LineBreak);
        }
        match block {
            Block::Heading { content, .. } => out.push(Inline::Strong(content.clone())),
            Block::Paragraph(content) => out.extend(content.iter().cloned()),
            Block::List(list) => {
                for (i, item) in list.items.iter().enumerate() {
                    if i > 0 {
                        out.push(Inline::LineBreak);
                    }
                    let marker = if list.ordered { format!("{}. ", list.start + i as u64) } else { "• ".to_string() };
                    out.push(Inline::Text(marker));
                    out.extend(blocks_to_inlines(&item.blocks));
                }
            }
            Block::Table(table) => {
                for (i, row) in table.rows.iter().enumerate() {
                    if i > 0 {
                        out.push(Inline::LineBreak);
                    }
                    for (j, cell) in row.iter().enumerate() {
                        if j > 0 {
                            out.push(Inline::text(" / "));
                        }
                        out.extend(cell.iter().cloned());
                    }
                }
            }
            Block::CodeBlock { code, .. } => {
                for (i, line) in code.lines().enumerate() {
                    if i > 0 {
                        out.push(Inline::LineBreak);
                    }
                    out.push(Inline::Code(line.to_string()));
                }
            }
            Block::BlockQuote(inner) => out.extend(blocks_to_inlines(inner)),
            Block::ThematicBreak | Block::Comment(_) => {}
        }
        if out.len() == start + 1 && start > 0 {
            out.pop();
        }
    }
    out
}

/// Visits every inline in the document (body and footnotes), mutably.
pub(crate) fn visit_inlines_mut(doc: &mut Document, f: &mut dyn FnMut(&mut Vec<Inline>)) {
    for block in &mut doc.blocks {
        visit_block(block, f);
    }
    for note in &mut doc.footnotes {
        for block in &mut note.blocks {
            visit_block(block, f);
        }
    }
}

fn visit_block(block: &mut Block, f: &mut dyn FnMut(&mut Vec<Inline>)) {
    match block {
        Block::Heading { content, .. } | Block::Paragraph(content) => visit_inline_vec(content, f),
        Block::List(list) => {
            for item in &mut list.items {
                for b in &mut item.blocks {
                    visit_block(b, f);
                }
            }
        }
        Block::Table(table) => {
            for row in &mut table.rows {
                for cell in row {
                    visit_inline_vec(cell, f);
                }
            }
        }
        Block::BlockQuote(inner) => {
            for b in inner {
                visit_block(b, f);
            }
        }
        Block::CodeBlock { .. } | Block::ThematicBreak | Block::Comment(_) => {}
    }
}

fn visit_inline_vec(inlines: &mut Vec<Inline>, f: &mut dyn FnMut(&mut Vec<Inline>)) {
    f(inlines);
    for inline in inlines.iter_mut() {
        match inline {
            Inline::Strong(c) | Inline::Emphasis(c) | Inline::Strikethrough(c) => visit_inline_vec(c, f),
            Inline::Link { content, .. } => visit_inline_vec(content, f),
            _ => {}
        }
    }
}
