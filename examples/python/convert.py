"""python examples/python/convert.py report.docx [out/report.md]
cat book.epub | python examples/python/convert.py -
"""
import sys

import alldocs


def main() -> int:
    if len(sys.argv) < 2:
        print("usage: convert.py <input|-> [output.md]", file=sys.stderr)
        return 2
    source = sys.stdin.buffer if sys.argv[1] == "-" else sys.argv[1]

    # Paths, bytes and binary file objects are all accepted.
    result = alldocs.convert(source)

    print(f"format: {result.format}, title: {result.metadata['title']}", file=sys.stderr)
    for warning in result.warnings:
        print(f"warning: {warning}", file=sys.stderr)
    for res in result.resources:
        print(f"resource: {res.path} ({res.mime_type}, {len(res.data)} bytes)", file=sys.stderr)

    if len(sys.argv) > 2:
        for path in result.save(sys.argv[2]):
            print(f"wrote {path}", file=sys.stderr)
    else:
        sys.stdout.reconfigure(encoding="utf-8")
        sys.stdout.write(result.markdown)
    return 0


if __name__ == "__main__":
    sys.exit(main())
