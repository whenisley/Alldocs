import os
import pathlib
from typing import BinaryIO, Literal, Optional, Union

Format = Literal["docx", "pptx", "xlsx", "xlsb", "xls", "odt", "odp", "ods", "rtf", "epub", "csv", "html", "pdf"]
ImageMode = Literal["ref", "embed", "omit"]
Source = Union[str, os.PathLike[str], bytes, bytearray, memoryview, BinaryIO]

__version__: str

class AlldocsError(Exception):
    """Raised when a document can't be converted."""

class Resource:
    """An image or embedded object extracted from the document."""
    id: str
    path: str
    mime_type: str
    kind: Literal["image", "object"]
    source: Optional[str]
    referenced: bool
    data: bytes

class Conversion:
    """The result of a conversion. ``str(result)`` is the Markdown."""
    @property
    def markdown(self) -> str: ...
    @property
    def format(self) -> Format: ...
    @property
    def metadata(self) -> dict[str, Optional[Union[str, int]]]: ...
    @property
    def resources(self) -> list[Resource]: ...
    @property
    def warnings(self) -> list[str]: ...
    def save(self, path: Union[str, os.PathLike[str]]) -> list[pathlib.Path]:
        """Writes the Markdown and its linked images; returns the written paths."""

def convert(
    source: Source,
    *,
    format: Optional[Format] = None,
    images: ImageMode = "ref",
    media_dir: str = "media",
    include_notes: bool = True,
    include_hidden: bool = True,
    front_matter: bool = False,
    filename: Optional[str] = None,
) -> Conversion: ...
def convert_file(
    path: Union[str, os.PathLike[str]],
    *,
    format: Optional[Format] = None,
    images: ImageMode = "ref",
    media_dir: str = "media",
    include_notes: bool = True,
    include_hidden: bool = True,
    front_matter: bool = False,
) -> Conversion: ...
def convert_bytes(
    data: Union[bytes, bytearray],
    filename: Optional[str] = None,
    *,
    format: Optional[Format] = None,
    images: ImageMode = "ref",
    media_dir: str = "media",
    include_notes: bool = True,
    include_hidden: bool = True,
    front_matter: bool = False,
) -> Conversion: ...
def detect_format(data: Union[bytes, bytearray], filename: Optional[str] = None) -> Optional[Format]: ...
def supported_formats() -> list[tuple[str, str, list[str]]]: ...
