// Builds the native addon with cargo and copies it to ./alldocs.node.
// Usage: node build.js [--debug]
'use strict'

const { execFileSync } = require('node:child_process')
const fs = require('node:fs')
const path = require('node:path')

const release = !process.argv.includes('--debug')
const root = path.resolve(__dirname, '..', '..')
const targetDir = process.env.CARGO_TARGET_DIR || path.join(root, 'target')

// With the Windows GNU Rust toolchain, napi-build links against a MinGW
// import library for node.exe ("libnode"). Official Node builds don't ship
// one, so generate it from node.exe's N-API exports (needs MinGW-w64
// `gendef` and `dlltool` on PATH). MSVC builds don't need this.
function prepareGnuLibnode() {
  if (process.platform !== 'win32' || process.env.LIBNODE_PATH) return
  const host = execFileSync('rustc', ['-vV']).toString()
  if (!/host: .*windows-gnu/.test(host)) return
  const dir = path.join(targetDir, 'libnode')
  const implib = path.join(dir, 'libnode.dll.a')
  if (!fs.existsSync(implib)) {
    fs.mkdirSync(dir, { recursive: true })
    const exports = execFileSync('gendef', ['-', process.execPath], { maxBuffer: 64 << 20 }).toString()
    const symbols = exports
      .split(/\r?\n/)
      .filter((l) => /^(napi_|node_api_)/.test(l))
      .map((l) => l.split(/\s/)[0])
    const def = path.join(dir, 'node.def')
    fs.writeFileSync(def, `LIBRARY node.exe\nEXPORTS\n${symbols.join('\n')}\n`)
    execFileSync('dlltool', ['-d', def, '-l', implib, '-D', 'node.exe'], { stdio: 'inherit' })
    // napi-build only checks that libnode.dll exists; the linker prefers libnode.dll.a.
    fs.writeFileSync(path.join(dir, 'libnode.dll'), '')
  }
  process.env.LIBNODE_PATH = dir
}

prepareGnuLibnode()

const args = ['build', '-p', 'alldocs-node', '--manifest-path', path.join(root, 'Cargo.toml')]
if (release) args.push('--release')
execFileSync('cargo', args, { stdio: 'inherit' })

const profile = release ? 'release' : 'debug'
const lib = {
  win32: 'alldocs_node.dll',
  darwin: 'liballdocs_node.dylib',
}[process.platform] || 'liballdocs_node.so'

const from = path.join(targetDir, profile, lib)
const to = path.join(__dirname, 'alldocs.node')
fs.copyFileSync(from, to)
console.log(`built ${to}`)
