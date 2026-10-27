//! Byte-to-text decoding with BOM and encoding-declaration handling.

use encoding_rs::{Encoding, UTF_8, UTF_16BE, UTF_16LE, WINDOWS_1252};

/// Decodes plain text: honours BOMs, accepts valid UTF-8, and falls back to
/// Windows-1252 (the usual encoding of legacy CSV exports).
pub(crate) fn decode_text(data: &[u8]) -> String {
    if let Some((enc, bom_len)) = Encoding::for_bom(data) {
        return enc.decode_without_bom_handling(&data[bom_len..]).0.into_owned();
    }
    match std::str::from_utf8(data) {
        Ok(s) => s.to_string(),
        // A multi-byte sequence cut off at the end of a sample is still UTF-8.
        Err(e) if e.error_len().is_none() => String::from_utf8_lossy(data).into_owned(),
        Err(_) => WINDOWS_1252.decode_without_bom_handling(data).0.into_owned(),
    }
}

/// Decodes an XML/HTML document, honouring BOMs and the declared encoding.
pub(crate) fn decode_markup(data: &[u8]) -> String {
    if let Some((enc, bom_len)) = Encoding::for_bom(data) {
        return enc.decode_without_bom_handling(&data[bom_len..]).0.into_owned();
    }
    if let Some(enc) = declared_encoding(&data[..data.len().min(1024)])
        && enc != UTF_8
        && enc != UTF_16LE
        && enc != UTF_16BE
    {
        return enc.decode_without_bom_handling(data).0.into_owned();
    }
    String::from_utf8_lossy(data).into_owned()
}

fn declared_encoding(head: &[u8]) -> Option<&'static Encoding> {
    let head = String::from_utf8_lossy(head).to_ascii_lowercase();
    for key in ["encoding=", "charset="] {
        if let Some(pos) = head.find(key) {
            let rest = head[pos + key.len()..].trim_start_matches(['"', '\'', ' ']);
            let label: String =
                rest.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').collect();
            if let Some(enc) = Encoding::for_label(label.as_bytes()) {
                return Some(enc);
            }
        }
    }
    None
}
