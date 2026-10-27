//! ZIP container access shared by OOXML, ODF and EPUB, plus OPC
//! relationships and document-property readers.

use std::collections::HashMap;
use std::io::{Cursor, Read};

use crate::xml::{self, Element};
use crate::{Error, Format, Metadata};

/// Upper bound for a single decompressed entry (guards against zip bombs).
const MAX_ENTRY_SIZE: u64 = 1 << 30;

pub(crate) struct Package<'a> {
    archive: zip::ZipArchive<Cursor<&'a [u8]>>,
    /// Lowercased name -> actual entry name.
    names: HashMap<String, String>,
}

impl<'a> Package<'a> {
    pub fn open(data: &'a [u8], format: Format) -> Result<Self, Error> {
        let archive = zip::ZipArchive::new(Cursor::new(data)).map_err(|e| Error::invalid(format, e))?;
        let names =
            archive.file_names().filter_map(|n| n.ok()).map(|n| (n.to_ascii_lowercase(), n.into_owned())).collect();
        Ok(Package { archive, names })
    }

    pub fn file_names(&self) -> impl Iterator<Item = &str> {
        self.names.values().map(String::as_str)
    }

    pub fn read(&mut self, name: &str) -> Option<Vec<u8>> {
        let actual = self.names.get(&normalize(name).to_ascii_lowercase())?.clone();
        let entry = self.archive.by_name(&actual).ok()?;
        let mut data = Vec::with_capacity(entry.size().min(64 << 20) as usize);
        entry.take(MAX_ENTRY_SIZE).read_to_end(&mut data).ok()?;
        Some(data)
    }

    pub fn read_xml(&mut self, name: &str) -> Option<Element> {
        self.read(name).map(|d| xml::parse_bytes(&d, false))
    }

    /// Relationships of an OPC part, e.g. `word/document.xml`.
    pub fn rels(&mut self, part: &str) -> Rels {
        let (dir, file) = split_dir(part);
        let rels_path = if dir.is_empty() { format!("_rels/{file}.rels") } else { format!("{dir}/_rels/{file}.rels") };
        let mut rels = Rels::default();
        let Some(root) = self.read_xml(&rels_path) else { return rels };
        let Some(list) = root.elements().find(|e| e.local() == "Relationships") else { return rels };
        for rel in list.elements().filter(|e| e.local() == "Relationship") {
            let (Some(id), Some(target)) = (rel.attr("Id"), rel.attr("Target")) else { continue };
            let external = rel.attr("TargetMode").is_some_and(|m| m.eq_ignore_ascii_case("External"));
            let kind = rel.attr("Type").unwrap_or("").rsplit('/').next().unwrap_or("").to_string();
            let target = if external { target.to_string() } else { resolve(dir, target) };
            rels.0.insert(id.to_string(), Rel { target, kind, external });
        }
        rels
    }

    /// Target of the package-level relationship of the given type (e.g. `officeDocument`).
    pub fn main_part(&mut self, kind: &str, fallback: &str) -> String {
        let rels = self.rels("");
        rels.0
            .values()
            .find(|r| r.kind == kind && !r.external)
            .map_or_else(|| fallback.to_string(), |r| r.target.clone())
    }

    /// Reads OOXML `docProps/core.xml` into `meta`.
    pub fn read_core_properties(&mut self, meta: &mut Metadata) {
        let Some(root) = self.read_xml("docProps/core.xml") else { return };
        let Some(props) = root.elements().next() else { return };
        for el in props.elements() {
            let value = el.text().trim().to_string();
            if value.is_empty() {
                continue;
            }
            let slot = match el.local() {
                "title" => &mut meta.title,
                "creator" => &mut meta.author,
                "subject" => &mut meta.subject,
                "description" => &mut meta.description,
                "keywords" => &mut meta.keywords,
                "language" => &mut meta.language,
                "created" => &mut meta.created,
                "modified" => &mut meta.modified,
                _ => continue,
            };
            *slot = Some(value);
        }
    }

    /// Reads ODF `meta.xml` into `meta`.
    pub fn read_odf_meta(&mut self, meta: &mut Metadata) {
        let Some(root) = self.read_xml("meta.xml") else { return };
        let Some(office_meta) = root.find("office:meta") else { return };
        for el in office_meta.elements() {
            let value = el.text().trim().to_string();
            if value.is_empty() {
                continue;
            }
            let slot = match el.name.as_str() {
                "dc:title" => &mut meta.title,
                "meta:initial-creator" => &mut meta.author,
                "dc:creator" if meta.author.is_none() => &mut meta.author,
                "dc:subject" => &mut meta.subject,
                "dc:description" => &mut meta.description,
                "meta:keyword" => &mut meta.keywords,
                "dc:language" => &mut meta.language,
                "meta:creation-date" => &mut meta.created,
                "dc:date" => &mut meta.modified,
                _ => continue,
            };
            *slot = Some(value);
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Rel {
    /// Resolved part path inside the package, or the URL for external targets.
    pub target: String,
    /// Last segment of the relationship type, e.g. `image`, `hyperlink`.
    pub kind: String,
    pub external: bool,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Rels(pub HashMap<String, Rel>);

impl Rels {
    pub fn get(&self, id: &str) -> Option<&Rel> {
        self.0.get(id)
    }
}

fn normalize(name: &str) -> &str {
    name.trim_start_matches('/')
}

/// Splits `a/b/c.xml` into (`a/b`, `c.xml`).
pub(crate) fn split_dir(path: &str) -> (&str, &str) {
    match path.rsplit_once('/') {
        Some((d, f)) => (d, f),
        None => ("", path),
    }
}

/// Resolves a relative `target` against `base_dir` inside a package.
pub(crate) fn resolve(base_dir: &str, target: &str) -> String {
    let target = target.split(['#', '?']).next().unwrap_or(target);
    let target = percent_decode(target);
    let mut parts: Vec<&str> =
        if target.starts_with('/') { Vec::new() } else { base_dir.split('/').filter(|p| !p.is_empty()).collect() };
    for seg in target.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            s => parts.push(s),
        }
    }
    parts.join("/")
}

pub(crate) fn percent_decode(s: &str) -> String {
    if !s.contains('%') {
        return s.to_string();
    }
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = |b: u8| (b as char).to_digit(16);
            if let (Some(hi), Some(lo)) = (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                out.push((hi * 16 + lo) as u8);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_relative_targets() {
        assert_eq!(resolve("word", "media/image1.png"), "word/media/image1.png");
        assert_eq!(resolve("ppt/slides", "../media/a%20b.png"), "ppt/media/a b.png");
        assert_eq!(resolve("OEBPS/text", "/OEBPS/img.jpg"), "OEBPS/img.jpg");
        assert_eq!(resolve("", "word/document.xml"), "word/document.xml");
    }
}
