# alldocs

Fast, local conversion of documents to clean **GitHub Flavored Markdown**.

| Input | Formats |
| --- | --- |
| Word | `.docx` `.docm` `.dotx` |
| PowerPoint | `.pptx` `.pptm` `.ppsx` `.potx` |
| Excel | `.xlsx` `.xlsm` `.xlsb` `.xls` |
| OpenDocument | `.odt` `.odp` `.ods` |
| Rich Text | `.rtf` |
| E-books | `.epub` (2 and 3) |
| Tables | `.csv` `.tsv` (delimiter and encoding detected) |
| Web | `.html` `.xhtml` |
| PDF | text-based `.pdf` |

- **One engine, four front ends.** A Rust core with a command-line tool and
  native bindings for **Rust**, **Node.js** and **Python**.
- **Files, bytes or stdin.** The format is detected from the content (magic
  bytes, ZIP layout, text sniffing); the file name is only a fallback hint.
- **Consistent output.** Every format is converted into one document model and
  rendered by one Markdown writer, so headings, lists (nested, numbered, task
  lists), tables, links, footnotes, code blocks, quotes and images look the
  same whatever the source was.
- **Images and embedded objects are kept.** They're returned as resources (bytes
  + MIME type) and linked from the Markdown, inlined as `data:` URIs, or left
  out — your choice.
- **Local and fast.** Pure Rust, no external services, no LibreOffice, no Java,
  no Python runtime needed for the CLI.

---

## Quick start (command line)

```sh
alldocs report.docx                    # print Markdown to the terminal
alldocs report.docx -o report.md       # save it (images go to report_media/)
alldocs *.docx *.pdf -d markdown/      # convert many files in parallel
cat slides.pptx | alldocs -            # read stdin; format detected from content
alldocs help                           # full help
alldocs examples                       # more examples
alldocs formats                        # supported formats
```

<details>
<summary><code>alldocs --help</code></summary>

```text
Usage: alldocs [OPTIONS] [INPUT]...

Arguments:
  [INPUT]...  Input files (wildcards allowed). Use `-` or pipe data in to read stdin.

Options:
  -o, --output <FILE>       Write the Markdown to this file instead of stdout (single input)
  -d, --out-dir <DIR>       Convert every input into this directory as <name>.md
  -f, --from <FORMAT>       Input format, skipping detection (docx, pptx, xlsx, xls, xlsb, odt, odp, ods, rtf, epub, csv, html, pdf)
      --images <MODE>       How images appear in the Markdown: ref (linked files), embed (data URIs) or omit [default: ref]
      --media-dir <DIR>     Folder for extracted images/objects, relative to the output file [default: <name>_media when writing files]
      --name <NAME>         File name to use as a format hint when reading stdin (e.g. data.tsv)
      --no-notes            Leave out speaker notes from presentations
      --no-hidden           Leave out hidden slides and sheets
      --front-matter        Prepend YAML front matter with the document metadata
      --json                Output JSON (markdown, metadata, warnings, resources with base64 data)
      --detect              Only print the detected format of each input
  -q, --quiet               Don't print warnings or progress to stderr
  -h, --help                Print help (see more with '--help')
  -V, --version             Print version
```

</details>

More examples:

```sh
alldocs deck.pptx --images embed -o deck.md     # self-contained Markdown (data URIs)
alldocs book.epub --front-matter -o book.md     # YAML front matter: title, author, dates
alldocs data.txt --from csv                     # force a format
alldocs sheet.xlsx --json                       # Markdown + metadata + resources (base64)
alldocs --detect mystery.bin                    # just identify the format
cmd /c "alldocs - < book.epub"                  # stdin from Windows PowerShell 5.1
```

## Use it from Rust

```toml
[dependencies]
alldocs = { path = "crates/alldocs" }   # PDF support is the default `pdf` feature
```

