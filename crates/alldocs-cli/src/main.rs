//! `alldocs` command-line interface.

use std::io::{self, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use alldocs::{Conversion, Format, ImageMode, Options};
use clap::{CommandFactory, Parser};

const EXAMPLES: &str = "\
Examples:
  Print a document as Markdown:
    alldocs report.docx

  Save to a file (images go to report_media/ next to it):
    alldocs report.docx -o report.md

  Convert many files at once into a folder (wildcards work in PowerShell too):
    alldocs *.docx *.pdf -d markdown/

  Read from stdin (the format is detected from the content):
    cat slides.pptx | alldocs -
    cmd /c \"alldocs - < book.epub\"                      (Windows PowerShell)

  Force a format when detection can't tell (e.g. a .txt that is really CSV):
    alldocs data.txt --from csv

  Self-contained Markdown with images inlined as data URIs:
    alldocs deck.pptx --images embed -o deck.md

  Add YAML front matter with title/author/dates:
    alldocs book.epub --front-matter -o book.md

  Machine-readable output (Markdown, metadata, resources as base64):
    alldocs sheet.xlsx --json

  Just tell me what format a file is:
    alldocs --detect mystery.bin
";

#[derive(Parser, Debug)]
#[command(
    name = "alldocs",
    version,
    about = "Convert Word, PowerPoint, Excel, OpenDocument, RTF, EPUB, CSV, HTML and PDF files to GitHub Flavored Markdown.",
    long_about = "Convert Word, PowerPoint, Excel, OpenDocument, RTF, EPUB, CSV, HTML and text-based PDF files \
to clean GitHub Flavored Markdown. Everything runs locally; nothing is uploaded.\n\n\
The input format is detected from the file content (the extension is only a fallback). \
Use `alldocs formats` to list supported formats and `alldocs examples` for more examples.",
    after_help = EXAMPLES,
    disable_help_subcommand = true
)]
struct Cli {
    /// Input files (wildcards allowed). Use `-` or pipe data in to read stdin.
    #[arg(value_name = "INPUT")]
    inputs: Vec<String>,

    /// Write the Markdown to this file instead of stdout (single input).
    #[arg(short, long, value_name = "FILE", conflicts_with = "out_dir")]
    output: Option<PathBuf>,

    /// Convert every input into this directory as <name>.md.
    #[arg(short = 'd', long, value_name = "DIR")]
    out_dir: Option<PathBuf>,

    /// Input format, skipping detection (docx, pptx, xlsx, xls, xlsb, odt, odp, ods, rtf, epub, csv, html, pdf).
    #[arg(short = 'f', long = "from", value_name = "FORMAT")]
    format: Option<Format>,

    /// How images appear in the Markdown: ref (linked files), embed (data URIs) or omit.
    #[arg(long, value_name = "MODE", default_value = "ref")]
    images: ImageMode,

    /// Folder for extracted images/objects, relative to the output file
    /// [default: <name>_media when writing files].
    #[arg(long, value_name = "DIR")]
    media_dir: Option<String>,

    /// File name to use as a format hint when reading stdin (e.g. data.tsv).
    #[arg(long, value_name = "NAME")]
    name: Option<String>,

    /// Leave out speaker notes from presentations.
    #[arg(long)]
    no_notes: bool,

    /// Leave out hidden slides and sheets.
    #[arg(long)]
    no_hidden: bool,

    /// Prepend YAML front matter with the document metadata.
    #[arg(long)]
    front_matter: bool,

    /// Output JSON (markdown, metadata, warnings, resources with base64 data).
    #[arg(long)]
    json: bool,

    /// Only print the detected format of each input.
    #[arg(long)]
    detect: bool,

    /// Don't print warnings or progress to stderr.
    #[arg(short, long)]
    quiet: bool,
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    // `alldocs help|examples|formats` (unless a file with that name exists).
    if args.len() == 2 && !Path::new(&args[1]).exists() {
        match args[1].as_str() {
            "help" => {
                let _ = Cli::command().print_long_help();
                return ExitCode::SUCCESS;
            }
            "examples" => {
                print!("{EXAMPLES}");
                return ExitCode::SUCCESS;
            }
            "formats" => {
                print_formats();
                return ExitCode::SUCCESS;
            }
            _ => {}
        }
    }
    let cli = Cli::parse();
    match run(cli) {
        Ok(code) => code,
        Err(message) => {
            eprintln!("alldocs: {message}");
            ExitCode::from(2)
        }
    }
}

fn print_formats() {
    println!("Supported input formats:\n");
    for f in Format::ALL {
        println!("  {:<6} {:<28} .{}", f.name(), f.description(), f.extensions().join(" ."));
    }
    println!("\nLegacy binary .doc and .ppt are not supported (save them as .docx/.pptx).");
}

