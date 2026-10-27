/// <reference types="node" />

export type Format =
  | 'docx' | 'pptx' | 'xlsx' | 'xlsb' | 'xls' | 'odt' | 'odp' | 'ods'
  | 'rtf' | 'epub' | 'csv' | 'html' | 'pdf'

export interface ConvertOptions {
  /** Force the input format instead of detecting it. */
  format?: Format
  /** How images appear in the Markdown: linked files (default), data URIs, or left out. */
  images?: 'ref' | 'embed' | 'omit'
  /** Folder prefix for image links, relative to the Markdown file. Default `"media"`. */
  mediaDir?: string
  /** Include presentation speaker notes. Default `true`. */
  includeNotes?: boolean
  /** Include hidden slides and sheets. Default `true`. */
  includeHidden?: boolean
  /** Prepend YAML front matter with the document metadata. Default `false`. */
  frontMatter?: boolean
  /** File name used as a format hint for buffers and streams (e.g. `"data.tsv"`). */
  filename?: string
}

export interface Resource {
  /** Unique file name, e.g. `image1.png`. */
  id: string
  /** Path used in the Markdown, e.g. `media/image1.png`. */
  path: string
  mimeType: string
  kind: 'image' | 'object'
  /** Where the data came from inside the source document. */
  source?: string
  /** True when the Markdown links to `path`. */
  referenced: boolean
  data: Buffer
}

export interface Metadata {
  title?: string
  author?: string
  subject?: string
  description?: string
  keywords?: string
  language?: string
  created?: string
  modified?: string
  /** Pages, slides or sheets. */
  pages?: number
}

export interface ConversionResult {
  markdown: string
  format: Format
  metadata: Metadata
  resources: Resource[]
  warnings: string[]
}

export interface FormatInfo {
  name: Format
  description: string
  extensions: string[]
}

/** Converts a file path or document bytes synchronously. */
export function convert(input: string | URL | Buffer | Uint8Array | ArrayBuffer, options?: ConvertOptions): ConversionResult
/** Converts on a worker thread; also accepts readable streams such as `process.stdin`. */
export function convertAsync(
  input: string | URL | Buffer | Uint8Array | ArrayBuffer | NodeJS.ReadableStream,
  options?: ConvertOptions,
): Promise<ConversionResult>
export function convertFile(path: string, options?: ConvertOptions): ConversionResult
export function convertBuffer(data: Buffer, options?: ConvertOptions): ConversionResult
export function convertFileAsync(path: string, options?: ConvertOptions): Promise<ConversionResult>
export function convertBufferAsync(data: Buffer, options?: ConvertOptions): Promise<ConversionResult>
/** Detects the format from content (with an optional file name hint); `null` if unknown. */
export function detectFormat(data: Buffer, filename?: string): Format | null
export function supportedFormats(): FormatInfo[]
/** Writes the Markdown and its linked resources; returns the written paths. */
export function save(result: ConversionResult, outPath: string): string[]
