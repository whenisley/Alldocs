//! One module per input format. Each produces a [`Document`].

pub(crate) mod csv;
mod docx;
mod epub;
mod html;
mod odf;
#[cfg(feature = "pdf")]
mod pdf;
mod pptx;
mod rtf;
mod sheet;

use crate::model::Document;
use crate::{Ctx, Format, Result};

pub(crate) fn convert(format: Format, data: &[u8], ctx: &mut Ctx) -> Result<Document> {
    match format {
        Format::Docx => docx::convert(data, ctx),
        Format::Pptx => pptx::convert(data, ctx),
        Format::Xlsx | Format::Xlsb | Format::Xls | Format::Ods => sheet::convert(format, data, ctx),
        Format::Odt | Format::Odp => odf::convert(format, data, ctx),
        Format::Rtf => rtf::convert(data, ctx),
        Format::Epub => epub::convert(data, ctx),
        Format::Csv => csv::convert(data, ctx),
        Format::Html => html::convert(data, ctx),
        #[cfg(feature = "pdf")]
        Format::Pdf => pdf::convert(data, ctx),
        #[cfg(not(feature = "pdf"))]
        Format::Pdf => {
            Err(crate::Error::Unsupported("PDF support was disabled at build time (enable the `pdf` feature)".into()))
        }
    }
}

/// True for font families that are monospaced (used to detect code).
pub(crate) fn is_monospace_font(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    [
        "courier",
        "consolas",
        "mono",
        "menlo",
        "monaco",
        "lucida console",
        "lucida sans typewriter",
        "source code",
        "fira code",
        "inconsolata",
        "cascadia",
        "andale",
        "fixedsys",
        "terminal",
        "typewriter",
        "ocr a",
        "letter gothic",
    ]
    .iter()
    .any(|k| lower.contains(k))
}

/// Heading level implied by a paragraph style name such as "heading 2",
/// "Title" or localized names like "Título 2" / "Überschrift 2".
pub(crate) fn style_name_heading_level(name: &str) -> Option<u8> {
    let lower = name.trim().to_lowercase();
    if lower == "title" || lower == "título" || lower == "titulo" || lower == "titre" || lower == "titel" {
        return Some(1);
    }
    const PREFIXES: &[&str] = &[
        "heading",
        "título",
        "titulo",
        "überschrift",
        "titre",
        "kop",
        "rubrik",
        "otsikko",
        "nagłówek",
        "заголовок",
        "intestazione",
        "encabezado",
        "cabeçalho",
    ];
    for prefix in PREFIXES {
        if let Some(rest) = lower.strip_prefix(prefix) {
            let digits: String = rest.trim_start_matches([' ', '_']).chars().take_while(char::is_ascii_digit).collect();
            if let Ok(n) = digits.parse::<u8>()
                && (1..=9).contains(&n)
            {
                return Some(n.min(6));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heading_names() {
        assert_eq!(style_name_heading_level("heading 2"), Some(2));
        assert_eq!(style_name_heading_level("Heading_20_3"), None);
        assert_eq!(style_name_heading_level("Título 1"), Some(1));
        assert_eq!(style_name_heading_level("Title"), Some(1));
        assert_eq!(style_name_heading_level("Normal"), None);
        assert!(is_monospace_font("Courier New"));
        assert!(is_monospace_font("JetBrains Mono"));
        assert!(!is_monospace_font("Calibri"));
    }
}
