// node examples/node/convert.js report.docx [out/report.md]
// cat book.epub | node examples/node/convert.js -
const alldocs = require('../../bindings/node')

async function main() {
  const [input, output] = process.argv.slice(2)
  if (!input) {
    console.error('usage: node convert.js <input|-> [output.md]')
    process.exit(2)
  }

  // Paths, Buffers and streams (here: stdin) are all accepted.
  const result = await alldocs.convertAsync(input === '-' ? process.stdin : input)

  console.error(`format: ${result.format}, title: ${result.metadata.title ?? '-'}`)
  for (const w of result.warnings) console.error(`warning: ${w}`)
  for (const r of result.resources) console.error(`resource: ${r.path} (${r.mimeType}, ${r.data.length} bytes)`)

  if (output) {
    for (const file of alldocs.save(result, output)) console.error(`wrote ${file}`)
  } else {
    process.stdout.write(result.markdown)
  }
}

main().catch((err) => {
  console.error(err.message)
  process.exit(1)
})
