//! End-to-end conversions of real documents (see scripts/make_fixtures.py).

use std::path::PathBuf;

use alldocs::{Format, ImageMode, Options, convert_bytes, convert_file};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name)
}

fn md(name: &str) -> String {
    let out = convert_file(fixture(name), &Options::default()).unwrap_or_else(|e| panic!("{name}: {e}"));
    out.markdown
}

fn assert_contains(haystack: &str, needles: &[&str]) {
    for n in needles {
        assert!(haystack.contains(n), "missing {n:?} in:\n{haystack}");
    }
}

#[test]
fn docx() {
    let out = convert_file(fixture("report.docx"), &Options::default()).unwrap();
    assert_eq!(out.format, Format::Docx);
    assert_eq!(out.metadata.title.as_deref(), Some("Quarterly Report"));
    assert_contains(
        &out.markdown,
        &[
            "# Quarterly Report\n",
            "# Summary\n",
            "Revenue grew **12%** in *Q3*, see the [full report](https://example.com/report) for details.[^1]",
            "- New markets opened\n  - Two regions\n- Costs reduced",
            "1. Plan\n2. Execute\n3. Review",
            "| South \\| West | 900   | 1,010 |",
            "```\ndef total(xs):\n    return sum(xs)\n```",
            "> A wise quote about \\*growth\\* and_snake_case.",
            "![](media/image1.png)",
            "Strike ~~this~~",
            "[^1]: Unaudited figures.",
        ],
    );
    let image = out.resources.iter().find(|r| r.path == "media/image1.png").expect("image resource");
    assert_eq!(image.mime_type, "image/png");
    assert!(image.referenced && image.data.starts_with(b"\x89PNG"));
}

#[test]
fn pptx() {
    assert_contains(
        &md("roadmap.pptx"),
        &[
            "<!-- Slide 1 -->\n\n## Product Roadmap\n\nPlanning for 2027",
            "## Goals\n\n- Grow revenue\n  - Enterprise segment\n- Improve quality\n- [Docs site](https://docs.example.com)",
            "### Notes\n\nMention the Q3 numbers.",
            "| Cloud  | $10k |",
            "](media/image1.png)",
        ],
    );
}

#[test]
fn xlsx() {
    let out = convert_file(fixture("sales.xlsx"), &Options::default()).unwrap();
    assert_contains(
        &out.markdown,
        &[
            "## Sales\n",
            "| Widget        | 10    | 2.5   | 2026-03-01          | TRUE   |",
            "| Gadget \\| Pro | 3     | 0.3   | 2026-03-02 14:30:00 | FALSE  |",
            "| Multi<br>line | x   |",
            "## Secret (hidden)",
        ],
    );
    assert_eq!(out.metadata.pages, Some(3));
    let hidden = Options { include_hidden: false, ..Options::default() };
    assert!(!convert_file(fixture("sales.xlsx"), &hidden).unwrap().markdown.contains("Secret"));
}

#[test]
fn opendocument() {
    assert_contains(
        &md("notes.odt"),
        &[
            "# Field Notes\n",
            "Observed **three** species near the [river](https://example.org/river).[^1]",
            "1. Arrive\n2. Observe\n3. Record",
            "- Birds\n  - Herons",
            "| Heron   | 4     |",
            "```\nprint('hello')\n```",
            "[^1]: Counted at dawn.",
        ],
    );
    assert_contains(&md("kickoff.odp"), &["## Kickoff\n\n- Scope\n- Timeline"]);
    assert_contains(&md("inventory.ods"), &["| Apples | 5   |"]);
}

