//! Spreadsheets (.xlsx, .xlsm, .xlsb, .xls, .ods) via calamine.
//!
//! Each sheet becomes a section; blocks of rows separated by empty rows
//! become separate tables, each with its first row as the header.

use std::io::Cursor;

use calamine::{Data, Reader, SheetVisible, open_workbook_auto_from_rs};

use crate::formats::csv::cell_inlines;
use crate::model::*;
use crate::package::Package;
use crate::{Ctx, Error, Format, ResourceKind, Result};

pub(crate) fn convert(format: Format, data: &[u8], ctx: &mut Ctx) -> Result<Document> {
    if matches!(format, Format::Xlsx | Format::Xlsb | Format::Ods)
        && let Ok(mut pkg) = Package::open(data, format)
    {
        if format == Format::Ods {
            pkg.read_odf_meta(&mut ctx.metadata);
        } else {
            pkg.read_core_properties(&mut ctx.metadata);
        }
    }

    let mut workbook = open_workbook_auto_from_rs(Cursor::new(data)).map_err(|e| Error::invalid(format, e))?;
    let sheets: Vec<(String, bool)> =
        workbook.sheets_metadata().iter().map(|s| (s.name.clone(), s.visible != SheetVisible::Visible)).collect();
    ctx.metadata.pages = Some(sheets.len());

    let included: Vec<&(String, bool)> =
        sheets.iter().filter(|(_, hidden)| ctx.opts.include_hidden || !hidden).collect();
    let with_headings = included.len() > 1;
    let mut blocks = Vec::new();
    for (name, hidden) in included {
        let range = match workbook.worksheet_range(name) {
            Ok(r) => r,
            Err(e) => {
                ctx.warn(format!("sheet '{name}' could not be read: {e}"));
                continue;
            }
        };
        if with_headings {
            let title = if *hidden { format!("{name} (hidden)") } else { name.clone() };
            blocks.push(Block::Heading { level: 2, content: vec![Inline::Text(title)] });
        }
        let rows: Vec<Vec<String>> = range.rows().map(|row| row.iter().map(cell_text).collect()).collect();
        blocks.extend(rows_to_tables(rows));
    }

    if let Some(pictures) = workbook.pictures() {
        for (i, (ext, bytes)) in pictures.into_iter().enumerate() {
            let name = format!("image{}.{}", i + 1, ext);
            let src = ctx.add_resource(&format!("picture:{i}"), &name, bytes, ResourceKind::Image);
            blocks.push(Block::Paragraph(vec![Inline::Image { src, alt: String::new(), title: None }]));
        }
    }

    Ok(Document { blocks, footnotes: Vec::new() })
}

fn rows_to_tables(rows: Vec<Vec<String>>) -> Vec<Block> {
    let width = rows.iter().map(Vec::len).max().unwrap_or(0);
    // Drop columns that are empty in every row.
    let keep: Vec<usize> =
        (0..width).filter(|&c| rows.iter().any(|r| r.get(c).is_some_and(|v| !v.is_empty()))).collect();
    let mut tables = Vec::new();
    let mut current: Vec<Vec<Vec<Inline>>> = Vec::new();
    for row in rows {
        if row.iter().all(String::is_empty) {
            if !current.is_empty() {
                tables.push(Block::Table(Table { rows: std::mem::take(&mut current), alignments: Vec::new() }));
            }
            continue;
        }
        current.push(keep.iter().map(|&c| row.get(c).map(|v| cell_inlines(v)).unwrap_or_default()).collect());
    }
    if !current.is_empty() {
        tables.push(Block::Table(Table { rows: current, alignments: Vec::new() }));
    }
    tables
}

fn cell_text(cell: &Data) -> String {
    match cell {
        Data::Empty => String::new(),
        Data::String(s) => s.clone(),
        Data::Int(i) => i.to_string(),
        Data::Float(f) => format_number(*f),
        Data::Bool(b) => if *b { "TRUE" } else { "FALSE" }.to_string(),
        Data::DateTime(dt) => {
            let serial = dt.as_f64();
            if dt.is_duration() {
                return format_duration(serial);
            }
            match dt.as_datetime() {
                Some(ndt) if serial < 1.0 => ndt.format("%H:%M:%S").to_string(),
                Some(ndt) => {
                    let s = ndt.to_string();
                    s.strip_suffix(" 00:00:00").map(str::to_string).unwrap_or(s)
                }
                None => format_number(serial),
            }
        }
        Data::DateTimeIso(s) | Data::DurationIso(s) => s.clone(),
        Data::Error(e) => e.to_string(),
    }
}

/// Formats like a spreadsheet would: integers without `.0`, and floating
/// noise removed by rounding to 15 significant digits.
fn format_number(f: f64) -> String {
    if !f.is_finite() {
        return f.to_string();
    }
    if f.fract() == 0.0 && f.abs() < 1e15 {
        return format!("{}", f as i64);
    }
    let rounded: f64 = format!("{f:.14e}").parse().unwrap_or(f);
    format!("{rounded}")
}

fn format_duration(days: f64) -> String {
    let total = (days * 86400.0).round() as i64;
    let (h, m, s) = (total / 3600, (total % 3600) / 60, total % 60);
    format!("{h}:{m:02}:{s:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_numbers() {
        assert_eq!(format_number(3.0), "3");
        assert_eq!(format_number(0.1 + 0.2), "0.3");
        assert_eq!(format_number(-2.5), "-2.5");
        assert_eq!(format_duration(1.5), "36:00:00");
    }
}
