"""Fast local conversion of documents to GitHub Flavored Markdown.

>>> import alldocs
>>> result = alldocs.convert("report.docx")
>>> print(result.markdown)
>>> result.save("out/report.md")   # Markdown plus extracted images
"""

from ._alldocs import (
    AlldocsError,
    Conversion,
    Resource,
    __version__,
    convert,
    convert_bytes,
    convert_file,
    detect_format,
    supported_formats,
)

__all__ = [
    "AlldocsError",
    "Conversion",
    "Resource",
    "__version__",
    "convert",
    "convert_bytes",
    "convert_file",
    "detect_format",
    "supported_formats",
]
