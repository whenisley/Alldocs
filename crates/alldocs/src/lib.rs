//! # alldocs
//!
//! Fast, local conversion of Word, PowerPoint, Excel, OpenDocument, RTF,
//! EPUB, CSV, HTML and text-based PDF files to GitHub Flavored Markdown.
//!
//! ```no_run
//! let result = alldocs::convert_file("report.docx", &alldocs::Options::default())?;
//! println!("{}", result.markdown);
//! result.save("out/report.md")?; // writes the Markdown plus extracted images
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! Every format is converted into a common [`Document`] model and rendered by
//! the same Markdown writer, so headings, lists, tables, links, footnotes,
//! code blocks and images look the same whatever the source format.

use std::collections::{HashMap, HashSet};
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};

mod detect;
mod formats;
pub mod markdown;
mod model;
mod package;
mod runs;
mod text;
mod xml;

pub use detect::{Format, detect_format};
pub use model::*;

/// Errors returned by the converter.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
    #[error("could not detect the input format; pass the format explicitly (supported: {})", supported_list())]
    UnknownFormat,
    #[error("{0}")]
    Unsupported(String),
    #[error("invalid {format} file: {message}")]
    Invalid { format: Format, message: String },
}

fn supported_list() -> String {
    Format::ALL.iter().map(|f| f.name()).collect::<Vec<_>>().join(", ")
}

impl Error {
    pub(crate) fn invalid(format: Format, message: impl std::fmt::Display) -> Self {
        Error::Invalid { format, message: message.to_string() }
    }
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// How images are represented in the Markdown output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize), serde(rename_all = "lowercase"))]
pub enum ImageMode {
    /// `![alt](media/image1.png)` — the bytes are in [`Conversion::resources`].
    #[default]
    Reference,
    /// Inline `data:` URIs, so the Markdown is self-contained.
    Embed,
    /// Leave images out of the Markdown (resources are still returned).
    Omit,
}

impl std::str::FromStr for ImageMode {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "ref" | "reference" | "link" => Ok(ImageMode::Reference),
            "embed" | "inline" | "base64" => Ok(ImageMode::Embed),
            "omit" | "none" | "skip" => Ok(ImageMode::Omit),
            _ => Err(format!("unknown image mode '{s}' (expected ref, embed or omit)")),
        }
    }
}

/// Conversion options. `Options::default()` is right for most uses.
#[derive(Debug, Clone)]
pub struct Options {
    /// Force an input format instead of detecting it.
    pub format: Option<Format>,
    /// How images appear in the Markdown.
    pub images: ImageMode,
    /// Directory prefix used for resource links, e.g. `media` gives
    /// `![](media/image1.png)`. Relative to the Markdown file.
    pub media_dir: String,
    /// Include speaker notes from presentations.
    pub include_notes: bool,
    /// Include hidden slides and spreadsheet sheets.
    pub include_hidden: bool,
    /// Prepend YAML front matter with the document metadata.
    pub front_matter: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            format: None,
            images: ImageMode::Reference,
            media_dir: "media".to_string(),
            include_notes: true,
            include_hidden: true,
            front_matter: false,
        }
    }
}

/// Kind of embedded resource.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize), serde(rename_all = "lowercase"))]
pub enum ResourceKind {
    Image,
    /// An embedded object (OLE object, attached file, nested document).
    Object,
}

/// An image or embedded object extracted from the document.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct Resource {
    /// Unique file name, e.g. `image1.png`.
    pub id: String,
    /// Path used in the Markdown, e.g. `media/image1.png`.
    pub path: String,
    pub mime_type: String,
    pub kind: ResourceKind,
    /// Where the data came from inside the source document, if known.
    pub source: Option<String>,
    /// True when the Markdown links to `path` (so [`Conversion::save`] writes it).
    pub referenced: bool,
    #[cfg_attr(feature = "serde", serde(skip))]
    pub data: Vec<u8>,
}

/// Document properties, when the format provides them.
#[derive(Debug, Clone, Default, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct Metadata {
    pub title: Option<String>,
    pub author: Option<String>,
    pub subject: Option<String>,
    pub description: Option<String>,
    pub keywords: Option<String>,
    pub language: Option<String>,
    pub created: Option<String>,
    pub modified: Option<String>,
    /// Number of pages, slides or sheets.
    pub pages: Option<usize>,
}

