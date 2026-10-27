//! Input format detection from content (magic bytes, ZIP layout) with the
//! file name as a fallback hint.

use std::fmt;
use std::io::{Cursor, Read};
use std::str::FromStr;

/// A supported input format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize), serde(rename_all = "lowercase"))]
pub enum Format {
    Docx,
    Pptx,
    Xlsx,
    Xlsb,
    Xls,
    Odt,
    Odp,
    Ods,
    Rtf,
    Epub,
    Csv,
    Html,
    Pdf,
}

impl Format {
    pub const ALL: [Format; 13] = [
        Format::Docx,
        Format::Pptx,
        Format::Xlsx,
        Format::Xlsb,
        Format::Xls,
        Format::Odt,
        Format::Odp,
        Format::Ods,
        Format::Rtf,
        Format::Epub,
        Format::Csv,
        Format::Html,
        Format::Pdf,
    ];

    /// Short lowercase name, e.g. `"docx"`.
    pub fn name(self) -> &'static str {
        match self {
            Format::Docx => "docx",
            Format::Pptx => "pptx",
            Format::Xlsx => "xlsx",
            Format::Xlsb => "xlsb",
            Format::Xls => "xls",
            Format::Odt => "odt",
            Format::Odp => "odp",
            Format::Ods => "ods",
            Format::Rtf => "rtf",
            Format::Epub => "epub",
            Format::Csv => "csv",
            Format::Html => "html",
            Format::Pdf => "pdf",
        }
    }

    /// Human-readable description.
    pub fn description(self) -> &'static str {
        match self {
            Format::Docx => "Word document",
            Format::Pptx => "PowerPoint presentation",
            Format::Xlsx => "Excel workbook",
            Format::Xlsb => "Excel binary workbook",
            Format::Xls => "Excel 97-2003 workbook",
            Format::Odt => "OpenDocument text",
            Format::Odp => "OpenDocument presentation",
            Format::Ods => "OpenDocument spreadsheet",
            Format::Rtf => "Rich Text Format",
            Format::Epub => "EPUB e-book",
            Format::Csv => "CSV/TSV table",
            Format::Html => "HTML/XHTML page",
            Format::Pdf => "PDF (text-based)",
        }
    }

    /// File extensions associated with this format.
    pub fn extensions(self) -> &'static [&'static str] {
        match self {
            Format::Docx => &["docx", "docm", "dotx", "dotm"],
            Format::Pptx => &["pptx", "pptm", "potx", "potm", "ppsx", "ppsm"],
            Format::Xlsx => &["xlsx", "xlsm", "xltx", "xltm"],
            Format::Xlsb => &["xlsb"],
            Format::Xls => &["xls", "xla"],
            Format::Odt => &["odt", "ott"],
            Format::Odp => &["odp", "otp"],
            Format::Ods => &["ods", "ots"],
            Format::Rtf => &["rtf"],
            Format::Epub => &["epub"],
            Format::Csv => &["csv", "tsv", "tab"],
            Format::Html => &["html", "htm", "xhtml"],
            Format::Pdf => &["pdf"],
        }
    }

    /// Looks up a format by file extension (case-insensitive, with or without a dot).
    pub fn from_extension(ext: &str) -> Option<Format> {
        let ext = ext.trim_start_matches('.').to_ascii_lowercase();
        Format::ALL.into_iter().find(|f| f.extensions().contains(&ext.as_str()))
    }

    /// Looks up a format from a file name or path.
    pub fn from_filename(name: &str) -> Option<Format> {
        let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
        let (_, ext) = base.rsplit_once('.')?;
        Format::from_extension(ext)
    }
}

impl fmt::Display for Format {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

impl FromStr for Format {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Format::from_extension(s).ok_or_else(|| {
            let names: Vec<&str> = Format::ALL.iter().map(|f| f.name()).collect();
            format!("unknown format '{s}' (expected one of: {})", names.join(", "))
        })
    }
}

/// What the content looks like when it is not a supported format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Unsupported {
    LegacyWord,
    LegacyPowerPoint,
}

/// Detects the format of `data`, using `filename` only when the content is ambiguous.
pub fn detect_format(data: &[u8], filename: Option<&str>) -> Option<Format> {
    detect_content(data).or_else(|| {
        let hinted = filename.and_then(Format::from_filename)?;
        // Only trust the extension for formats we can't reliably sniff.
        match hinted {
            Format::Csv | Format::Html => looks_like_text(data).then_some(hinted),
            _ => None,
        }
    })
}

fn detect_content(data: &[u8]) -> Option<Format> {
    let head = &data[..data.len().min(1024)];
    if memfind(head, b"%PDF-").is_some() {
        return Some(Format::Pdf);
    }
    let text_start = skip_bom_and_space(data);
    if data[text_start..].starts_with(b"{\\rtf") {
        return Some(Format::Rtf);
    }
    if data.starts_with(b"PK\x03\x04") {
        return detect_zip(data);
    }
    if data.starts_with(&[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1]) {
        return if contains_utf16(data, "Workbook") || contains_utf16(data, "Book") { Some(Format::Xls) } else { None };
    }
    if looks_like_html(head) {
        return Some(Format::Html);
    }
    if looks_like_csv(data) {
        return Some(Format::Csv);
    }
    None
}

