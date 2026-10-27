//! Rich Text Format.
//!
//! A single-pass tokenizer/interpreter. It understands character and
//! paragraph formatting, style sheets (headings), list paragraphs, tables,
//! HYPERLINK fields, footnotes, embedded pictures and OLE objects, code
//! pages (`\ansicpg`, `\fcharset`) and Unicode escapes (`\u`, `\uc`).

use std::collections::HashMap;

use encoding_rs::Encoding;

use super::{is_monospace_font, style_name_heading_level};
use crate::model::*;
use crate::runs::{Item, Para, ParaKind, RunBuilder, Style, assemble};
use crate::{Ctx, ResourceKind, Result};

pub(crate) fn convert(data: &[u8], ctx: &mut Ctx) -> Result<Document> {
    let mut rtf = Rtf::new(ctx);
    rtf.parse(data);
    Ok(rtf.finish())
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Dest {
    Normal,
    Skip,
    FontTable,
    StyleSheet,
    Info,
    InfoField(InfoField),
    ListText,
    FieldInst,
    Pict,
    ObjData,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum InfoField {
    Title,
    Author,
    Subject,
    Keywords,
    Comment,
}

#[derive(Clone)]
struct State {
    dest: Dest,
    bold: bool,
    italic: bool,
    strike: bool,
    hidden: bool,
    font: Option<i32>,
    uc: usize,
    style: Option<i32>,
    outline: Option<u8>,
    list: Option<i32>,
    ilvl: usize,
    intbl: bool,
}

impl Default for State {
    fn default() -> Self {
        State {
            dest: Dest::Normal,
            bold: false,
            italic: false,
            strike: false,
            hidden: false,
            font: None,
            uc: 1,
            style: None,
            outline: None,
            list: None,
            ilvl: 0,
            intbl: false,
        }
    }
}

#[derive(Default, Clone)]
struct FontDef {
    name: String,
    charset: Option<i32>,
    mono: bool,
}

#[derive(Default, Clone)]
struct StyleDef {
    name: String,
    outline: Option<u8>,
}

enum Scope {
    Pict { data: Vec<u8>, nibble: Option<u8>, ext: &'static str },
    Obj { data: Vec<u8>, nibble: Option<u8> },
    Field { instr: String, result_start: Option<(usize, usize)> },
    Footnote { label: String },
}

/// Where paragraphs currently go: the body, or a footnote being read.
#[derive(Default)]
struct Target {
    items: Vec<Item>,
    para: RunBuilder,
    para_id: usize,
    list_text: String,
    cell: Vec<Inline>,
    row: Vec<Vec<Inline>>,
    rows: Vec<Vec<Vec<Inline>>>,
}

struct Rtf<'c, 'o> {
    ctx: &'c mut Ctx<'o>,
    stack: Vec<State>,
    fonts: HashMap<i32, FontDef>,
    styles: HashMap<i32, StyleDef>,
    default_font: Option<i32>,
    codepage: &'static Encoding,
    targets: Vec<Target>,
    scopes: Vec<(usize, Scope)>,
    footnotes: Vec<Footnote>,
    next_para_id: usize,
    pending: Vec<u8>,
    skip_chars: usize,
    ignorable: bool,
    high_surrogate: Option<u16>,
    pending_font: Option<(i32, FontDef)>,
    pending_style: Option<(Option<i32>, StyleDef)>,
    info_text: String,
    images: usize,
    objects: usize,
}

impl<'c, 'o> Rtf<'c, 'o> {
    fn new(ctx: &'c mut Ctx<'o>) -> Self {
        Rtf {
            ctx,
            stack: vec![State::default()],
            fonts: HashMap::new(),
            styles: HashMap::new(),
            default_font: None,
            codepage: encoding_rs::WINDOWS_1252,
            targets: vec![Target::default()],
            scopes: Vec::new(),
            footnotes: Vec::new(),
            next_para_id: 1,
            pending: Vec::new(),
            skip_chars: 0,
            ignorable: false,
            high_surrogate: None,
            pending_font: None,
            pending_style: None,
            info_text: String::new(),
            images: 0,
            objects: 0,
        }
    }

    fn state(&mut self) -> &mut State {
        self.stack.last_mut().expect("state")
    }

    fn dest(&self) -> Dest {
        self.stack.last().map_or(Dest::Normal, |s| s.dest)
    }

    fn target(&mut self) -> &mut Target {
        self.targets.last_mut().expect("target")
    }

    fn parse(&mut self, data: &[u8]) {
        let len = data.len();
        let mut i = 0;
        while i < len {
            match data[i] {
                b'{' => {
                    self.flush();
                    let top = self.stack.last().cloned().unwrap_or_default();
                    self.stack.push(top);
                    self.ignorable = false;
                    i += 1;
                }
                b'}' => {
                    self.flush();
                    self.close_group();
                    i += 1;
                }
                b'\\' => {
                    i += 1;
                    let Some(&c) = data.get(i) else { break };
                    if c.is_ascii_alphabetic() {
                        let start = i;
                        while i < len && data[i].is_ascii_alphabetic() && i - start < 32 {
                            i += 1;
                        }
                        let word = std::str::from_utf8(&data[start..i]).unwrap_or("");
                        let negative = data.get(i) == Some(&b'-') && data.get(i + 1).is_some_and(u8::is_ascii_digit);
                        if negative {
                            i += 1;
                        }
                        let digits_start = i;
                        while i < len && data[i].is_ascii_digit() && i - digits_start < 10 {
                            i += 1;
                        }
                        let param = (i > digits_start).then(|| {
                            let n: i64 = std::str::from_utf8(&data[digits_start..i])
                                .ok()
                                .and_then(|s| s.parse().ok())
                                .unwrap_or(0);
                            let n = if negative { -n } else { n };
                            n.clamp(i32::MIN as i64, i32::MAX as i64) as i32
                        });
                        if data.get(i) == Some(&b' ') {
                            i += 1;
                        }
                        if word == "bin" {
                            let n = param.unwrap_or(0).max(0) as usize;
                            let end = (i + n).min(len);
                            self.flush();
                            self.binary(&data[i..end]);
                            i = end;
                            continue;
                        }
                        self.control(word, param);
                    } else if c == b'\'' {
                        let hex = data
                            .get(i + 1..i + 3)
                            .and_then(|h| std::str::from_utf8(h).ok())
                            .and_then(|h| u8::from_str_radix(h, 16).ok());
                        match hex {
                            Some(v) => {
                                self.byte(v);
                                i += 3;
                            }
                            None => i += 1,
                        }
                    } else {
                        i += 1;
                        self.symbol(c);
                    }
                }
                b'\r' | b'\n' => i += 1,
                _ => {
                    let start = i;
                    while i < len && !matches!(data[i], b'{' | b'}' | b'\\' | b'\r' | b'\n') {
                        i += 1;
                    }
                    self.text_bytes(&data[start..i]);
                }
            }
        }
        self.flush();
    }

    // -- text --------------------------------------------------------------

    fn text_bytes(&mut self, mut bytes: &[u8]) {
        if self.skip_chars > 0 {
            let n = self.skip_chars.min(bytes.len());
            bytes = &bytes[n..];
            self.skip_chars -= n;
        }
        match self.dest() {
            Dest::Pict | Dest::ObjData => self.hex_data(bytes),
            Dest::Skip => {}
            _ => self.pending.extend_from_slice(bytes),
        }
    }

    fn byte(&mut self, v: u8) {
        if self.skip_chars > 0 {
            self.skip_chars -= 1;
            return;
        }
        if !matches!(self.dest(), Dest::Pict | Dest::ObjData | Dest::Skip) {
            self.pending.push(v);
        }
    }

    fn hex_data(&mut self, bytes: &[u8]) {
        let Some((_, scope)) = self.scopes.last_mut() else { return };
        let (data, nibble) = match scope {
            Scope::Pict { data, nibble, .. } | Scope::Obj { data, nibble } => (data, nibble),
            _ => return,
        };
        for &b in bytes {
            let Some(v) = (b as char).to_digit(16) else { continue };
            match nibble.take() {
                Some(hi) => data.push(hi << 4 | v as u8),
                None => *nibble = Some(v as u8),
            }
        }
    }

    fn binary(&mut self, bytes: &[u8]) {
        if let Some((_, Scope::Pict { data, .. } | Scope::Obj { data, .. })) = self.scopes.last_mut() {
            data.extend_from_slice(bytes);
        }
    }

    fn encoding(&self) -> &'static Encoding {
        let state = self.stack.last();
        if state.is_some_and(|s| s.dest == Dest::Normal)
            && let Some(cs) = state.and_then(|s| s.font).and_then(|f| self.fonts.get(&f)).and_then(|f| f.charset)
            && let Some(enc) = charset_encoding(cs)
        {
            return enc;
        }
        self.codepage
    }

    fn flush(&mut self) {
        if self.pending.is_empty() {
            return;
        }
        let bytes = std::mem::take(&mut self.pending);
        let text = self.encoding().decode_without_bom_handling(&bytes).0.into_owned();
        self.emit(&text);
    }

    fn emit(&mut self, text: &str) {
        match self.dest() {
            Dest::Normal => {
                let st = self.stack.last().expect("state");
                if st.hidden {
                    return;
                }
                let mono = st.font.and_then(|f| self.fonts.get(&f)).is_some_and(|f| f.mono);
                let style = Style { bold: st.bold, italic: st.italic, strike: st.strike, code: mono };
                self.target().para.text(text, style, None);
            }
            Dest::FontTable => {
                for ch in text.chars() {
                    if ch == ';' {
                        self.commit_font();
                    } else if let Some((_, f)) = self.pending_font.as_mut() {
                        f.name.push(ch);
                    }
                }
            }
            Dest::StyleSheet => {
                for ch in text.chars() {
                    if ch == ';' {
                        self.commit_style();
                    } else if let Some((_, s)) = self.pending_style.as_mut() {
                        s.name.push(ch);
                    }
                }
            }
            Dest::InfoField(_) => self.info_text.push_str(text),
            Dest::ListText => self.target().list_text.push_str(text),
            Dest::FieldInst => {
                if let Some(Scope::Field { instr, .. }) =
                    self.scopes.iter_mut().rev().map(|(_, s)| s).find(|s| matches!(s, Scope::Field { .. }))
                {
                    instr.push_str(text);
                }
            }
            _ => {}
        }
    }

    fn emit_char(&mut self, ch: char) {
        self.flush();
        self.emit(ch.encode_utf8(&mut [0; 4]));
    }

    fn commit_font(&mut self) {
        if let Some((id, mut f)) = self.pending_font.take() {
            f.name = f.name.trim().to_string();
            f.mono |= is_monospace_font(&f.name);
            self.fonts.insert(id, f);
        }
    }

    fn commit_style(&mut self) {
        if let Some((id, mut s)) = self.pending_style.take()
            && let Some(id) = id
        {
            s.name = s.name.trim().to_string();
            self.styles.insert(id, s);
        }
    }

    // -- groups ------------------------------------------------------------

    fn close_group(&mut self) {
        let depth = self.stack.len();
        match self.dest() {
            Dest::FontTable => self.commit_font(),
            Dest::StyleSheet => self.commit_style(),
            Dest::InfoField(field) => {
                let value = std::mem::take(&mut self.info_text).trim().to_string();
                if !value.is_empty() {
                    let meta = &mut self.ctx.metadata;
                    let slot = match field {
                        InfoField::Title => &mut meta.title,
                        InfoField::Author => &mut meta.author,
                        InfoField::Subject => &mut meta.subject,
                        InfoField::Keywords => &mut meta.keywords,
                        InfoField::Comment => &mut meta.description,
                    };
                    *slot = Some(value);
                }
            }
            _ => {}
        }
        while self.scopes.last().is_some_and(|(d, _)| *d == depth) {
            let (_, scope) = self.scopes.pop().expect("scope");
            self.finish_scope(scope);
        }
        if self.stack.len() > 1 {
            self.stack.pop();
        }
    }

    fn finish_scope(&mut self, scope: Scope) {
        match scope {
            Scope::Pict { data, ext, .. } => {
                if data.is_empty() {
                    return;
                }
                self.images += 1;
                let name = format!("image{}.{ext}", self.images);
                let src = self.ctx.add_resource(&format!("rtf-pict:{}", self.images), &name, data, ResourceKind::Image);
                self.target().para.inline(
                    Inline::Image { src, alt: String::new(), title: None },
                    Style::default(),
                    None,
                );
            }
            Scope::Obj { data, .. } => {
                if data.is_empty() {
                    return;
                }
                self.objects += 1;
                let name = format!("object{}.bin", self.objects);
                let src =
                    self.ctx.add_resource(&format!("rtf-obj:{}", self.objects), &name, data, ResourceKind::Object);
                let link = Inline::Link { url: src, title: None, content: vec![Inline::text("Embedded object")] };
                self.target().para.inline(link, Style::default(), None);
            }
            Scope::Field { instr, result_start } => {
                let url = hyperlink_url(&instr);
                let target = self.target();
                if let (Some((para_id, start)), Some(url)) = (result_start, url)
                    && para_id == target.para_id
                {
                    target.para.link_from(start, &url);
                }
            }
            Scope::Footnote { label } => {
                self.end_paragraph();
                self.flush_table();
                let target = self.targets.pop().expect("footnote target");
                self.footnotes.push(Footnote { label, blocks: assemble(target.items) });
            }
        }
    }

    // -- control words -----------------------------------------------------

    fn symbol(&mut self, c: u8) {
        if c == b'*' {
            self.ignorable = true;
            return;
        }
        if self.dest() == Dest::Skip {
            return;
        }
        match c {
            b'\\' | b'{' | b'}' => self.byte(c),
            b'~' => self.emit_char('\u{a0}'),
            b'_' => self.emit_char('-'),
            b'\n' | b'\r' => {
                self.flush();
                self.end_paragraph();
            }
            _ => {}
        }
    }

    fn control(&mut self, word: &str, param: Option<i32>) {
        let ignorable = std::mem::take(&mut self.ignorable);
        let dest = self.dest();
        if dest == Dest::Skip {
            return;
        }
        if dest == Dest::Pict {
            if let Some((_, Scope::Pict { ext, .. })) = self.scopes.last_mut() {
                match word {
                    "pngblip" => *ext = "png",
                    "jpegblip" => *ext = "jpg",
                    "emfblip" => *ext = "emf",
                    "wmetafile" => *ext = "wmf",
                    "macpict" => *ext = "pict",
                    "dibitmap" | "wbitmap" => *ext = "bmp",
                    _ => {}
                }
            }
            if ignorable {
                self.state().dest = Dest::Skip;
            }
            return;
        }
        if dest == Dest::ObjData {
            return;
        }
        self.flush();
        let on = param != Some(0);
        let depth = self.stack.len();
        match word {
            // Destinations
            "fonttbl" => self.state().dest = Dest::FontTable,
            "stylesheet" => self.state().dest = Dest::StyleSheet,
            "info" => self.state().dest = Dest::Info,
            "title" | "author" | "subject" | "keywords" | "doccomm" if dest == Dest::Info => {
                self.info_text.clear();
                self.state().dest = Dest::InfoField(match word {
                    "title" => InfoField::Title,
                    "author" => InfoField::Author,
                    "subject" => InfoField::Subject,
                    "keywords" => InfoField::Keywords,
                    _ => InfoField::Comment,
                });
            }
            "listtext" | "pntext" => self.state().dest = Dest::ListText,
            "fldinst" => self.state().dest = Dest::FieldInst,
            "fldrslt" => {
                let (para_id, len) = {
                    let t = self.target();
                    (t.para_id, t.para.mark())
                };
                if let Some(Scope::Field { result_start, .. }) =
                    self.scopes.iter_mut().rev().map(|(_, s)| s).find(|s| matches!(s, Scope::Field { .. }))
                {
                    *result_start = Some((para_id, len));
                }
            }
            "field" => self.scopes.push((depth, Scope::Field { instr: String::new(), result_start: None })),
            "pict" => {
                self.scopes.push((depth, Scope::Pict { data: Vec::new(), nibble: None, ext: "bin" }));
                self.state().dest = Dest::Pict;
            }
            "objdata" => {
                self.scopes.push((depth, Scope::Obj { data: Vec::new(), nibble: None }));
                self.state().dest = Dest::ObjData;
            }
            "footnote" => {
                let label = (self.footnotes.len()
                    + self.scopes.iter().filter(|(_, s)| matches!(s, Scope::Footnote { .. })).count()
                    + 1)
                .to_string();
                self.target().para.inline(Inline::FootnoteRef(label.clone()), Style::default(), None);
                let para_id = self.new_para_id();
                self.targets.push(Target { para_id, ..Target::default() });
                self.scopes.push((depth, Scope::Footnote { label }));
                let st = self.state();
                st.style = None;
                st.outline = None;
                st.list = None;
                st.intbl = false;
            }
            "shppict" | "shprslt" | "result" | "object" | "chftn" => {}
            "colortbl" | "header" | "headerl" | "headerr" | "headerf" | "footer" | "footerl" | "footerr"
            | "footerf" | "pntxta" | "pntxtb" | "listtable" | "listoverridetable" | "rsidtbl" | "generator"
            | "xmlnstbl" | "mmathPr" | "themedata" | "colorschememapping" | "latentstyles" | "datastore"
            | "filetbl" | "revtbl" | "pgdsctbl" | "ftnsep" | "ftnsepc" | "aftnsep" | "aftnsepc" | "tc" | "tcn"
            | "xe" | "txe" | "nonshppict" | "bkmkstart" | "bkmkend" | "annotation" | "atnid" | "atnauthor"
            | "atndate" | "atnref" | "userprops" | "docvar" | "template" | "objclass" | "objname" | "fldtype"
            | "formfield" | "datafield" | "background" | "operator" | "company" | "manager" | "category"
            | "hlinkbase" | "creatim" | "revtim" | "printim" | "buptim" => {
                self.state().dest = Dest::Skip;
            }

            // Font table / style sheet entries
            "f" if dest == Dest::FontTable => {
                self.commit_font();
                self.pending_font = Some((param.unwrap_or(0), FontDef::default()));
            }
            "fcharset" if dest == Dest::FontTable => {
                if let Some((_, f)) = self.pending_font.as_mut() {
                    f.charset = param;
                }
            }
            "fprq" if dest == Dest::FontTable => {
                if let Some((_, f)) = self.pending_font.as_mut() {
                    f.mono |= param == Some(1);
                }
            }
            "fmodern" if dest == Dest::FontTable => {
                if let Some((_, f)) = self.pending_font.as_mut() {
                    f.mono = true;
                }
            }
            "s" if dest == Dest::StyleSheet => self.pending_style = Some((param, StyleDef::default())),
            "cs" | "ds" | "ts" if dest == Dest::StyleSheet => self.pending_style = Some((None, StyleDef::default())),
            "outlinelevel" if dest == Dest::StyleSheet => {
                if let Some((_, s)) = self.pending_style.as_mut() {
                    s.outline = param.and_then(|p| u8::try_from(p).ok()).filter(|p| *p < 9);
                }
            }

            // Document settings
            "ansicpg" => {
                if let Some(enc) = param.and_then(codepage_encoding) {
                    self.codepage = enc;
                }
            }
            "mac" => self.codepage = encoding_rs::MACINTOSH,
            "deff" => self.default_font = param,

            // Character formatting
            "plain" => {
                let font = self.default_font;
                let st = self.state();
                st.bold = false;
                st.italic = false;
                st.strike = false;
                st.hidden = false;
                st.font = font;
            }
            "b" => self.state().bold = on,
            "i" => self.state().italic = on,
            "strike" | "striked" => self.state().strike = on,
            "v" => self.state().hidden = on,
            "f" => self.state().font = param,
            "uc" => self.state().uc = param.unwrap_or(1).max(0) as usize,
            "u" => {
                if let Some(p) = param {
                    let unit = if p < 0 { (p + 65536) as u16 } else { p as u16 };
                    self.unicode(unit);
                }
                self.skip_chars = self.stack.last().map_or(1, |s| s.uc);
            }

            // Paragraph formatting
            "pard" => {
                let st = self.state();
                st.style = None;
                st.outline = None;
                st.list = None;
                st.ilvl = 0;
                st.intbl = false;
            }
            "s" => self.state().style = param,
            "outlinelevel" => self.state().outline = param.and_then(|p| u8::try_from(p).ok()).filter(|p| *p < 9),
            "ls" => self.state().list = param,
            "ilvl" => self.state().ilvl = param.unwrap_or(0).clamp(0, 8) as usize,
            "intbl" => self.state().intbl = true,
            "itap" => self.state().intbl = param.unwrap_or(1) > 0,

            // Breaks and structure
            "par" => self.end_paragraph(),
            "line" | "lbr" => self.target().para.line_break(),
            "tab" => self.emit_char('\t'),
            "cell" => self.end_cell(),
            "row" => self.end_row(),
            "nestcell" => self.emit_char(' '),
            "nestrow" => self.target().para.line_break(),

            // Special characters
            "emdash" => self.emit_char('—'),
            "endash" => self.emit_char('–'),
            "bullet" => self.emit_char('•'),
            "lquote" => self.emit_char('‘'),
            "rquote" => self.emit_char('’'),
            "ldblquote" => self.emit_char('“'),
            "rdblquote" => self.emit_char('”'),
            "emspace" | "enspace" | "qmspace" => self.emit_char(' '),

            _ => {
                if ignorable {
                    self.state().dest = Dest::Skip;
                }
            }
        }
    }

    fn unicode(&mut self, unit: u16) {
        if (0xD800..0xDC00).contains(&unit) {
            self.high_surrogate = Some(unit);
            return;
        }
        let ch = if (0xDC00..0xE000).contains(&unit) {
            match self.high_surrogate.take() {
                Some(hi) => char::decode_utf16([hi, unit]).next().and_then(|r| r.ok()),
                None => None,
            }
        } else {
            char::from_u32(u32::from(unit))
        };
        if let Some(ch) = ch {
            self.emit_char(ch);
        }
    }

    fn new_para_id(&mut self) -> usize {
        self.next_para_id += 1;
        self.next_para_id
    }

    // -- paragraphs and tables ---------------------------------------------

    fn end_paragraph(&mut self) {
        self.flush();
        let st = self.stack.last().cloned().unwrap_or_default();
        if st.intbl {
            let rb = std::mem::take(&mut self.target().para);
            let para_id = self.new_para_id();
            let t = self.target();
            t.para_id = para_id;
            if !t.cell.is_empty() {
                t.cell.push(Inline::LineBreak);
            }
            t.cell.extend(rb.finish());
            t.list_text.clear();
            return;
        }
        self.flush_table();

        let style = st.style.and_then(|s| self.styles.get(&s)).cloned();
        let heading = st
            .outline
            .or_else(|| style.as_ref().and_then(|s| s.outline))
            .map(|l| (l + 1).min(6))
            .or_else(|| style.as_ref().and_then(|s| style_name_heading_level(&s.name)));
        let style_name = style.map(|s| s.name.to_lowercase()).unwrap_or_default();

        let para_id = self.new_para_id();
        let t = self.target();
        let rb = std::mem::take(&mut t.para);
        t.para_id = para_id;
        let list_text = std::mem::take(&mut t.list_text);
        let marker = list_text.trim().trim_end_matches('\t').trim();

        let kind = if let Some(level) = heading {
            ParaKind::Heading(level)
        } else if st.list.is_some() || !marker.is_empty() {
            let (ordered, start) = parse_marker(marker);
            ParaKind::ListItem {
                level: st.ilvl,
                ordered,
                start,
                list_id: st.list.map_or_else(|| "pn".to_string(), |l| l.to_string()),
            }
        } else if rb.is_all_code() || style_name.contains("code") || style_name.contains("preformatted") {
            ParaKind::Code
        } else if style_name.contains("quote") {
            ParaKind::Quote
        } else {
            ParaKind::Normal
        };
        let inlines = if kind == ParaKind::Code { vec![Inline::Text(rb.plain_text())] } else { rb.finish() };
        self.target().items.push(Item::Para(Para { kind, inlines }));
    }

    fn end_cell(&mut self) {
        self.flush();
        let rb = std::mem::take(&mut self.target().para);
        let para_id = self.new_para_id();
        let t = self.target();
        t.para_id = para_id;
        let mut cell = std::mem::take(&mut t.cell);
        let content = rb.finish();
        if !cell.is_empty() && !content.is_empty() {
            cell.push(Inline::LineBreak);
        }
        cell.extend(content);
        trim_inlines(&mut cell);
        t.row.push(cell);
        t.list_text.clear();
    }

    fn end_row(&mut self) {
        let t = self.target();
        let row = std::mem::take(&mut t.row);
        if !row.is_empty() {
            t.rows.push(row);
        }
    }

    fn flush_table(&mut self) {
        let t = self.target();
        if !t.row.is_empty() {
            let row = std::mem::take(&mut t.row);
            t.rows.push(row);
        }
        if t.rows.is_empty() {
            return;
        }
        let rows = std::mem::take(&mut t.rows);
        t.items.push(Item::Block(Block::Table(Table { rows, alignments: Vec::new() })));
    }

    fn finish(mut self) -> Document {
        self.flush();
        while self.targets.len() > 1 {
            // Unterminated footnote group: close it.
            if let Some(pos) = self.scopes.iter().rposition(|(_, s)| matches!(s, Scope::Footnote { .. })) {
                let (_, scope) = self.scopes.remove(pos);
                self.finish_scope(scope);
            } else {
                self.targets.pop();
            }
        }
        if self.target().para.has_content() {
            self.end_paragraph();
        }
        self.flush_table();
        let target = self.targets.pop().expect("main target");
        Document { blocks: assemble(target.items), footnotes: self.footnotes }
    }
}

/// Classifies a list marker such as "1.", "a)", "iv." or "•".
fn parse_marker(marker: &str) -> (bool, u64) {
    let digits: String = marker.chars().take_while(char::is_ascii_digit).collect();
    if !digits.is_empty() {
        return (true, digits.parse().unwrap_or(1));
    }
    let mut chars = marker.chars();
    if let (Some(c), Some(p)) = (chars.next(), chars.next())
        && c.is_ascii_alphabetic()
        && matches!(p, '.' | ')')
    {
        return (true, 1);
    }
    let roman = marker.trim_end_matches(['.', ')']);
    if !roman.is_empty() && roman.len() < marker.len() && roman.chars().all(|c| "ivxlcdmIVXLCDM".contains(c)) {
        return (true, 1);
    }
    (false, 1)
}

fn hyperlink_url(instr: &str) -> Option<String> {
    let rest = instr.trim().strip_prefix("HYPERLINK")?.trim();
    let url = match rest.strip_prefix('"') {
        Some(s) => s.split('"').next().unwrap_or(""),
        None if !rest.starts_with('\\') => rest.split_whitespace().next().unwrap_or(""),
        None => "",
    };
    (!url.is_empty()).then(|| url.to_string())
}

fn codepage_encoding(cp: i32) -> Option<&'static Encoding> {
    use encoding_rs::*;
    Some(match cp {
        874 => WINDOWS_874,
        1250 => WINDOWS_1250,
        1251 => WINDOWS_1251,
        1252 => WINDOWS_1252,
        1253 => WINDOWS_1253,
        1254 => WINDOWS_1254,
        1255 => WINDOWS_1255,
        1256 => WINDOWS_1256,
        1257 => WINDOWS_1257,
        1258 => WINDOWS_1258,
        932 => SHIFT_JIS,
        936 => GBK,
        949 => EUC_KR,
        950 => BIG5,
        866 => IBM866,
        10000 => MACINTOSH,
        20866 => KOI8_R,
        54936 => GB18030,
        65001 => UTF_8,
        _ => return None,
    })
}

fn charset_encoding(charset: i32) -> Option<&'static Encoding> {
    codepage_encoding(match charset {
        77 => 10000,
        128 => 932,
        129 => 949,
        134 => 936,
        136 => 950,
        161 => 1253,
        162 => 1254,
        163 => 1258,
        177 => 1255,
        178 => 1256,
        186 => 1257,
        204 => 1251,
        222 => 874,
        238 => 1250,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use crate::{Options, convert_bytes};

    fn md(rtf: &str) -> String {
        convert_bytes(rtf.as_bytes(), None, &Options::default()).unwrap().markdown
    }

    #[test]
    fn converts_basic_rtf() {
        let rtf = r#"{\rtf1\ansi\ansicpg1252\deff0{\fonttbl{\f0\froman Times;}{\f1\fmodern Courier New;}}
{\stylesheet{\s1\outlinelevel0 heading 1;}}
{\info{\title My Doc}}
\pard\s1 Intro\par
\pard Hello \b bold\b0  and \i it\i0 . Caf\'e9 \u960?\par
{\listtext\'b7\tab}\pard\ls1 one\par
{\listtext\'b7\tab}\pard\ls1 two\par
\pard {\field{\*\fldinst HYPERLINK "https://example.com"}{\fldrslt link}} here\par
\pard\intbl A\cell B\cell\row
\pard\intbl 1\cell 2\cell\row
\pard Note{\footnote \pard Footnote text.}\par
\pard\f1 let x = 1;\par
}"#;
        let out = md(rtf);
        assert_eq!(
            out,
            "# Intro\n\nHello **bold** and *it*. Café π\n\n- one\n- two\n\n[link](https://example.com) here\n\n| A   | B   |\n| --- | --- |\n| 1   | 2   |\n\nNote[^1]\n\n```\nlet x = 1;\n```\n\n[^1]: Footnote text.\n"
        );
    }
}
