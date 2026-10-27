//! CSV / TSV with delimiter sniffing and encoding detection.

use crate::model::*;
use crate::text::decode_text;
use crate::{Ctx, Error, Format, Result};

const CANDIDATES: [u8; 4] = *b",;\t|";

pub(crate) fn convert(data: &[u8], ctx: &mut Ctx) -> Result<Document> {
    let text = decode_text(data);
    let hinted_tab = ctx
        .filename
        .as_deref()
        .and_then(|n| n.rsplit_once('.'))
        .is_some_and(|(_, ext)| ext.eq_ignore_ascii_case("tsv") || ext.eq_ignore_ascii_case("tab"));
    let delimiter = if hinted_tab { b'\t' } else { sniff_delimiter(&text).unwrap_or(b',') };

    let mut reader =
        csv::ReaderBuilder::new().has_headers(false).flexible(true).delimiter(delimiter).from_reader(text.as_bytes());

    let mut rows: Vec<Vec<Vec<Inline>>> = Vec::new();
    for record in reader.records() {
        let record = record.map_err(|e| Error::invalid(Format::Csv, e))?;
        rows.push(record.iter().map(cell_inlines).collect());
    }
    while rows.last().is_some_and(|r| r.iter().all(Vec::is_empty)) {
        rows.pop();
    }
    let blocks = if rows.is_empty() { Vec::new() } else { vec![Block::Table(Table { rows, alignments: Vec::new() })] };
    Ok(Document { blocks, footnotes: Vec::new() })
}

pub(crate) fn cell_inlines(value: &str) -> Vec<Inline> {
    let value = value.trim_matches(|c: char| c == '\r' || c == '\n');
    let mut out = Vec::new();
    for (i, line) in value.lines().enumerate() {
        if i > 0 {
            out.push(Inline::LineBreak);
        }
        if !line.is_empty() {
            out.push(Inline::text(line.trim_end_matches('\r')));
        }
    }
    out
}

/// Picks the delimiter that splits the first lines into a consistent,
/// non-trivial number of fields. Returns `None` if nothing looks tabular.
pub(crate) fn sniff_delimiter(text: &str) -> Option<u8> {
    let records = split_records(text, 30);
    if records.is_empty() {
        return None;
    }
    // The last sampled record may be cut short by a sample boundary.
    let sample = if records.len() > 2 { &records[..records.len() - 1] } else { &records[..] };

    let mut best: Option<(u8, usize, f64)> = None;
    for &d in &CANDIDATES {
        let counts: Vec<usize> = sample.iter().map(|l| count_outside_quotes(l, d)).collect();
        let mut freq = std::collections::HashMap::new();
        for &c in &counts {
            *freq.entry(c).or_insert(0usize) += 1;
        }
        let Some((&mode, &n)) = freq.iter().filter(|(c, _)| **c > 0).max_by_key(|(c, n)| (**n, **c)) else { continue };
        let consistency = n as f64 / counts.len() as f64;
        let ok = if counts.len() == 1 { mode >= 1 } else { consistency >= 0.8 };
        if ok && best.is_none_or(|(_, m, c)| (consistency, mode) > (c, m)) {
            best = Some((d, mode, consistency));
        }
    }
    best.map(|(d, _, _)| d)
}

/// Splits text into records, keeping newlines inside quoted fields.
fn split_records(text: &str, max: usize) -> Vec<&str> {
    let mut records = Vec::new();
    let mut in_quotes = false;
    let mut start = 0;
    for (i, b) in text.bytes().enumerate() {
        match b {
            b'"' => in_quotes = !in_quotes,
            b'\n' if !in_quotes => {
                let record = text[start..i].trim_end_matches('\r');
                if !record.trim().is_empty() {
                    records.push(record);
                    if records.len() == max {
                        return records;
                    }
                }
                start = i + 1;
            }
            _ => {}
        }
    }
    if !text[start..].trim().is_empty() {
        records.push(&text[start..]);
    }
    records
}

fn count_outside_quotes(line: &str, delimiter: u8) -> usize {
    let mut in_quotes = false;
    let mut count = 0;
    for &b in line.as_bytes() {
        if b == b'"' {
            in_quotes = !in_quotes;
        } else if b == delimiter && !in_quotes {
            count += 1;
        }
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sniffs_delimiters() {
        assert_eq!(sniff_delimiter("a;b;c\n1;2,5;3\n4;5;6\n"), Some(b';'));
        assert_eq!(sniff_delimiter("a\tb\n1\t2\n"), Some(b'\t'));
        assert_eq!(sniff_delimiter("\"x,y\",z\n\"1,2\",3\n"), Some(b','));
        assert_eq!(sniff_delimiter("hello world\nplain text\n"), None);
        assert_eq!(sniff_delimiter("n;c;v\n\"S, A\";SP;9,5\nJ;\"Rio\nde J\";8\n"), Some(b';'));
    }
}