```rust
use alldocs::{convert_bytes, convert_file, convert_reader, ImageMode, Options};

// A file on disk
let result = convert_file("report.docx", &Options::default())?;
println!("{}", result.markdown);
result.save("out/report.md")?;              // Markdown + out/media/*.png

// Bytes in memory (the name is only a hint for ambiguous content)
let result = convert_bytes(&bytes, Some("data.tsv"), &Options::default())?;

// Any reader, e.g. stdin
let result = convert_reader(std::io::stdin(), None, &Options {
    images: ImageMode::Embed,
    ..Options::default()
})?;

for res in &result.resources {
    println!("{} {} {} bytes", res.path, res.mime_type, res.data.len());
}
```

`Conversion` also carries `format`, `metadata` (title, author, dates, page
count…), `warnings`, and `document` — the intermediate model, if you want to
post-process before rendering with `alldocs::render_markdown`.

Run the example: `cargo run --example to_markdown -- report.docx out/report.md`.

## Use it from Node.js

```sh
cd bindings/node && npm run build      # builds ./alldocs.node (needs Rust)
```

```js
const alldocs = require('./bindings/node')

const result = alldocs.convert('report.docx')                 // path
const fromBytes = alldocs.convert(fs.readFileSync('a.pdf'))   // Buffer / Uint8Array
const fromStdin = await alldocs.convertAsync(process.stdin)   // streams (worker thread)

console.log(result.markdown, result.metadata.title)
alldocs.save(result, 'out/report.md')                         // Markdown + linked images

alldocs.convert(buf, { filename: 'data.tsv', images: 'embed', frontMatter: true })
alldocs.detectFormat(buf)                                     // 'docx' | ... | null
```

TypeScript types are in `bindings/node/index.d.ts`. Example script:
`node examples/node/convert.js report.docx out/report.md`.

## Use it from Python

```sh
pip install maturin
cd bindings/python && maturin develop --release   # or: pip install ./bindings/python
```

```python
import sys, alldocs

result = alldocs.convert("report.docx")            # path or os.PathLike
result = alldocs.convert(open("a.pdf", "rb"))      # binary file object
result = alldocs.convert(sys.stdin.buffer)         # stdin
result = alldocs.convert(data, filename="x.tsv")   # bytes / bytearray / memoryview

print(result.markdown, result.metadata["title"])
result.save("out/report.md")                       # Markdown + linked images
for res in result.resources:
    print(res.path, res.mime_type, len(res.data))
```

Conversion releases the GIL, so threads convert in parallel. A minimal CLI is
included: `python -m alldocs report.docx -o report.md`. One wheel (abi3) works
on CPython 3.9+.

## Options

| CLI | Rust `Options` | Node | Python | Default |
| --- | --- | --- | --- | --- |
| `--from FMT` | `format` | `format` | `format=` | detect |
| `--images ref\|embed\|omit` | `images` | `images` | `images=` | `ref` |
| `--media-dir DIR` | `media_dir` | `mediaDir` | `media_dir=` | `media` (`<name>_media` in the CLI when saving) |
| `--no-notes` | `include_notes` | `includeNotes` | `include_notes=` | notes included |
| `--no-hidden` | `include_hidden` | `includeHidden` | `include_hidden=` | hidden included |
| `--front-matter` | `front_matter` | `frontMatter` | `front_matter=` | off |
| `--name` | `filename` arg | `filename` | `filename=` | — |

## How documents map to Markdown

| Source | Markdown |
| --- | --- |
| Heading styles / outline levels, ODF `text:h`, HTML `h1`–`h6`, RTF `\outlinelevel`, larger PDF fonts | `#` … `######` |
| Bulleted / numbered paragraphs (Word numbering, ODF lists, PowerPoint bullets, RTF lists, PDF `•`/`1.` markers) | nested `-` / `1.` lists; numbering continues across interruptions |
| Bold, italic, strikethrough | `**`, `*`, `~~` |
| Monospace runs / code styles | `` `code` ``; whole monospace paragraphs → fenced code blocks |
| Quote styles, `<blockquote>` | `>` |
| Tables (merged cells padded), spreadsheet sheets | GFM tables (first row is the header) |
| Hyperlinks (incl. Word/RTF `HYPERLINK` fields) | `[text](url)` |
| Footnotes and endnotes (Word, ODF, RTF, EPUB `noteref`) | `[^1]` + definitions at the end |
| Images | `![alt](media/image1.png)` (or data URIs) |
| Embedded objects (OLE, attached files) | `[Embedded …](media/oleObject1.bin)` + resource |
| Slides | `<!-- Slide N -->`, `## Title`, content in reading order, `### Notes` |
| Sheets | `## Sheet name` per sheet, blocks separated by empty rows become separate tables |