/// The result of a conversion.
#[derive(Debug, Clone)]
pub struct Conversion {
    pub markdown: String,
    pub format: Format,
    pub metadata: Metadata,
    /// Images and embedded objects, with their bytes.
    pub resources: Vec<Resource>,
    /// Non-fatal problems (e.g. a PDF page without a text layer).
    pub warnings: Vec<String>,
    /// The intermediate document model the Markdown was rendered from.
    pub document: Document,
}

impl Conversion {
    /// Writes the Markdown to `path` and every referenced resource next to it
    /// (at `<dir of path>/<resource.path>`). Returns the files written.
    pub fn save(&self, path: impl AsRef<Path>) -> io::Result<Vec<PathBuf>> {
        let path = path.as_ref();
        let dir = path.parent().unwrap_or(Path::new(""));
        if !dir.as_os_str().is_empty() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, &self.markdown)?;
        let mut written = vec![path.to_path_buf()];
        for res in self.resources.iter().filter(|r| r.referenced) {
            written.push(write_resource(dir, &res.path, &res.data)?);
        }
        Ok(written)
    }

    /// Writes every resource (referenced or not) into `dir` using its `id`
    /// as the file name. Returns the files written.
    pub fn save_resources(&self, dir: impl AsRef<Path>) -> io::Result<Vec<PathBuf>> {
        let dir = dir.as_ref();
        self.resources.iter().map(|r| write_resource(dir, &r.id, &r.data)).collect()
    }
}

fn write_resource(dir: &Path, rel: &str, data: &[u8]) -> io::Result<PathBuf> {
    // Only plain relative components: never write outside `dir`.
    let mut target = dir.to_path_buf();
    for comp in Path::new(rel).components() {
        if let Component::Normal(part) = comp {
            target.push(part);
        }
    }
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&target, data)?;
    Ok(target)
}

/// Converts a file on disk. The format is detected from the content, with
/// the file extension as a fallback.
pub fn convert_file(path: impl AsRef<Path>, options: &Options) -> Result<Conversion> {
    let path = path.as_ref();
    let data = std::fs::read(path)?;
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned());
    convert_bytes(&data, name.as_deref(), options)
}

/// Converts everything read from `reader` (e.g. stdin).
pub fn convert_reader(mut reader: impl Read, filename: Option<&str>, options: &Options) -> Result<Conversion> {
    let mut data = Vec::new();
    reader.read_to_end(&mut data)?;
    convert_bytes(&data, filename, options)
}

/// Converts an in-memory document. `filename` (or just an extension such as
/// `"report.docx"`) is only used as a hint when the content is ambiguous.
pub fn convert_bytes(data: &[u8], filename: Option<&str>, options: &Options) -> Result<Conversion> {
    let format = match options.format {
        Some(f) => f,
        None => match detect_format(data, filename) {
            Some(f) => f,
            None => {
                return Err(match detect::detect_unsupported(data) {
                    Some(detect::Unsupported::LegacyWord) => Error::Unsupported(
                        "legacy Word 97-2003 (.doc) files are not supported; save the file as .docx".into(),
                    ),
                    Some(detect::Unsupported::LegacyPowerPoint) => Error::Unsupported(
                        "legacy PowerPoint 97-2003 (.ppt) files are not supported; save the file as .pptx".into(),
                    ),
                    None => Error::UnknownFormat,
                });
            }
        },
    };

    let mut ctx = Ctx::new(options, filename);
    let mut doc = formats::convert(format, data, &mut ctx)?;
    ctx.finish(&mut doc);

    let mut markdown = String::new();
    if options.front_matter {
        markdown.push_str(&front_matter(&ctx.metadata, format));
    }
    markdown.push_str(&markdown::render(&doc));

    Ok(Conversion {
        markdown,
        format,
        metadata: ctx.metadata,
        resources: ctx.resources,
        warnings: ctx.warnings,
        document: doc,
    })
}

