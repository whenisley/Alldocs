//! Python bindings (PyO3). Conversion runs with the GIL released.

use std::borrow::Cow;
use std::path::PathBuf;

use pyo3::create_exception;
use pyo3::exceptions::{PyException, PyTypeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyDict};

create_exception!(_alldocs, AlldocsError, PyException, "Raised when a document can't be converted.");

/// An image or embedded object extracted from the document.
#[pyclass(frozen, get_all, name = "Resource", module = "alldocs")]
struct PyResource {
    /// Unique file name, e.g. ``image1.png``.
    id: String,
    /// Path used in the Markdown, e.g. ``media/image1.png``.
    path: String,
    mime_type: String,
    /// ``"image"`` or ``"object"``.
    kind: String,
    source: Option<String>,
    /// True when the Markdown links to ``path``.
    referenced: bool,
    data: Py<PyBytes>,
}

#[pymethods]
impl PyResource {
    fn __repr__(&self) -> String {
        format!("Resource(path={:?}, mime_type={:?})", self.path, self.mime_type)
    }
}

/// The result of a conversion. ``str(result)`` is the Markdown.
#[pyclass(frozen, name = "Conversion", module = "alldocs")]
struct PyConversion {
    inner: alldocs::Conversion,
}

#[pymethods]
impl PyConversion {
    #[getter]
    fn markdown(&self) -> &str {
        &self.inner.markdown
    }

    /// Detected (or forced) input format, e.g. ``"docx"``.
    #[getter]
    fn format(&self) -> &str {
        self.inner.format.name()
    }

    #[getter]
    fn warnings(&self) -> Vec<String> {
        self.inner.warnings.clone()
    }

    /// Document properties as a dict (title, author, ... pages).
    #[getter]
    fn metadata<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        let m = &self.inner.metadata;
        let d = PyDict::new(py);
        for (k, v) in [
            ("title", &m.title),
            ("author", &m.author),
            ("subject", &m.subject),
            ("description", &m.description),
            ("keywords", &m.keywords),
            ("language", &m.language),
            ("created", &m.created),
            ("modified", &m.modified),
        ] {
            d.set_item(k, v)?;
        }
        d.set_item("pages", m.pages)?;
        Ok(d)
    }

    /// Images and embedded objects, with their bytes.
    #[getter]
    fn resources(&self, py: Python<'_>) -> Vec<PyResource> {
        self.inner
            .resources
            .iter()
            .map(|r| PyResource {
                id: r.id.clone(),
                path: r.path.clone(),
                mime_type: r.mime_type.clone(),
                kind: match r.kind {
                    alldocs::ResourceKind::Image => "image".into(),
                    alldocs::ResourceKind::Object => "object".into(),
                },
                source: r.source.clone(),
                referenced: r.referenced,
                data: PyBytes::new(py, &r.data).unbind(),
            })
            .collect()
    }

    /// Writes the Markdown to ``path`` and its linked images next to it.
    /// Returns the list of written paths.
    fn save(&self, py: Python<'_>, path: PathBuf) -> PyResult<Vec<PathBuf>> {
        let inner = &self.inner;
        Ok(py.detach(|| inner.save(&path))?)
    }

    fn __str__(&self) -> &str {
        &self.inner.markdown
    }

    fn __repr__(&self) -> String {
        format!(
            "Conversion(format={:?}, markdown=<{} chars>, resources={})",
            self.inner.format.name(),
            self.inner.markdown.chars().count(),
            self.inner.resources.len()
        )
    }
}

#[allow(clippy::too_many_arguments)]
fn build_options(
    format: Option<&str>,
    images: &str,
    media_dir: &str,
    include_notes: bool,
    include_hidden: bool,
    front_matter: bool,
) -> PyResult<alldocs::Options> {
    Ok(alldocs::Options {
        format: format.map(str::parse).transpose().map_err(PyValueError::new_err)?,
        images: images.parse().map_err(PyValueError::new_err)?,
        media_dir: media_dir.to_string(),
        include_notes,
        include_hidden,
        front_matter,
    })
}

fn to_py_err(e: alldocs::Error) -> PyErr {
    match e {
        alldocs::Error::Io(io) => io.into(),
        other => AlldocsError::new_err(other.to_string()),
    }
}

enum Source {
    Path(PathBuf),
    Bytes(Vec<u8>),
}

