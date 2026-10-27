//! Shared building blocks for "flat" formats (DOCX, RTF, PPTX, PDF) whose
//! source is a sequence of styled runs and paragraphs, not a tree.
//!
//! * [`RunBuilder`] turns styled text runs into nested inlines.
//! * [`assemble`] turns a sequence of classified paragraphs into blocks,
//!   building nested lists from list levels and merging code/quote lines.

use std::collections::HashMap;

use crate::model::*;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Style {
    pub bold: bool,
    pub italic: bool,
    pub strike: bool,
    pub code: bool,
}

#[derive(Debug, Clone)]
enum RunContent {
    Text(String),
    Inline(Inline),
}

#[derive(Debug, Clone)]
struct Run {
    style: Style,
    link: Option<String>,
    content: RunContent,
}

#[derive(Debug, Default, Clone)]
pub(crate) struct RunBuilder {
    runs: Vec<Run>,
    /// Set by [`RunBuilder::mark`]: the next text starts a new run.
    barrier: bool,
}

impl RunBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn text(&mut self, text: &str, style: Style, link: Option<&str>) {
        if text.is_empty() {
            return;
        }
        if !std::mem::take(&mut self.barrier)
            && let Some(Run { style: s, link: l, content: RunContent::Text(prev) }) = self.runs.last_mut()
            && *s == style
            && l.as_deref() == link
        {
            prev.push_str(text);
            return;
        }
        self.runs.push(Run { style, link: link.map(str::to_string), content: RunContent::Text(text.to_string()) });
    }

    /// Returns the current position for a later [`RunBuilder::link_from`],
    /// making sure following text isn't merged into earlier runs.
    pub fn mark(&mut self) -> usize {
        self.barrier = true;
        self.runs.len()
    }

    pub fn inline(&mut self, inline: Inline, style: Style, link: Option<&str>) {
        self.runs.push(Run { style, link: link.map(str::to_string), content: RunContent::Inline(inline) });
    }

    pub fn line_break(&mut self) {
        self.runs.push(Run { style: Style::default(), link: None, content: RunContent::Inline(Inline::LineBreak) });
    }

    /// Links every run from `start` onwards to `url` (used for hyperlink fields).
    pub fn link_from(&mut self, start: usize, url: &str) {
        for run in self.runs.iter_mut().skip(start) {
            if run.link.is_none() {
                run.link = Some(url.to_string());
            }
        }
    }

    /// True when every visible text run is monospace (a code line).
    pub fn is_all_code(&self) -> bool {
        let mut any = false;
        for run in &self.runs {
            match &run.content {
                RunContent::Text(t) if !t.trim().is_empty() => {
                    if !run.style.code {
                        return false;
                    }
                    any = true;
                }
                RunContent::Inline(Inline::LineBreak) | RunContent::Text(_) => {}
                RunContent::Inline(_) => return false,
            }
        }
        any
    }

    pub fn has_content(&self) -> bool {
        self.runs.iter().any(|r| match &r.content {
            RunContent::Text(t) => !t.trim().is_empty(),
            RunContent::Inline(Inline::LineBreak) => false,
            RunContent::Inline(_) => true,
        })
    }

    pub fn plain_text(&self) -> String {
        let mut out = String::new();
        for run in &self.runs {
            match &run.content {
                RunContent::Text(t) => out.push_str(t),
                RunContent::Inline(i) => out.push_str(&plain_text(std::slice::from_ref(i))),
            }
        }
        out
    }

    pub fn finish(self) -> Vec<Inline> {
        let mut out = Vec::new();
        let runs = &self.runs;
        let mut i = 0;
        while i < runs.len() {
            let link = &runs[i].link;
            let mut j = i + 1;
            while j < runs.len() && runs[j].link == *link {
                j += 1;
            }
            let content = build_styled(&runs[i..j], 0);
            match link {
                Some(url) => out.push(Inline::Link { url: url.clone(), title: None, content }),
                None => out.extend(content),
            }
            i = j;
        }
        out
    }
}

fn flag(style: &Style, level: usize) -> bool {
    match level {
        0 => style.bold,
        1 => style.italic,
        2 => style.strike,
        _ => style.code,
    }
}