Text is escaped so it never turns into accidental Markdown (`*`, `_`, `#`, `1.`,
`|` in tables…), while intra-word underscores like `snake_case` are left alone.

## PDFs

PDFs are converted locally from their text layer (via the pure-Rust
[pdf_oxide](https://crates.io/crates/pdf_oxide)). Lines are rebuilt into
paragraphs; larger fonts become headings; bullets and numbered markers become
lists; monospace text becomes code; running headers/footers and page numbers
are removed; hyphenated words split across lines are rejoined; embedded images
are extracted.

Scanned/image-only PDFs have no text layer. OCR is not included; such pages are
reported in `warnings`. Table detection in PDFs is not attempted (table text
comes out as paragraphs).

## Limitations

- Legacy binary Word/PowerPoint (`.doc`, `.ppt`) aren't supported — save as
  `.docx`/`.pptx`. Legacy Excel `.xls` *is* supported.
- Spreadsheet formulas show their last calculated value (files saved by tools
  that don't store calculated values show empty cells).
- Charts and SmartArt are not converted to text; their images are kept when the
  file contains a rendered fallback.

## Performance

Release build on a modest laptop CPU (Intel Core i3-8130U, 2 cores), one file at
a time, end to end including process start and writing the `.md`:

| Document | Size | Time |
| --- | --- | --- |
| Word, ~300 pages (1,800 paragraphs, 60 tables, lists) | 43 KB | 85 ms |
| PowerPoint, 200 slides | 206 KB | 31 ms |
| PDF, 100 pages of text | 77 KB | 300 ms |
| Excel, 50,000 rows × 5 columns (3.3 MB of Markdown) | 1.4 MB | 670 ms |
| Small documents (the test fixtures) | < 40 KB | 13–25 ms |

Batch mode (`-d`) converts files in parallel across all cores.

## Building

Requirements: Rust 1.88+ (`rustup`). Node and Python are only needed for their
bindings.

```sh
cargo build --release            # CLI at target/release/alldocs(.exe)
cargo test --workspace --exclude alldocs-python
```

Without the `pdf` feature the core has no PDF dependency:
`alldocs = { path = "crates/alldocs", default-features = false }`.

<details>
<summary>Windows with the GNU toolchain (no Visual Studio)</summary>

The project builds with `stable-x86_64-pc-windows-gnu`. You also need a
MinGW-w64 install for `dlltool`/`as` (for example `winget install
BrechtSanders.WinLibs.POSIX.UCRT`), and its `bin` folder on `PATH` *after*
Rust's. `.cargo/config.toml` makes Rust use its own bundled linker, which avoids
problems when MinGW lives in a path with spaces. The Node build script generates
the `node.exe` import library the GNU target needs automatically. With the MSVC
toolchain none of this is necessary.

Tip: if the project lives in a synced folder (Google Drive, OneDrive), set
`CARGO_TARGET_DIR` to a local folder to keep build files out of sync.

</details>

## Project layout

```text
crates/alldocs/          core library: detection, converters, Markdown renderer
  src/formats/           docx, pptx, sheet (xlsx/xls/xlsb/ods), odf (odt/odp),
                         rtf, epub, html, csv, pdf
  tests/fixtures/        sample documents used by the tests
crates/alldocs-cli/      the `alldocs` command
bindings/node/           napi-rs addon + JS wrapper + TypeScript types
bindings/python/         PyO3 extension (maturin) + typing stubs
examples/                Node and Python example scripts
scripts/make_fixtures.py regenerates the test documents
```

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in this project, as defined in the Apache-2.0 license, shall be
dual licensed as above, without any additional terms or conditions.