/// Accepts bytes-like objects, paths (str / os.PathLike) and file-like objects.
fn read_source(source: &Bound<'_, PyAny>) -> PyResult<Source> {
    if let Ok(bytes) = source.extract::<Cow<'_, [u8]>>() {
        return Ok(Source::Bytes(bytes.into_owned()));
    }
    if let Ok(path) = source.extract::<PathBuf>() {
        return Ok(Source::Path(path));
    }
    if source.hasattr("read")? {
        let data = source.call_method0("read")?;
        return data
            .extract::<Cow<'_, [u8]>>()
            .map(|b| Source::Bytes(b.into_owned()))
            .map_err(|_| PyTypeError::new_err("file object must be opened in binary mode (read() must return bytes)"));
    }
    if source.hasattr("tobytes")? {
        let data = source.call_method0("tobytes")?;
        return Ok(Source::Bytes(data.extract::<Cow<'_, [u8]>>()?.into_owned()));
    }
    Err(PyTypeError::new_err("expected a path, bytes-like object or binary file object"))
}

fn run(py: Python<'_>, source: Source, filename: Option<String>, options: alldocs::Options) -> PyResult<PyConversion> {
    let result = py.detach(|| match source {
        Source::Path(p) => alldocs::convert_file(p, &options),
        Source::Bytes(b) => alldocs::convert_bytes(&b, filename.as_deref(), &options),
    });
    result.map(|inner| PyConversion { inner }).map_err(to_py_err)
}

/// Converts a document to GitHub Flavored Markdown.
///
/// ``source`` may be a path (``str`` or ``os.PathLike``), the document bytes
/// (``bytes``, ``bytearray``, ``memoryview``) or a binary file object such as
/// ``sys.stdin.buffer``. The format is detected from the content; pass
/// ``format="csv"`` etc. to force it, or ``filename`` as a hint for bytes.
#[pyfunction]
#[pyo3(signature = (source, *, format=None, images="ref", media_dir="media", include_notes=true, include_hidden=true, front_matter=false, filename=None))]
#[allow(clippy::too_many_arguments)]
fn convert(
    py: Python<'_>,
    source: &Bound<'_, PyAny>,
    format: Option<&str>,
    images: &str,
    media_dir: &str,
    include_notes: bool,
    include_hidden: bool,
    front_matter: bool,
    filename: Option<String>,
) -> PyResult<PyConversion> {
    let options = build_options(format, images, media_dir, include_notes, include_hidden, front_matter)?;
    // File objects returned by open() carry their name: use it as a hint.
    let filename = filename.or_else(|| source.getattr("name").ok().and_then(|n| n.extract::<String>().ok()));
    let source = read_source(source)?;
    run(py, source, filename, options)
}

/// Converts a file on disk. Same keyword options as :func:`convert`.
#[pyfunction]
#[pyo3(signature = (path, *, format=None, images="ref", media_dir="media", include_notes=true, include_hidden=true, front_matter=false))]
#[allow(clippy::too_many_arguments)]
fn convert_file(
    py: Python<'_>,
    path: PathBuf,
    format: Option<&str>,
    images: &str,
    media_dir: &str,
    include_notes: bool,
    include_hidden: bool,
    front_matter: bool,
) -> PyResult<PyConversion> {
    let options = build_options(format, images, media_dir, include_notes, include_hidden, front_matter)?;
    run(py, Source::Path(path), None, options)
}

/// Converts document bytes. ``filename`` is an optional format hint.
#[pyfunction]
#[pyo3(signature = (data, filename=None, *, format=None, images="ref", media_dir="media", include_notes=true, include_hidden=true, front_matter=false))]
#[allow(clippy::too_many_arguments)]
fn convert_bytes(
    py: Python<'_>,
    data: Cow<'_, [u8]>,
    filename: Option<String>,
    format: Option<&str>,
    images: &str,
    media_dir: &str,
    include_notes: bool,
    include_hidden: bool,
    front_matter: bool,
) -> PyResult<PyConversion> {
    let options = build_options(format, images, media_dir, include_notes, include_hidden, front_matter)?;
    run(py, Source::Bytes(data.into_owned()), filename, options)
}

/// Detects the format of document bytes; returns e.g. ``"pdf"`` or ``None``.
#[pyfunction]
#[pyo3(signature = (data, filename=None))]
fn detect_format(data: Cow<'_, [u8]>, filename: Option<&str>) -> Option<&'static str> {
    alldocs::detect_format(&data, filename).map(|f| f.name())
}

/// Lists supported formats as ``(name, description, extensions)`` tuples.
#[pyfunction]
fn supported_formats() -> Vec<(&'static str, &'static str, Vec<&'static str>)> {
    alldocs::Format::ALL.iter().map(|f| (f.name(), f.description(), f.extensions().to_vec())).collect()
}

#[pymodule]
fn _alldocs(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(convert, m)?)?;
    m.add_function(wrap_pyfunction!(convert_file, m)?)?;
    m.add_function(wrap_pyfunction!(convert_bytes, m)?)?;
    m.add_function(wrap_pyfunction!(detect_format, m)?)?;
    m.add_function(wrap_pyfunction!(supported_formats, m)?)?;
    m.add_class::<PyConversion>()?;
    m.add_class::<PyResource>()?;
    m.add("AlldocsError", m.py().get_type::<AlldocsError>())?;
    m.add("__version__", env!("CARGO_PKG_VERSION"))?;
    Ok(())
}