fn run(cli: Cli) -> Result<ExitCode, String> {
    let read_stdin = cli.inputs.iter().any(|i| i == "-") || (cli.inputs.is_empty() && !io::stdin().is_terminal());
    if cli.inputs.is_empty() && !read_stdin {
        let _ = Cli::command().print_help();
        return Ok(ExitCode::from(2));
    }

    let mut options = Options {
        format: cli.format,
        images: cli.images,
        include_notes: !cli.no_notes,
        include_hidden: !cli.no_hidden,
        front_matter: cli.front_matter,
        ..Options::default()
    };

    if read_stdin {
        if cli.inputs.len() > 1 {
            return Err("`-` (stdin) can't be combined with other inputs".into());
        }
        let mut data = Vec::new();
        io::stdin().read_to_end(&mut data).map_err(|e| format!("reading stdin: {e}"))?;
        if cli.detect {
            println!("{}", detect_name(&data, cli.name.as_deref()));
            return Ok(ExitCode::SUCCESS);
        }
        let stem = cli.name.as_deref().map(file_stem).unwrap_or_else(|| "stdin".into());
        let target =
            cli.output.clone().or_else(|| cli.out_dir.as_ref().map(|d| d.join(format!("{stem}.{}", ext(&cli)))));
        options.media_dir = media_dir(&cli, &stem, target.is_some());
        let result = alldocs::convert_bytes(&data, cli.name.as_deref(), &options).map_err(|e| e.to_string())?;
        emit(&cli, &result, target.as_deref(), "stdin")?;
        return Ok(ExitCode::SUCCESS);
    }

    let inputs = expand_inputs(&cli.inputs)?;
    if cli.detect {
        for input in &inputs {
            match std::fs::read(input) {
                Ok(data) => println!("{}\t{}", detect_name(&data, input.to_str()), input.display()),
                Err(e) => println!("error\t{}: {e}", input.display()),
            }
        }
        return Ok(ExitCode::SUCCESS);
    }
    if inputs.len() > 1 && cli.out_dir.is_none() {
        return Err(format!(
            "{} inputs given; use --out-dir DIR to convert several files (or convert them one at a time)",
            inputs.len()
        ));
    }

    if let Some(dir) = &cli.out_dir {
        std::fs::create_dir_all(dir).map_err(|e| format!("creating {}: {e}", dir.display()))?;
        return Ok(convert_batch(&cli, &options, &inputs, dir));
    }

    let input = &inputs[0];
    let stem = file_stem(&input.to_string_lossy());
    options.media_dir = media_dir(&cli, &stem, cli.output.is_some());
    let started = Instant::now();
    let result = alldocs::convert_file(input, &options).map_err(|e| format!("{}: {e}", input.display()))?;
    emit(&cli, &result, cli.output.as_deref(), &input.display().to_string())?;
    if let (Some(out), false) = (&cli.output, cli.quiet) {
        eprintln!("{} -> {} ({} ms)", input.display(), out.display(), started.elapsed().as_millis());
    }
    Ok(ExitCode::SUCCESS)
}

fn convert_batch(cli: &Cli, options: &Options, inputs: &[PathBuf], dir: &Path) -> ExitCode {
    let next = AtomicUsize::new(0);
    let failures = AtomicUsize::new(0);
    let stderr = Mutex::new(());
    let workers = std::thread::available_parallelism().map_or(4, |n| n.get()).min(inputs.len()).max(1);
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(input) = inputs.get(i) else { break };
                    let stem = file_stem(&input.to_string_lossy());
                    let target = dir.join(format!("{stem}.{}", ext(cli)));
                    let mut opts = options.clone();
                    opts.media_dir = media_dir(cli, &stem, true);
                    let started = Instant::now();
                    let outcome = alldocs::convert_file(input, &opts)
                        .map_err(|e| e.to_string())
                        .and_then(|r| emit(cli, &r, Some(&target), &input.display().to_string()));
                    let _guard = stderr.lock();
                    match outcome {
                        Ok(()) if !cli.quiet => {
                            eprintln!(
                                "{} -> {} ({} ms)",
                                input.display(),
                                target.display(),
                                started.elapsed().as_millis()
                            )
                        }
                        Ok(()) => {}
                        Err(e) => {
                            failures.fetch_add(1, Ordering::Relaxed);
                            eprintln!("alldocs: {}: {e}", input.display());
                        }
                    }
                }
            });
        }
    });
    let failed = failures.load(Ordering::Relaxed);
    if !cli.quiet && inputs.len() > 1 {
        eprintln!("{} converted, {} failed", inputs.len() - failed, failed);
    }
    if failed > 0 { ExitCode::FAILURE } else { ExitCode::SUCCESS }
}