fn build_styled(runs: &[Run], level: usize) -> Vec<Inline> {
    let mut out = Vec::new();
    if level == 4 {
        for run in runs {
            match &run.content {
                RunContent::Text(t) => out.push(Inline::Text(t.clone())),
                RunContent::Inline(i) => out.push(i.clone()),
            }
        }
        return out;
    }
    let mut i = 0;
    while i < runs.len() {
        let on = flag(&runs[i].style, level);
        let mut j = i + 1;
        while j < runs.len() && flag(&runs[j].style, level) == on {
            j += 1;
        }
        let group = &runs[i..j];
        if !on {
            out.extend(build_styled(group, level + 1));
        } else {
            match level {
                0 => out.push(Inline::Strong(build_styled(group, 1))),
                1 => out.push(Inline::Emphasis(build_styled(group, 2))),
                2 => out.push(Inline::Strikethrough(build_styled(group, 3))),
                _ => {
                    let mut code = String::new();
                    for run in group {
                        match &run.content {
                            RunContent::Text(t) => code.push_str(t),
                            RunContent::Inline(inline) => {
                                if !code.is_empty() {
                                    out.push(Inline::Code(std::mem::take(&mut code)));
                                }
                                out.push(inline.clone());
                            }
                        }
                    }
                    if !code.is_empty() {
                        // Keep surrounding spaces outside the code span.
                        let trimmed = code.trim();
                        let lead = &code[..code.len() - code.trim_start().len()];
                        let trail = &code[code.trim_end().len()..];
                        if !lead.is_empty() {
                            out.push(Inline::text(lead));
                        }
                        if !trimmed.is_empty() {
                            out.push(Inline::Code(trimmed.to_string()));
                        }
                        if !trail.is_empty() {
                            out.push(Inline::text(trail));
                        }
                    }
                }
            }
        }
        i = j;
    }
    out
}

// ---------------------------------------------------------------------------
// Paragraph assembly

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ParaKind {
    Normal,
    Heading(u8),
    Code,
    Quote,
    ListItem { level: usize, ordered: bool, start: u64, list_id: String },
}

#[derive(Debug, Clone)]
pub(crate) struct Para {
    pub kind: ParaKind,
    pub inlines: Vec<Inline>,
}

#[derive(Debug, Clone)]
pub(crate) enum Item {
    Para(Para),
    Block(Block),
}

/// Turns classified paragraphs into blocks: nests list items by level,
/// merges consecutive code lines into one code block and consecutive quote
/// paragraphs into one block quote.
pub(crate) fn assemble(items: Vec<Item>) -> Vec<Block> {
    let mut out = Vec::new();
    let mut lists = ListBuilder::default();
    let mut code: Vec<String> = Vec::new();
    let mut quote: Vec<Block> = Vec::new();

    fn flush_code(code: &mut Vec<String>, out: &mut Vec<Block>) {
        if !code.is_empty() {
            let text = code.join("\n");
            code.clear();
            if !text.trim().is_empty() {
                out.push(Block::CodeBlock { language: None, code: text });
            }
        }
    }
    fn flush_quote(quote: &mut Vec<Block>, out: &mut Vec<Block>) {
        if !quote.is_empty() {
            out.push(Block::BlockQuote(std::mem::take(quote)));
        }
    }

    for item in items {
        match item {
            Item::Para(Para { kind: ParaKind::ListItem { level, ordered, start, list_id }, mut inlines }) => {
                flush_code(&mut code, &mut out);
                flush_quote(&mut quote, &mut out);
                trim_inlines(&mut inlines);
                lists.push(level, ordered, start, &list_id, inlines, &mut out);
            }
            Item::Para(Para { kind: ParaKind::Code, inlines }) => {
                lists.close_all(&mut out);
                flush_quote(&mut quote, &mut out);
                code.push(plain_text(&inlines).trim_end().replace('\u{a0}', " "));
            }
            Item::Para(Para { kind: ParaKind::Quote, mut inlines }) => {
                if !has_content(&inlines) {
                    continue;
                }
                lists.close_all(&mut out);
                flush_code(&mut code, &mut out);
                trim_inlines(&mut inlines);
                quote.push(Block::Paragraph(inlines));
            }
            Item::Para(Para { kind, mut inlines }) => {
                // Blank paragraphs are spacing, not structure: they don't end lists.
                if !has_content(&inlines) {
                    if !code.is_empty() {
                        code.push(String::new());
                    }
                    continue;
                }
                lists.close_all(&mut out);
                flush_code(&mut code, &mut out);
                flush_quote(&mut quote, &mut out);
                trim_inlines(&mut inlines);
                match kind {
                    ParaKind::Heading(level) => out.push(Block::Heading { level, content: inlines }),
                    _ => out.push(Block::Paragraph(inlines)),
                }
            }
            Item::Block(block) => {
                lists.close_all(&mut out);
                flush_code(&mut code, &mut out);
                flush_quote(&mut quote, &mut out);
                out.push(block);
            }
        }
    }
    lists.close_all(&mut out);
    flush_code(&mut code, &mut out);
    flush_quote(&mut quote, &mut out);
    out
}

