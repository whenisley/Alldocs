//! Node.js bindings (napi-rs). The JavaScript wrapper in `index.js` adds
//! path/Buffer/stream dispatch and `save()` on top of these functions.

use napi::bindgen_prelude::*;
use napi_derive::napi;

/// Conversion options (all optional).
#[napi(object)]
#[derive(Default)]
pub struct ConvertOptions {
    /// Force the input format, e.g. "docx" or "pdf".
    pub format: Option<String>,
    /// "ref" (default), "embed" or "omit".
    pub images: Option<String>,
    /// Folder prefix for image links (default "media").
    pub media_dir: Option<String>,
    pub include_notes: Option<bool>,
    pub include_hidden: Option<bool>,
    pub front_matter: Option<bool>,
    /// File name used as a format hint for buffers.
    pub filename: Option<String>,
}

#[napi(object)]
pub struct Resource {
    pub id: String,
    pub path: String,
    pub mime_type: String,
    /// "image" or "object".
    pub kind: String,
    pub source: Option<String>,
    pub referenced: bool,
    pub data: Buffer,
}

#[napi(object)]
pub struct Metadata {
    pub title: Option<String>,
    pub author: Option<String>,
    pub subject: Option<String>,
    pub description: Option<String>,
    pub keywords: Option<String>,
    pub language: Option<String>,
    pub created: Option<String>,
    pub modified: Option<String>,
    pub pages: Option<u32>,
}

#[napi(object)]
pub struct ConversionResult {
    pub markdown: String,
    pub format: String,
    pub metadata: Metadata,
    pub resources: Vec<Resource>,
    pub warnings: Vec<String>,
}

#[napi(object)]
pub struct FormatInfo {
    pub name: String,
    pub description: String,
    pub extensions: Vec<String>,
}

fn to_options(opts: Option<ConvertOptions>) -> Result<(alldocs::Options, Option<String>)> {
    let opts = opts.unwrap_or_default();
    let mut out = alldocs::Options::default();
    if let Some(f) = opts.format {
        out.format = Some(f.parse().map_err(|e: String| Error::new(Status::InvalidArg, e))?);
    }
    if let Some(i) = opts.images {
        out.images = i.parse().map_err(|e: String| Error::new(Status::InvalidArg, e))?;
    }
    if let Some(m) = opts.media_dir {
        out.media_dir = m;
    }
    if let Some(v) = opts.include_notes {
        out.include_notes = v;
    }
    if let Some(v) = opts.include_hidden {
        out.include_hidden = v;
    }
    if let Some(v) = opts.front_matter {
        out.front_matter = v;
    }
    Ok((out, opts.filename))
}

fn to_js(c: alldocs::Conversion) -> ConversionResult {
    let m = c.metadata;
    ConversionResult {
        markdown: c.markdown,
        format: c.format.name().to_string(),
        metadata: Metadata {
            title: m.title,
            author: m.author,
            subject: m.subject,
            description: m.description,
            keywords: m.keywords,
            language: m.language,
            created: m.created,
            modified: m.modified,
            pages: m.pages.map(|p| p as u32),
        },
        resources: c
            .resources
            .into_iter()
            .map(|r| Resource {
                id: r.id,
                path: r.path,
                mime_type: r.mime_type,
                kind: match r.kind {
                    alldocs::ResourceKind::Image => "image".into(),
                    alldocs::ResourceKind::Object => "object".into(),
                },
                source: r.source,
                referenced: r.referenced,
                data: r.data.into(),
            })
            .collect(),
        warnings: c.warnings,
    }
}

fn js_err(e: alldocs::Error) -> Error {
    Error::new(Status::GenericFailure, e.to_string())
}

/// Converts a file on disk.
#[napi]
pub fn convert_file(path: String, options: Option<ConvertOptions>) -> Result<ConversionResult> {
    let (opts, _) = to_options(options)?;
    alldocs::convert_file(&path, &opts).map(to_js).map_err(js_err)
}

/// Converts an in-memory document.
#[napi]
pub fn convert_buffer(data: Buffer, options: Option<ConvertOptions>) -> Result<ConversionResult> {
    let (opts, filename) = to_options(options)?;
    alldocs::convert_bytes(&data, filename.as_deref(), &opts).map(to_js).map_err(js_err)
}

pub enum Input {
    Path(String),
    Bytes(Vec<u8>),
}

pub struct ConvertTask {
    input: Input,
    options: alldocs::Options,
    filename: Option<String>,
}

impl Task for ConvertTask {
    type Output = alldocs::Conversion;
    type JsValue = ConversionResult;

    fn compute(&mut self) -> Result<Self::Output> {
        match &self.input {
            Input::Path(p) => alldocs::convert_file(p, &self.options),
            Input::Bytes(b) => alldocs::convert_bytes(b, self.filename.as_deref(), &self.options),
        }
        .map_err(js_err)
    }

    fn resolve(&mut self, _env: Env, output: Self::Output) -> Result<Self::JsValue> {
        Ok(to_js(output))
    }
}

/// Converts a file on a worker thread; returns a Promise.
#[napi]
pub fn convert_file_async(path: String, options: Option<ConvertOptions>) -> Result<AsyncTask<ConvertTask>> {
    let (options, filename) = to_options(options)?;
    Ok(AsyncTask::new(ConvertTask { input: Input::Path(path), options, filename }))
}

/// Converts an in-memory document on a worker thread; returns a Promise.
#[napi]
pub fn convert_buffer_async(data: Buffer, options: Option<ConvertOptions>) -> Result<AsyncTask<ConvertTask>> {
    let (options, filename) = to_options(options)?;
    Ok(AsyncTask::new(ConvertTask { input: Input::Bytes(data.to_vec()), options, filename }))
}

/// Detects the format of a buffer ("docx", "pdf", ...), or null.
#[napi]
pub fn detect_format(data: Buffer, filename: Option<String>) -> Option<String> {
    alldocs::detect_format(&data, filename.as_deref()).map(|f| f.name().to_string())
}

/// Lists the supported input formats.
#[napi]
pub fn supported_formats() -> Vec<FormatInfo> {
    alldocs::Format::ALL
        .iter()
        .map(|f| FormatInfo {
            name: f.name().into(),
            description: f.description().into(),
            extensions: f.extensions().iter().map(|e| e.to_string()).collect(),
        })
        .collect()
}