/// Writes one result to `target` (or stdout) and reports warnings.
fn emit(cli: &Cli, result: &Conversion, target: Option<&Path>, label: &str) -> Result<(), String> {
    if !cli.quiet {
        for w in &result.warnings {
            eprintln!("warning: {label}: {w}");
        }
    }
    let unsaved = result.resources.iter().filter(|r| r.referenced).count();
    match target {
        Some(path) => {
            if path.exists() && is_same_file_as_input(cli, path) {
                return Err(format!("refusing to overwrite the input file {}", path.display()));
            }
            if cli.json {
                // Resource bytes are embedded in the JSON as base64.
                write_file(path, &to_json(result))?;
            } else {
                result.save(path).map_err(|e| format!("writing {}: {e}", path.display()))?;
            }
        }
        None => {
            let mut out = io::stdout().lock();
            let text = if cli.json { to_json(result) } else { result.markdown.clone() };
            out.write_all(text.as_bytes()).map_err(|e| e.to_string())?;
            out.flush().ok();
            if cli.media_dir.is_some() && !cli.json {
                // stdout + explicit --media-dir: save resources relative to the current directory.
                for res in result.resources.iter().filter(|r| r.referenced) {
                    let path = Path::new(&res.path);
                    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
                        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                    }
                    std::fs::write(path, &res.data).map_err(|e| e.to_string())?;
                }
            } else if unsaved > 0 && !cli.quiet && !cli.json {
                eprintln!(
                    "note: {label}: {unsaved} image(s)/object(s) are linked but not saved; use -o FILE, --media-dir DIR or --images embed"
                );
            }
        }
    }
    Ok(())
}

fn is_same_file_as_input(cli: &Cli, path: &Path) -> bool {
    let canon = |p: &Path| std::fs::canonicalize(p).ok();
    let target = canon(path);
    target.is_some() && cli.inputs.iter().any(|i| canon(Path::new(i)) == target)
}

fn write_file(path: &Path, text: &str) -> Result<(), String> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).map_err(|e| format!("creating {}: {e}", parent.display()))?;
    }
    std::fs::write(path, text).map_err(|e| format!("writing {}: {e}", path.display()))
}

fn to_json(result: &Conversion) -> String {
    let resources: Vec<serde_json::Value> = result
        .resources
        .iter()
        .map(|r| {
            let mut v = serde_json::to_value(r).unwrap_or_default();
            if let Some(obj) = v.as_object_mut() {
                obj.insert("size".into(), r.data.len().into());
                obj.insert("data_base64".into(), base64(&r.data).into());
            }
            v
        })
        .collect();
    let value = serde_json::json!({
        "format": result.format,
        "markdown": result.markdown,
        "metadata": result.metadata,
        "warnings": result.warnings,
        "resources": resources,
    });
    let mut s = serde_json::to_string_pretty(&value).unwrap_or_default();
    s.push('\n');
    s
}

fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n =
            (u32::from(c[0]) << 16) | (u32::from(*c.get(1).unwrap_or(&0)) << 8) | u32::from(*c.get(2).unwrap_or(&0));
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if c.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}

fn detect_name(data: &[u8], name: Option<&str>) -> String {
    alldocs::detect_format(data, name).map_or_else(|| "unknown".to_string(), |f| f.name().to_string())
}

fn ext(cli: &Cli) -> &'static str {
    if cli.json { "json" } else { "md" }
}

fn media_dir(cli: &Cli, stem: &str, to_file: bool) -> String {
    match &cli.media_dir {
        Some(dir) => dir.clone(),
        None if to_file => format!("{stem}_media"),
        None => "media".to_string(),
    }
}

fn file_stem(name: &str) -> String {
    let stem = Path::new(name).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    if stem.is_empty() { "document".into() } else { stem }
}

/// Expands `*` and `?` wildcards (needed on Windows, where the shell doesn't).
fn expand_inputs(inputs: &[String]) -> Result<Vec<PathBuf>, String> {
    let mut out = Vec::new();
    for input in inputs {
        let path = Path::new(input);
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        if path.exists() || !(name.contains('*') || name.contains('?')) {
            if !path.exists() {
                return Err(format!("{input}: file not found"));
            }
            if path.is_dir() {
                return Err(format!("{input}: is a directory (use {input}/* to convert its files)"));
            }
            out.push(path.to_path_buf());
            continue;
        }
        let dir = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
        let mut matches: Vec<PathBuf> = std::fs::read_dir(dir)
            .map_err(|e| format!("{}: {e}", dir.display()))?
            .filter_map(|e| e.ok())
            .filter(|e| e.path().is_file())
            .filter(|e| wildcard_match(&name.to_lowercase(), &e.file_name().to_string_lossy().to_lowercase()))
            .map(|e| if dir == Path::new(".") { PathBuf::from(e.file_name()) } else { e.path() })
            .collect();
        if matches.is_empty() {
            return Err(format!("{input}: no matching files"));
        }
        matches.sort();
        out.extend(matches);
    }
    Ok(out)
}

fn wildcard_match(pattern: &str, name: &str) -> bool {
    let (p, n): (Vec<char>, Vec<char>) = (pattern.chars().collect(), name.chars().collect());
    let (mut pi, mut ni, mut star, mut mark) = (0, 0, None, 0);
    while ni < n.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == n[ni]) {
            pi += 1;
            ni += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ni;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ni = mark;
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|c| *c == '*')
}

#[cfg(test)]
mod tests {
    use super::wildcard_match;

    #[test]
    fn wildcards() {
        assert!(wildcard_match("*.docx", "report.docx"));
        assert!(wildcard_match("r?port*", "report.docx"));
        assert!(!wildcard_match("*.pdf", "report.docx"));
    }
}