#[test]
fn rtf() {
    let out = convert_file(fixture("minutes.rtf"), &Options::default()).unwrap();
    assert_eq!(out.metadata.title.as_deref(), Some("Meeting Minutes"));
    assert_contains(
        &out.markdown,
        &[
            "# Meeting Minutes\n",
            "Attendees discussed the **budget** and *timeline*. São Paulo office — confirmed.",
            "## Actions\n\n1. Draft plan\n2. Send invites",
            "See [the archive](https://example.com/minutes).[^1]",
            "| Rui   | Friday |",
            "```\ngit push origin main\n```",
            "[^1]: Archive is internal.",
        ],
    );
}

#[test]
fn epub() {
    let out = convert_file(fixture("book.epub"), &Options::default()).unwrap();
    assert_eq!(out.metadata.title.as_deref(), Some("The Small Book"));
    assert_contains(
        &out.markdown,
        &[
            "# Chapter One\n\nIt was a *quiet* morning—the kind with “fog”.[^1]",
            "![A figure](media/fig1.png)",
            "```python\nprint(\"hi\")\n```",
            "See chapter one and [the site](https://example.com).",
            "3. three\n4. four",
            "[^1]: Fog is common here.",
        ],
    );
    assert!(!out.markdown.contains("toc"), "navigation document must be skipped");
}

#[test]
fn csv_and_tsv() {
    assert_eq!(
        md("people.csv"),
        "| Nome       | Cidade            | Nota |\n| ---------- | ----------------- | ---- |\n| Silva, Ana | São Paulo         | 9,5  |\n| João       | Rio<br>de Janeiro | 8    |\n"
    );
    assert_eq!(md("data.tsv"), "| a   | b   |\n| --- | --- |\n| 1   | 2   |\n");
}

#[test]
fn pdf() {
    let out = convert_file(fixture("paper.pdf"), &Options::default()).unwrap();
    assert_eq!(out.metadata.pages, Some(3));
    assert_contains(
        &out.markdown,
        &[
            "# A Study of Things\n\n## 1. Introduction\n",
            "behave in a predictable way under normal conditions.",
            "- First point\n- Second point\n- Third point",
            "```\nfor x in things:\n    measure(x)\n```",
            "## 3. Results\n\nResults were good.",
        ],
    );
    // Running header and page numbers are removed.
    assert!(!out.markdown.contains("ACME Research"));
    assert!(!out.markdown.lines().any(|l| l.trim() == "2"));
}

#[test]
fn bytes_and_image_modes() {
    let data = std::fs::read(fixture("report.docx")).unwrap();
    let embed = Options { images: ImageMode::Embed, ..Options::default() };
    let out = convert_bytes(&data, None, &embed).unwrap();
    assert!(out.markdown.contains("![](data:image/png;base64,iVBOR"));
    let omit = Options { images: ImageMode::Omit, ..Options::default() };
    let out = convert_bytes(&data, None, &omit).unwrap();
    assert!(!out.markdown.contains("!["));
    assert_eq!(out.resources.len(), 1, "omitted images are still returned");
}

#[test]
fn save_writes_markdown_and_media() {
    let dir = std::env::temp_dir().join(format!("alldocs-save-{}", std::process::id()));
    let out = convert_file(fixture("book.epub"), &Options::default()).unwrap();
    let written = out.save(dir.join("book.md")).unwrap();
    assert_eq!(written.len(), 2);
    assert!(dir.join("media").join("fig1.png").exists());
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn every_fixture_detects_from_content_alone() {
    for (name, format) in [
        ("report.docx", Format::Docx),
        ("roadmap.pptx", Format::Pptx),
        ("sales.xlsx", Format::Xlsx),
        ("notes.odt", Format::Odt),
        ("kickoff.odp", Format::Odp),
        ("inventory.ods", Format::Ods),
        ("minutes.rtf", Format::Rtf),
        ("book.epub", Format::Epub),
        ("people.csv", Format::Csv),
        ("page.html", Format::Html),
        ("paper.pdf", Format::Pdf),
    ] {
        let data = std::fs::read(fixture(name)).unwrap();
        assert_eq!(alldocs::detect_format(&data, None), Some(format), "{name}");
    }
}
