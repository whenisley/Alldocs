'use strict'

const assert = require('node:assert/strict')
const fs = require('node:fs')
const os = require('node:os')
const path = require('node:path')
const { Readable } = require('node:stream')
const { test } = require('node:test')

const alldocs = require('..')

const fixtures = path.join(__dirname, '..', '..', '..', 'crates', 'alldocs', 'tests', 'fixtures')
const fixture = (name) => path.join(fixtures, name)

test('converts a file path synchronously', () => {
  const result = alldocs.convert(fixture('report.docx'))
  assert.equal(result.format, 'docx')
  assert.equal(result.metadata.title, 'Quarterly Report')
  assert.match(result.markdown, /^# Quarterly Report\n/)
  assert.match(result.markdown, /\[\^1\]: Unaudited figures\./)
  const image = result.resources.find((r) => r.path === 'media/image1.png')
  assert.ok(Buffer.isBuffer(image.data))
  assert.equal(image.mimeType, 'image/png')
})

test('converts bytes and detects the format from content', () => {
  const data = fs.readFileSync(fixture('minutes.rtf'))
  assert.equal(alldocs.detectFormat(data), 'rtf')
  const result = alldocs.convert(new Uint8Array(data))
  assert.match(result.markdown, /See \[the archive\]\(https:\/\/example\.com\/minutes\)/)
})

test('converts asynchronously, including from a stream', async () => {
  const fromPath = await alldocs.convertAsync(fixture('paper.pdf'))
  assert.match(fromPath.markdown, /- First point\n- Second point/)
  const stream = Readable.from([fs.readFileSync(fixture('book.epub'))])
  const fromStream = await alldocs.convertAsync(stream)
  assert.equal(fromStream.format, 'epub')
  assert.match(fromStream.markdown, /\[\^1\]: Fog is common here\./)
})

test('options: forced format, filename hint, embedded images', () => {
  const tsv = Buffer.from('a\tb\n1\t2\n')
  assert.match(alldocs.convert(tsv, { filename: 'x.tsv' }).markdown, /\| a {3}\| b {3}\|/)
  const embedded = alldocs.convert(fixture('report.docx'), { images: 'embed' })
  assert.match(embedded.markdown, /!\[\]\(data:image\/png;base64,/)
  assert.throws(() => alldocs.convert(Buffer.from('plain words')), /could not detect/)
  assert.throws(() => alldocs.convert(tsv, { format: 'nope' }), /unknown format/)
})

test('save() writes markdown and linked media', () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'alldocs-'))
  const written = alldocs.save(alldocs.convert(fixture('book.epub')), path.join(dir, 'book.md'))
  assert.equal(written.length, 2)
  assert.ok(fs.existsSync(path.join(dir, 'media', 'fig1.png')))
  fs.rmSync(dir, { recursive: true, force: true })
})

test('lists supported formats', () => {
  const names = alldocs.supportedFormats().map((f) => f.name)
  for (const n of ['docx', 'pptx', 'xlsx', 'odt', 'rtf', 'epub', 'csv', 'pdf']) assert.ok(names.includes(n))
})
