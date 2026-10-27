//! Converts a file (or stdin) to Markdown and saves it with its images.
//!
//!     cargo run --example to_markdown -- report.docx out/report.md
//!     cat slides.pptx | cargo run --example to_markdown -- - out/slides.md

use std::io::Read;

use alldocs::{Options, convert_bytes, convert_file};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let input = args.next().ok_or("usage: to_markdown <input|-> [output.md]")?;
    let output = args.next();

    let options = Options::default();
    let result = if input == "-" {
        let mut data = Vec::new();
        std::io::stdin().read_to_end(&mut data)?;
        convert_bytes(&data, None, &options)?
    } else {
        convert_file(&input, &options)?
    };

    eprintln!("format: {}, title: {:?}", result.format, result.metadata.title);
    for warning in &result.warnings {
        eprintln!("warning: {warning}");
    }
    for res in &result.resources {
        eprintln!("resource: {} ({}, {} bytes)", res.path, res.mime_type, res.data.len());
    }

    match output {
        Some(path) => {
            for written in result.save(&path)? {
                eprintln!("wrote {}", written.display());
            }
        }
        None => print!("{}", result.markdown),
    }
    Ok(())
}
