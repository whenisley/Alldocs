'use strict'

const fs = require('node:fs')
const path = require('node:path')
const { fileURLToPath } = require('node:url')

const native = require('./alldocs.node')

function toPath(input) {
  return input instanceof URL ? fileURLToPath(input) : input
}

function isStream(x) {
  return x != null && typeof x === 'object' && typeof x.pipe === 'function' && typeof x.on === 'function'
}

function toBuffer(x) {
  if (Buffer.isBuffer(x)) return x
  if (x instanceof ArrayBuffer) return Buffer.from(x)
  if (ArrayBuffer.isView(x)) return Buffer.from(x.buffer, x.byteOffset, x.byteLength)
  return null
}

/**
 * Converts a document synchronously.
 * `input` is a file path, a Buffer/Uint8Array/ArrayBuffer of the document bytes.
 */
function convert(input, options) {
  if (typeof input === 'string' || input instanceof URL) {
    return native.convertFile(toPath(input), options)
  }
  const buf = toBuffer(input)
  if (buf) return native.convertBuffer(buf, options)
  throw new TypeError('convert(): expected a path, Buffer, Uint8Array or ArrayBuffer')
}

/**
 * Converts a document on a worker thread. Also accepts a readable stream
 * (for example `process.stdin`).
 */
async function convertAsync(input, options) {
  if (typeof input === 'string' || input instanceof URL) {
    return native.convertFileAsync(toPath(input), options)
  }
  if (isStream(input)) {
    const chunks = []
    for await (const chunk of input) chunks.push(typeof chunk === 'string' ? Buffer.from(chunk) : chunk)
    return native.convertBufferAsync(Buffer.concat(chunks), options)
  }
  const buf = toBuffer(input)
  if (buf) return native.convertBufferAsync(buf, options)
  throw new TypeError('convertAsync(): expected a path, Buffer, Uint8Array, ArrayBuffer or readable stream')
}

/**
 * Writes `result.markdown` to `outPath`, plus every resource the Markdown
 * links to (at `<dir of outPath>/<resource.path>`). Returns the written paths.
 */
function save(result, outPath) {
  const dir = path.dirname(outPath)
  fs.mkdirSync(dir, { recursive: true })
  fs.writeFileSync(outPath, result.markdown)
  const written = [outPath]
  const root = path.resolve(dir)
  for (const res of result.resources) {
    if (!res.referenced) continue
    const target = path.resolve(dir, res.path)
    if (!target.startsWith(root + path.sep)) continue // never write outside `dir`
    fs.mkdirSync(path.dirname(target), { recursive: true })
    fs.writeFileSync(target, res.data)
    written.push(target)
  }
  return written
}

module.exports = {
  convert,
  convertAsync,
  convertFile: native.convertFile,
  convertBuffer: native.convertBuffer,
  convertFileAsync: native.convertFileAsync,
  convertBufferAsync: native.convertBufferAsync,
  detectFormat: native.detectFormat,
  supportedFormats: native.supportedFormats,
  save,
}