/// Renders a document model to GFM (useful if you build or edit a [`Document`]).
pub fn render_markdown(doc: &Document) -> String {
    markdown::render(doc)
}

fn front_matter(meta: &Metadata, format: Format) -> String {
    let mut out = String::from("---\n");
    let mut field = |key: &str, value: &Option<String>| {
        if let Some(v) = value.as_deref().map(str::trim).filter(|v| !v.is_empty()) {
            let escaped = v.replace('\\', "\\\\").replace('"', "\\\"").replace(['\n', '\r'], " ");
            out.push_str(&format!("{key}: \"{escaped}\"\n"));
        }
    };
    field("title", &meta.title);
    field("author", &meta.author);
    field("subject", &meta.subject);
    field("description", &meta.description);
    field("keywords", &meta.keywords);
    field("language", &meta.language);
    field("created", &meta.created);
    field("modified", &meta.modified);
    if let Some(pages) = meta.pages {
        out.push_str(&format!("pages: {pages}\n"));
    }
    out.push_str(&format!("source_format: {format}\n---\n\n"));
    out
}

/// Shared state for one conversion: options, collected resources, metadata
/// and warnings.
pub(crate) struct Ctx<'o> {
    pub opts: &'o Options,
    pub filename: Option<String>,
    pub resources: Vec<Resource>,
    pub metadata: Metadata,
    pub warnings: Vec<String>,
    by_source: HashMap<String, usize>,
    used_ids: HashSet<String>,
}

impl<'o> Ctx<'o> {
    fn new(opts: &'o Options, filename: Option<&str>) -> Self {
        Ctx {
            opts,
            filename: filename.map(str::to_string),
            resources: Vec::new(),
            metadata: Metadata::default(),
            warnings: Vec::new(),
            by_source: HashMap::new(),
            used_ids: HashSet::new(),
        }
    }

    pub fn warn(&mut self, message: impl Into<String>) {
        self.warnings.push(message.into());
    }

    /// Registers a resource and returns the path to use in the Markdown.
    /// Resources with the same `source` key are stored once.
    pub fn add_resource(&mut self, source: &str, name_hint: &str, data: Vec<u8>, kind: ResourceKind) -> String {
        if let Some(&idx) = self.by_source.get(source) {
            return self.resources[idx].path.clone();
        }
        let mime = mime_for(&data, name_hint);
        let id = self.unique_id(name_hint, &mime, kind);
        let media_dir = self.opts.media_dir.trim_end_matches(['/', '\\']);
        let path = if media_dir.is_empty() { id.clone() } else { format!("{}/{id}", media_dir.replace('\\', "/")) };
        self.by_source.insert(source.to_string(), self.resources.len());
        self.resources.push(Resource {
            id,
            path: path.clone(),
            mime_type: mime,
            kind,
            source: Some(source.to_string()),
            referenced: false,
            data,
        });
        path
    }

    fn unique_id(&mut self, name_hint: &str, mime: &str, kind: ResourceKind) -> String {
        let base = name_hint.rsplit(['/', '\\']).next().unwrap_or(name_hint);
        let mut clean: String = base
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') { c } else { '_' })
            .collect();
        clean = clean.trim_matches(['.', '_']).to_string();
        if clean.is_empty() {
            clean = match kind {
                ResourceKind::Image => "image".into(),
                ResourceKind::Object => "object".into(),
            };
        }
        if !clean.contains('.')
            && let Some(ext) = extension_for_mime(mime)
        {
            clean = format!("{clean}.{ext}");
        }
        let (stem, ext) = match clean.rsplit_once('.') {
            Some((s, e)) => (s.to_string(), format!(".{e}")),
            None => (clean.clone(), String::new()),
        };
        let mut candidate = clean.clone();
        let mut n = 2;
        while !self.used_ids.insert(candidate.to_ascii_lowercase()) {
            candidate = format!("{stem}-{n}{ext}");
            n += 1;
        }
        candidate
    }

