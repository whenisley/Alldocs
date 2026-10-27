"""Minimal command line: ``python -m alldocs INPUT [-o OUTPUT]``.

The standalone ``alldocs`` binary has more options (batch, JSON, detect).
"""

import argparse
import sys

from . import AlldocsError, convert


def main(argv=None):
    parser = argparse.ArgumentParser(prog="python -m alldocs", description="Convert a document to GitHub Flavored Markdown.")
    parser.add_argument("input", help="input file, or - for stdin")
    parser.add_argument("-o", "--output", help="write Markdown here (images go next to it)")
    parser.add_argument("-f", "--from", dest="format", help="force the input format, e.g. docx or pdf")
    parser.add_argument("--images", choices=["ref", "embed", "omit"], default="ref")
    parser.add_argument("--front-matter", action="store_true", help="prepend YAML front matter")
    args = parser.parse_args(argv)

    source = sys.stdin.buffer if args.input == "-" else args.input
    media_dir = "media"
    if args.output:
        import os

        media_dir = os.path.splitext(os.path.basename(args.output))[0] + "_media"
    try:
        result = convert(source, format=args.format, images=args.images, media_dir=media_dir, front_matter=args.front_matter)
    except (AlldocsError, OSError) as e:
        print(f"alldocs: {e}", file=sys.stderr)
        return 1
    for warning in result.warnings:
        print(f"warning: {warning}", file=sys.stderr)
    if args.output:
        result.save(args.output)
    else:
        sys.stdout.reconfigure(encoding="utf-8")
        sys.stdout.write(result.markdown)
    return 0


if __name__ == "__main__":
    sys.exit(main())