#[derive(Default)]
struct ListBuilder {
    /// Open lists, outermost first, with their nesting level.
    stack: Vec<(List, usize)>,
    /// Last number used per (list id, level), so interrupted numbered lists
    /// continue counting like they do in the source document.
    counters: HashMap<(String, usize), u64>,
}

impl ListBuilder {
    fn push(
        &mut self,
        level: usize,
        ordered: bool,
        start: u64,
        list_id: &str,
        inlines: Vec<Inline>,
        out: &mut Vec<Block>,
    ) {
        while self.stack.last().is_some_and(|(_, l)| *l > level) {
            self.pop(out);
        }
        if self.stack.last().is_some_and(|(list, l)| *l == level && list.ordered != ordered) {
            self.pop(out);
        }
        if self.stack.last().is_none_or(|(_, l)| *l < level) {
            let key = (list_id.to_string(), level);
            let start = if ordered { self.counters.get(&key).map_or(start, |n| n + 1) } else { 1 };
            self.stack.push((List { ordered, start, items: Vec::new() }, level));
        }
        let (list, _) = self.stack.last_mut().expect("open list");
        list.items.push(ListItem { checked: None, blocks: vec![Block::Paragraph(inlines)] });
        if ordered {
            let n = list.start + list.items.len() as u64 - 1;
            self.counters.insert((list_id.to_string(), level), n);
            // A new item restarts numbering of its sub-levels.
            self.counters.retain(|(id, l), _| id != list_id || *l <= level);
        }
    }

    fn pop(&mut self, out: &mut Vec<Block>) {
        let Some((list, _)) = self.stack.pop() else { return };
        match self.stack.last_mut() {
            Some((parent, _)) => {
                if parent.items.is_empty() {
                    parent.items.push(ListItem::default());
                }
                parent.items.last_mut().expect("item").blocks.push(Block::List(list));
            }
            None => out.push(Block::List(list)),
        }
    }

    fn close_all(&mut self, out: &mut Vec<Block>) {
        while !self.stack.is_empty() {
            self.pop(out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn li(level: usize, ordered: bool, text: &str) -> Item {
        Item::Para(Para {
            kind: ParaKind::ListItem { level, ordered, start: 1, list_id: "1".into() },
            inlines: vec![Inline::text(text)],
        })
    }

    #[test]
    fn builds_nested_lists_and_continues_numbering() {
        let blocks = assemble(vec![
            li(0, true, "a"),
            li(1, false, "a.1"),
            li(0, true, "b"),
            Item::Para(Para { kind: ParaKind::Normal, inlines: vec![Inline::text("break")] }),
            li(0, true, "c"),
        ]);
        let md = crate::markdown::render(&Document { blocks, footnotes: vec![] });
        assert_eq!(md, "1. a\n   - a.1\n2. b\n\nbreak\n\n3. c\n");
    }

    #[test]
    fn styles_runs() {
        let mut b = RunBuilder::new();
        let bold = Style { bold: true, ..Style::default() };
        b.text("Hello ", bold, None);
        b.text("world", Style { italic: true, ..bold }, None);
        b.text(" and ", Style::default(), None);
        b.text("code", Style { code: true, ..Style::default() }, Some("https://x.y"));
        let md = crate::markdown::render_inlines(&b.finish());
        assert_eq!(md, "**Hello *world*** and [`code`](https://x.y)");
    }
}
