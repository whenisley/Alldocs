"""Run with: python -m pytest bindings/python/tests  (or plain python this_file.py)"""
import io
import os
import pathlib
import tempfile

import alldocs

FIXTURES = pathlib.Path(__file__).resolve().parents[3] / "crates" / "alldocs" / "tests" / "fixtures"


def test_convert_path():
    result = alldocs.convert(FIXTURES / "report.docx")
    assert result.format == "docx"
    assert result.metadata["title"] == "Quarterly Report"
    assert result.markdown.startswith("# Quarterly Report\n")
    assert str(result) == result.markdown
    image = next(r for r in result.resources if r.path == "media/image1.png")
    assert image.data.startswith(b"\x89PNG") and image.mime_type == "image/png"


def test_convert_bytes_and_file_objects():
    data = (FIXTURES / "minutes.rtf").read_bytes()
    assert alldocs.detect_format(data) == "rtf"
    for source in (data, bytearray(data), memoryview(data), io.BytesIO(data)):
        assert "[the archive](https://example.com/minutes)" in alldocs.convert(source).markdown
    with open(FIXTURES / "data.tsv", "rb") as f:  # the file name is used as a format hint
        assert "| a   | b   |" in alldocs.convert(f).markdown


def test_options_and_errors():
    assert "data:image/png;base64," in alldocs.convert(FIXTURES / "report.docx", images="embed").markdown
    assert alldocs.convert_bytes(b"x;y\n1;2\n", "t.csv").markdown.startswith("| x ")
    try:
        alldocs.convert(b"plain words")
    except alldocs.AlldocsError as e:
        assert "could not detect" in str(e)
    else:
        raise AssertionError("expected AlldocsError")
    try:
        alldocs.convert(FIXTURES / "missing.docx")
    except FileNotFoundError:
        pass
    else:
        raise AssertionError("expected FileNotFoundError")


def test_save():
    with tempfile.TemporaryDirectory() as d:
        written = alldocs.convert(FIXTURES / "book.epub").save(os.path.join(d, "book.md"))
        assert len(written) == 2
        assert os.path.exists(os.path.join(d, "media", "fig1.png"))


if __name__ == "__main__":
    for name, fn in list(globals().items()):
        if name.startswith("test_"):
            fn()
            print("ok", name)