pub(crate) fn detect_unsupported(data: &[u8]) -> Option<Unsupported> {
    if !data.starts_with(&[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1]) {
        return None;
    }
    if contains_utf16(data, "WordDocument") {
        Some(Unsupported::LegacyWord)
    } else if contains_utf16(data, "PowerPoint Document") {
        Some(Unsupported::LegacyPowerPoint)
    } else {
        None
    }
}

fn detect_zip(data: &[u8]) -> Option<Format> {
    let mut archive = zip::ZipArchive::new(Cursor::new(data)).ok()?;
    if let Ok(mut entry) = archive.by_name("mimetype") {
        let mut mime = String::new();
        if entry.read_to_string(&mut mime).is_ok() {
            let mime = mime.trim();
            if mime == "application/epub+zip" {
                return Some(Format::Epub);
            }
            if let Some(kind) = mime.strip_prefix("application/vnd.oasis.opendocument.") {
                let kind = kind.trim_end_matches("-template");
                match kind {
                    "text" | "text-master" => return Some(Format::Odt),
                    "presentation" => return Some(Format::Odp),
                    "spreadsheet" => return Some(Format::Ods),
                    _ => {}
                }
            }
        }
    }
    let names: Vec<String> = archive.file_names().filter_map(|n| n.ok().map(|n| n.into_owned())).collect();
    let has = |n: &str| names.iter().any(|x| x.eq_ignore_ascii_case(n));
    if has("word/document.xml") {
        Some(Format::Docx)
    } else if has("ppt/presentation.xml") {
        Some(Format::Pptx)
    } else if has("xl/workbook.xml") {
        Some(Format::Xlsx)
    } else if has("xl/workbook.bin") {
        Some(Format::Xlsb)
    } else if has("META-INF/container.xml") {
        Some(Format::Epub)
    } else if has("content.xml") {
        // ODF package without a mimetype entry: look at the body element.
        let mut content = String::new();
        archive.by_name("content.xml").ok()?.take(64 * 1024).read_to_string(&mut content).ok()?;
        if content.contains("<office:text") {
            Some(Format::Odt)
        } else if content.contains("<office:presentation") {
            Some(Format::Odp)
        } else if content.contains("<office:spreadsheet") {
            Some(Format::Ods)
        } else {
            None
        }
    } else {
        None
    }
}

fn skip_bom_and_space(data: &[u8]) -> usize {
    let mut i = if data.starts_with(&[0xEF, 0xBB, 0xBF]) { 3 } else { 0 };
    while i < data.len() && data[i].is_ascii_whitespace() {
        i += 1;
    }
    i
}

fn memfind(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    memchr::memmem::find(haystack, needle)
}

fn contains_utf16(data: &[u8], name: &str) -> bool {
    let needle: Vec<u8> = name.encode_utf16().flat_map(u16::to_le_bytes).collect();
    memfind(data, &needle).is_some()
}

fn looks_like_html(head: &[u8]) -> bool {
    let start = skip_bom_and_space(head);
    if head.get(start) != Some(&b'<') {
        return false;
    }
    let lower = String::from_utf8_lossy(head).to_ascii_lowercase();
    lower.contains("<!doctype html") || lower.contains("<html")
}

pub(crate) fn looks_like_text(data: &[u8]) -> bool {
    if data.starts_with(&[0xFF, 0xFE]) || data.starts_with(&[0xFE, 0xFF]) {
        return true;
    }
    let sample = &data[..data.len().min(8192)];
    !sample.contains(&0)
}

fn looks_like_csv(data: &[u8]) -> bool {
    if !looks_like_text(data) || data.is_empty() {
        return false;
    }
    let text = crate::text::decode_text(&data[..data.len().min(16 * 1024)]);
    crate::formats::csv::sniff_delimiter(&text).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_by_magic() {
        assert_eq!(detect_format(b"%PDF-1.7 ...", None), Some(Format::Pdf));
        assert_eq!(detect_format(b"{\\rtf1\\ansi hello}", None), Some(Format::Rtf));
        assert_eq!(detect_format(b"<!DOCTYPE html><html></html>", None), Some(Format::Html));
        assert_eq!(detect_format(b"a,b,c\n1,2,3\n4,5,6\n", None), Some(Format::Csv));
        assert_eq!(detect_format(b"just some words", None), None);
        assert_eq!(detect_format(b"x", Some("data.tsv")), Some(Format::Csv));
    }

    #[test]
    fn parses_names() {
        assert_eq!("DOCX".parse::<Format>().unwrap(), Format::Docx);
        assert_eq!(Format::from_filename("C:\\a\\b.Report.PPTX"), Some(Format::Pptx));
        assert!("nope".parse::<Format>().is_err());
    }
}