    /// Applies the image mode and marks which resources the Markdown links to.
    fn finish(&mut self, doc: &mut Document) {
        let by_path: HashMap<String, usize> =
            self.resources.iter().enumerate().map(|(i, r)| (r.path.clone(), i)).collect();
        let mode = self.opts.images;
        let resources = &mut self.resources;
        model::visit_inlines_mut(doc, &mut |inlines: &mut Vec<Inline>| {
            inlines.retain_mut(|inline| match inline {
                Inline::Image { src, .. } => {
                    let Some(&idx) = by_path.get(src.as_str()) else { return mode != ImageMode::Omit };
                    match mode {
                        ImageMode::Reference => {
                            resources[idx].referenced = true;
                            true
                        }
                        ImageMode::Embed => {
                            let res = &resources[idx];
                            *src = format!("data:{};base64,{}", res.mime_type, base64_encode(&res.data));
                            true
                        }
                        ImageMode::Omit => false,
                    }
                }
                Inline::Link { url, .. } => {
                    if let Some(&idx) = by_path.get(url.as_str()) {
                        resources[idx].referenced = true;
                    }
                    true
                }
                _ => true,
            });
        });
    }
}

fn mime_for(data: &[u8], name: &str) -> String {
    let sniffed = if data.starts_with(b"\x89PNG") {
        Some("image/png")
    } else if data.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg")
    } else if data.starts_with(b"GIF8") {
        Some("image/gif")
    } else if data.starts_with(b"BM") && data.len() > 14 {
        Some("image/bmp")
    } else if data.starts_with(b"RIFF") && data.get(8..12) == Some(b"WEBP") {
        Some("image/webp")
    } else if data.starts_with(b"II*\0") || data.starts_with(b"MM\0*") {
        Some("image/tiff")
    } else if data.starts_with(b"%PDF") {
        Some("application/pdf")
    } else if data.starts_with(&[0xD7, 0xCD, 0xC6, 0x9A]) || data.starts_with(&[0x01, 0x00, 0x09, 0x00]) {
        Some("image/wmf")
    } else if data.len() > 44 && data.starts_with(&[1, 0, 0, 0]) && &data[40..44] == b" EMF" {
        Some("image/emf")
    } else {
        None
    };
    if let Some(m) = sniffed {
        return m.to_string();
    }
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" | "jpe" => "image/jpeg",
        "gif" => "image/gif",
        "bmp" | "dib" => "image/bmp",
        "svg" => "image/svg+xml",
        "webp" => "image/webp",
        "tif" | "tiff" => "image/tiff",
        "emf" => "image/emf",
        "wmf" => "image/wmf",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "pdf" => "application/pdf",
        "xml" => "application/xml",
        _ => "application/octet-stream",
    }
    .to_string()
}

fn extension_for_mime(mime: &str) -> Option<&'static str> {
    Some(match mime {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "image/bmp" => "bmp",
        "image/webp" => "webp",
        "image/tiff" => "tiff",
        "image/svg+xml" => "svg",
        "image/emf" => "emf",
        "image/wmf" => "wmf",
        "application/pdf" => "pdf",
        "application/octet-stream" => "bin",
        _ => return None,
    })
}

pub(crate) fn base64_encode(data: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(TABLE[(n >> 18) as usize & 63] as char);
        out.push(TABLE[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { TABLE[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { TABLE[n as usize & 63] as char } else { '=' });
    }
    out
}

pub(crate) fn base64_decode(input: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(input.len() / 4 * 3);
    let (mut acc, mut bits) = (0u32, 0u32);
    for b in input.bytes() {
        let v = match b {
            b'A'..=b'Z' => b - b'A',
            b'a'..=b'z' => b - b'a' + 26,
            b'0'..=b'9' => b - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' => break,
            b' ' | b'\n' | b'\r' | b'\t' => continue,
            _ => return None,
        };
        acc = (acc << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_reference() {
        assert_eq!(base64_decode("Zm9vYmFy").unwrap(), b"foobar");
        assert_eq!(base64_decode("Zm8=").unwrap(), b"fo");
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn csv_from_bytes() {
        let out = convert_bytes(b"name,qty\nApple,3\n", None, &Options::default()).unwrap();
        assert_eq!(out.format, Format::Csv);
        assert_eq!(out.markdown, "| name  | qty |\n| ----- | --- |\n| Apple | 3   |\n");
    }
}
