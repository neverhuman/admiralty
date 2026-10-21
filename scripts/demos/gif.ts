// GIF reading (Pillow frame semantics), palette quantization and Pillow-shaped writing.
import type { RgbImage } from './png.ts';

export interface GifFrame {
  left: number; top: number; width: number; height: number;
  interlaced: boolean;
  /** Local color table (empty when it is an identity grayscale ramp), or undefined to use the global table. */
  palette: Uint8Array | undefined;
  transparency: number | undefined;
  /** Graphic Control Extension delay in milliseconds, undefined without a GCE. */
  duration: number | undefined;
  /** Pillow's effective disposal method (unspecified values inherit the previous one). */
  disposal: number;
  minCodeSize: number;
  data: Uint8Array;
}

export interface GifFile {
  screenWidth: number;
  screenHeight: number;
  globalPalette: Uint8Array | undefined;
  background: number;
  loop: number | undefined;
  frames: GifFrame[];
}

export function isGif(bytes: Uint8Array): boolean {
  const header = String.fromCharCode(...bytes.subarray(0, 6));
  return header === 'GIF87a' || header === 'GIF89a';
}

class Reader {
  offset = 0;
  readonly bytes: Uint8Array;
  constructor(bytes: Uint8Array) { this.bytes = bytes; }
  byte(): number | undefined { return this.offset < this.bytes.length ? this.bytes[this.offset++] : undefined; }
  take(length: number): Uint8Array {
    if (this.offset + length > this.bytes.length) throw new Error('truncated GIF file');
    const view = this.bytes.subarray(this.offset, this.offset + length);
    this.offset += length;
    return view;
  }
  u16(): number { const view = this.take(2); return (view[0] ?? 0) | ((view[1] ?? 0) << 8); }
  /** One data sub-block, or undefined at a block terminator / end of file (Pillow `data()`). */
  block(): Uint8Array | undefined {
    const length = this.byte();
    return length ? this.take(length) : undefined;
  }
  skipBlocks(): void { while (this.block()) { /* skip */ } }
}

const grayscale = new Uint8Array(0);

/** Pillow treats an identity grayscale table as "no palette" (mode L). */
function neededPalette(table: Uint8Array): Uint8Array | undefined {
  for (let index = 0; index + 2 < table.length; index += 3) {
    const entry = index / 3;
    if (table[index] !== entry || table[index + 1] !== entry || table[index + 2] !== entry) return table;
  }
  return undefined;
}

/** Parses every frame the way Pillow's sequential `seek()` reads them. */
export function parseGif(bytes: Uint8Array): GifFile {
  if (!isGif(bytes)) throw new Error('not a GIF file');
  const reader = new Reader(bytes);
  reader.take(6);
  const screenWidth = reader.u16(), screenHeight = reader.u16();
  const flags = reader.byte() ?? 0;
  const backgroundIndex = reader.byte() ?? 0;
  reader.byte();
  let globalPalette: Uint8Array | undefined;
  let background = 0;
  if (flags & 0x80) {
    background = backgroundIndex;
    globalPalette = neededPalette(reader.take(3 << ((flags & 7) + 1)));
  }
  const frames: GifFrame[] = [];
  let loop: number | undefined;
  let disposal = 0;
  for (;;) {
    let introducer = reader.byte();
    if (introducer === undefined || introducer === 0x3b) break;
    let duration: number | undefined;
    let transparency: number | undefined;
    let found = false;
    while (introducer !== undefined && introducer !== 0x3b) {
      if (introducer === 0x21) {
        const label = reader.byte();
        let block = reader.block();
        if (label === 0xf9 && block) {
          if ((block[0] ?? 0) & 1) transparency = block[3];
          duration = ((block[1] ?? 0) | ((block[2] ?? 0) << 8)) * 10;
          const bits = ((block[0] ?? 0) & 0b00011100) >> 2;
          if (bits) disposal = bits;
        } else if (label === 0xff && frames.length === 0 && block) {
          if (String.fromCharCode(...block.subarray(0, 11)) === 'NETSCAPE2.0') {
            block = reader.block();
            if (block && block.length >= 3 && block[0] === 1) loop = (block[1] ?? 0) | ((block[2] ?? 0) << 8);
          }
        }
        if (block) reader.skipBlocks();
      } else if (introducer === 0x2c) {
        const left = reader.u16(), top = reader.u16(), width = reader.u16(), height = reader.u16();
        const imageFlags = reader.byte() ?? 0;
        const palette = imageFlags & 0x80 ? neededPalette(reader.take(3 << ((imageFlags & 7) + 1))) ?? grayscale : undefined;
        const minCodeSize = reader.byte();
        if (minCodeSize === undefined) throw new Error('truncated GIF file');
        const start = reader.offset;
        reader.skipBlocks();
        frames.push({ left, top, width, height, interlaced: (imageFlags & 0x40) !== 0, palette, transparency, duration,
          disposal, minCodeSize, data: bytes.subarray(start, reader.offset) });
        found = true;
        break;
      }
      // Unknown bytes between blocks are ignored, as Pillow does.
      introducer = reader.byte();
    }
    if (!found) break;
  }
  if (!frames.length) throw new Error('image not found in GIF file');
  return { screenWidth, screenHeight, globalPalette, background, loop, frames };
}

/** Pillow's image size after loading frame `index` (frames may enlarge the canvas). */
export function gifSizeAt(gif: GifFile, index: number): [number, number] {
  let width = gif.screenWidth, height = gif.screenHeight;
  for (const frame of gif.frames.slice(0, index + 1)) {
    const right = frame.left + frame.width, bottom = frame.top + frame.height;
    if (right > width || bottom > height) { width = Math.max(width, right); height = Math.max(height, bottom); }
  }
  return [width, height];
}

/** Decodes one frame's LZW stream into palette indices (row-major, deinterlaced). */
export function decodeLzw(frame: GifFrame): Uint8Array {
  const pixelCount = frame.width * frame.height;
  const output = new Uint8Array(pixelCount);
  const minimum = frame.minCodeSize;
  if (minimum < 1 || minimum > 11) throw new Error('invalid GIF LZW code size');
  // Concatenate sub-blocks.
  const parts: Uint8Array[] = [];
  let total = 0;
  for (let offset = 0; offset < frame.data.length;) {
    const length = frame.data[offset] ?? 0;
    if (!length) break;
    parts.push(frame.data.subarray(offset + 1, offset + 1 + length));
    total += length;
    offset += length + 1;
  }
  const stream = new Uint8Array(total);
  let cursor = 0;
  for (const part of parts) { stream.set(part, cursor); cursor += part.length; }
  const clear = 1 << minimum, end = clear + 1;
  const prefix = new Int32Array(4096), suffix = new Uint8Array(4096), lengths = new Int32Array(4096);
  for (let code = 0; code < clear; code += 1) { prefix[code] = -1; suffix[code] = code; lengths[code] = 1; }
  let codeSize = minimum + 1, next = clear + 2, previous = -1;
  let bitBuffer = 0, bitCount = 0, position = 0, written = 0;
  const scratch = new Uint8Array(4096);
  while (written < pixelCount) {
    while (bitCount < codeSize) {
      if (position >= stream.length) throw new Error('image file is truncated');
      bitBuffer |= (stream[position++] ?? 0) << bitCount;
      bitCount += 8;
    }
    const code = bitBuffer & ((1 << codeSize) - 1);
    bitBuffer >>>= codeSize;
    bitCount -= codeSize;
    if (code === clear) { codeSize = minimum + 1; next = clear + 2; previous = -1; continue; }
    if (code === end) break;
    let first: number;
    let emit: number;
    if (previous === -1) {
      if (code >= clear) throw new Error('broken GIF LZW stream');
      emit = code;
    } else if (code < next) {
      emit = code;
    } else if (code === next) {
      emit = previous;
    } else {
      throw new Error('broken GIF LZW stream');
    }
    // Expand `emit` into scratch (reverse order).
    let length = lengths[emit] ?? 0;
    for (let walk = emit, at = length - 1; walk !== -1; walk = prefix[walk] ?? -1, at -= 1) scratch[at] = suffix[walk] ?? 0;
    first = scratch[0] ?? 0;
    if (previous !== -1 && code === next) { scratch[length] = first; length += 1; }
    const count = Math.min(length, pixelCount - written);
    output.set(scratch.subarray(0, count), written);
    written += count;
    if (previous !== -1 && next < 4096) {
      prefix[next] = previous; suffix[next] = first; lengths[next] = (lengths[previous] ?? 0) + 1;
      next += 1;
      if (next === 1 << codeSize && codeSize < 12) codeSize += 1;
    }
    previous = code;
  }
  if (written < pixelCount) throw new Error('image file is truncated');
  if (!frame.interlaced) return output;
  const rows = new Uint8Array(pixelCount);
  let source = 0;
  for (const [start, step] of [[0, 8], [4, 8], [2, 4], [1, 2]] as const) {
    for (let y = start; y < frame.height; y += step) {
      rows.set(output.subarray(source, source + frame.width), y * frame.width);
      source += frame.width;
    }
  }
  return rows;
}

function frameTable(gif: GifFile, frame: GifFrame): Uint8Array | undefined {
  const table = frame.palette ?? gif.globalPalette;
  return table && table.length ? table : undefined;
}

function paletteLookup(table: Uint8Array | undefined): Uint8Array {
  // Pillow pads palettes from a grayscale ramp; a missing table is grayscale ("L").
  const lookup = new Uint8Array(768);
  for (let index = 0; index < 256; index += 1) lookup.fill(index, index * 3, index * 3 + 3);
  if (table) lookup.set(table.subarray(0, Math.min(768, table.length)));
  return lookup;
}

function rgbOf(table: Uint8Array | undefined, color: number): [number, number, number] {
  // Pillow `_rgb`: out-of-range indices fall back to entry 0.
  if (!table) return [color, color, color];
  const index = color * 3 + 3 > table.length ? 0 : color;
  return [table[index * 3] ?? 0, table[index * 3 + 1] ?? 0, table[index * 3 + 2] ?? 0];
}

function fillRect(canvas: RgbImage, left: number, top: number, right: number, bottom: number, color: [number, number, number]): void {
  for (let y = top; y < Math.min(bottom, canvas.height); y += 1) {
    for (let x = left; x < Math.min(right, canvas.width); x += 1) {
      const at = (y * canvas.width + x) * 3;
      canvas.rgb[at] = color[0]; canvas.rgb[at + 1] = color[1]; canvas.rgb[at + 2] = color[2];
    }
  }
}

function enlarge(canvas: RgbImage, width: number, height: number, fill: [number, number, number]): RgbImage {
  if (width === canvas.width && height === canvas.height) return canvas;
  const larger: RgbImage = { width, height, rgb: new Uint8Array(width * height * 3) };
  fillRect(larger, 0, 0, width, height, fill);
  for (let y = 0; y < canvas.height; y += 1) larger.rgb.set(canvas.rgb.subarray(y * canvas.width * 3, (y + 1) * canvas.width * 3), y * width * 3);
  return larger;
}

/**
 * Yields each composited frame converted to RGB, following Pillow's GIF
 * loader: frame 0 is a palette image (transparent indices keep their palette
 * color), later frames paste non-transparent pixels over the previous canvas
 * after applying the previous frame's disposal (2 = transparency/background
 * color, 3 = previous contents).
 */
export function* decodeGifFrames(gif: GifFile): Generator<RgbImage> {
  let canvas: RgbImage | undefined;
  let pendingDispose: { left: number; top: number; right: number; bottom: number; color?: [number, number, number]; previous?: Uint8Array } | undefined;
  for (const [index, frame] of gif.frames.entries()) {
    const table = frameTable(gif, frame);
    const lookup = paletteLookup(table);
    const lookupColor = (color: number): [number, number, number] => [lookup[color * 3] ?? 0, lookup[color * 3 + 1] ?? 0, lookup[color * 3 + 2] ?? 0];
    const [width, height] = gifSizeAt(gif, index);
    const indices = decodeLzw(frame);
    const right = frame.left + frame.width, bottom = frame.top + frame.height;
    if (!canvas) {
      const fill = frame.transparency ?? 0;
      canvas = { width, height, rgb: new Uint8Array(width * height * 3) };
      fillRect(canvas, 0, 0, width, height, lookupColor(fill));
      for (let y = 0; y < frame.height; y += 1) {
        for (let x = 0; x < frame.width; x += 1) {
          const value = indices[y * frame.width + x] ?? 0;
          const at = ((frame.top + y) * width + frame.left + x) * 3;
          canvas.rgb[at] = lookup[value * 3] ?? 0; canvas.rgb[at + 1] = lookup[value * 3 + 1] ?? 0; canvas.rgb[at + 2] = lookup[value * 3 + 2] ?? 0;
        }
      }
    } else {
      if (pendingDispose) {
        const { left, top, right: disposeRight, bottom: disposeBottom, color, previous } = pendingDispose;
        if (color) fillRect(canvas, left, top, disposeRight, disposeBottom, color);
        else if (previous) {
          const rowBytes = (Math.min(disposeRight, canvas.width) - left) * 3;
          for (let y = top, row = 0; y < Math.min(disposeBottom, canvas.height); y += 1, row += 1) {
            canvas.rgb.set(previous.subarray(row * rowBytes, (row + 1) * rowBytes), (y * canvas.width + left) * 3);
          }
        }
      }
      canvas = enlarge(canvas, width, height, frame.transparency === undefined ? [lookup[0] ?? 0, lookup[1] ?? 0, lookup[2] ?? 0] : [0, 0, 0]);
    }
    // The disposal for this frame applies before the next frame is drawn.
    // Frame 0 is still a palette image in Pillow, so its fill color is a
    // palette lookup; later frames use Pillow's `_rgb` helper.
    pendingDispose = undefined;
    const disposeColor = (color: number) => index === 0 ? lookupColor(color) : rgbOf(table, color);
    if (frame.disposal === 2) {
      pendingDispose = { left: frame.left, top: frame.top, right, bottom,
        color: disposeColor(frame.transparency ?? gif.background) };
    } else if (frame.disposal >= 3 && index > 0) {
      const rowBytes = (Math.min(right, canvas.width) - frame.left) * 3;
      const rows = Math.max(0, Math.min(bottom, canvas.height) - frame.top);
      const previous = new Uint8Array(Math.max(0, rowBytes) * rows);
      for (let row = 0; row < rows && rowBytes > 0; row += 1) {
        const at = ((frame.top + row) * canvas.width + frame.left) * 3;
        previous.set(canvas.rgb.subarray(at, at + rowBytes), row * rowBytes);
      }
      pendingDispose = { left: frame.left, top: frame.top, right, bottom, previous };
    } else if (frame.disposal >= 3 && frame.transparency !== undefined) {
      pendingDispose = { left: frame.left, top: frame.top, right, bottom, color: disposeColor(frame.transparency) };
    }
    if (index > 0) {
      for (let y = 0; y < frame.height && frame.top + y < canvas.height; y += 1) {
        for (let x = 0; x < frame.width && frame.left + x < canvas.width; x += 1) {
          const value = indices[y * frame.width + x] ?? 0;
          if (value === frame.transparency) continue;
          const at = ((frame.top + y) * canvas.width + frame.left + x) * 3;
          canvas.rgb[at] = lookup[value * 3] ?? 0; canvas.rgb[at + 1] = lookup[value * 3 + 1] ?? 0; canvas.rgb[at + 2] = lookup[value * 3 + 2] ?? 0;
        }
      }
    }
    yield { width: canvas.width, height: canvas.height, rgb: canvas.rgb.slice() };
  }
}

/**
 * Maps RGB pixels to the nearest palette entry exactly like Pillow's
 * `quantize(palette=..., dither=NONE)`: its color cache resolves each
 * 6-bit-per-channel cell (r>>2, g>>2, b>>2) by the squared distance from the
 * cell origin, with the lowest palette index winning ties.
 */
export function quantize(image: RgbImage, palette: Uint8Array): Uint8Array {
  const size = Math.min(256, Math.floor(palette.length / 3));
  const cache = new Int16Array(64 * 64 * 64).fill(-1);
  const out = new Uint8Array(image.width * image.height);
  const rgb = image.rgb;
  for (let pixel = 0, at = 0; pixel < out.length; pixel += 1, at += 3) {
    const red = rgb[at] ?? 0, green = rgb[at + 1] ?? 0, blue = rgb[at + 2] ?? 0;
    const key = (red >> 2) + ((green >> 2) << 6) + ((blue >> 2) << 12);
    let index = cache[key] ?? -1;
    if (index < 0) {
      const r0 = red & 0xfc, g0 = green & 0xfc, b0 = blue & 0xfc;
      let best = Infinity;
      index = 0;
      for (let entry = 0; entry < size; entry += 1) {
        const dr = r0 - (palette[entry * 3] ?? 0), dg = g0 - (palette[entry * 3 + 1] ?? 0), db = b0 - (palette[entry * 3 + 2] ?? 0);
        const distance = dr * dr + dg * dg + db * db;
        if (distance < best) { best = distance; index = entry; }
      }
      cache[key] = index;
    }
    out[pixel] = index;
  }
  return out;
}

class BitWriter {
  private readonly chunks: Uint8Array[] = [];
  private buffer = new Uint8Array(65536);
  private length = 0;
  private bits = 0;
  private count = 0;
  write(code: number, size: number): void {
    this.bits |= code << this.count;
    this.count += size;
    while (this.count >= 8) { this.push(this.bits & 0xff); this.bits >>>= 8; this.count -= 8; }
  }
  private push(value: number): void {
    if (this.length === this.buffer.length) { this.chunks.push(this.buffer); this.buffer = new Uint8Array(65536); this.length = 0; }
    this.buffer[this.length++] = value;
  }
  finish(): Uint8Array {
    if (this.count > 0) { this.push(this.bits & 0xff); this.bits = 0; this.count = 0; }
    this.chunks.push(this.buffer.subarray(0, this.length));
    const total = this.chunks.reduce((sum, chunk) => sum + chunk.length, 0);
    const out = new Uint8Array(total);
    let offset = 0;
    for (const chunk of this.chunks) { out.set(chunk, offset); offset += chunk.length; }
    return out;
  }
}

/** GIF LZW-compresses 8-bit indices into data sub-blocks (minimum code size 8). */
export function encodeLzw(indices: Uint8Array): Uint8Array {
  const minimum = 8, clear = 1 << minimum, end = clear + 1;
  const writer = new BitWriter();
  let codeSize = minimum + 1, next = clear + 2;
  const table = new Map<number, number>();
  writer.write(clear, codeSize);
  let current = -1;
  for (const value of indices) {
    if (current === -1) { current = value; continue; }
    const key = current * 256 + value;
    const found = table.get(key);
    if (found !== undefined) { current = found; continue; }
    writer.write(current, codeSize);
    if (next < 4096) {
      table.set(key, next);
      next += 1;
      if (next > 1 << codeSize && codeSize < 12) codeSize += 1;
    } else {
      writer.write(clear, codeSize);
      table.clear();
      codeSize = minimum + 1;
      next = clear + 2;
    }
    current = value;
  }
  if (current !== -1) writer.write(current, codeSize);
  writer.write(end, codeSize);
  const data = writer.finish();
  const blocks = new Uint8Array(data.length + Math.ceil(data.length / 255) + 1);
  let offset = 0;
  for (let start = 0; start < data.length; start += 255) {
    const piece = data.subarray(start, start + 255);
    blocks[offset++] = piece.length;
    blocks.set(piece, offset);
    offset += piece.length;
  }
  blocks[offset++] = 0;
  return blocks.subarray(0, offset);
}

function u16(value: number): number[] {
  if (!Number.isInteger(value) || value < 0 || value > 0xffff) throw new Error(`GIF field out of range: ${value}`);
  return [value & 0xff, value >> 8];
}

interface PendingFrame { indices: Uint8Array; duration: number; bbox: [number, number, number, number] | undefined }

/**
 * Streams palette frames into a GIF laid out the way Pillow 12 saves
 * `save(save_all=True, duration=[...], loop=0, disposal=1, optimize=False)`:
 * one global 256-color table and NETSCAPE loop block; identical consecutive
 * frames merge their durations; later frames are cropped to the changed
 * bounding box and carry a local copy of the table; a single remaining frame
 * is written interlaced as a still image. Only the LZW bytes may differ.
 */
export class PaletteGifWriter {
  private readonly parts: Uint8Array[] = [];
  private pending: PendingFrame | undefined;
  private kept = 0;
  private readonly width: number;
  private readonly height: number;
  private readonly palette: Uint8Array;

  constructor(width: number, height: number, palette: Uint8Array) {
    if (palette.length !== 768) throw new Error('GIF palette must contain 256 RGB entries');
    this.width = width;
    this.height = height;
    this.palette = palette;
    this.parts.push(Uint8Array.from([0x47, 0x49, 0x46, 0x38, 0x39, 0x61, ...u16(width), ...u16(height), 0x80 | 7, 0, 0]));
    this.parts.push(palette);
    this.parts.push(Uint8Array.from([0x21, 0xff, 11, ...Array.from('NETSCAPE2.0', (character) => character.charCodeAt(0)), 3, 1, ...u16(0), 0]));
  }

  add(indices: Uint8Array, duration: number): void {
    if (indices.length !== this.width * this.height) throw new Error('GIF frame size does not match the canvas');
    const previous = this.pending;
    if (!previous) {
      this.pending = { indices, duration, bbox: undefined };
      this.kept = 1;
      return;
    }
    // Pillow diffs against the last frame it kept, not a merged duplicate.
    const { width, height } = this;
    const last = previous.indices;
    let left = width, top = height, right = -1, bottom = -1;
    for (let y = 0; y < height; y += 1) {
      const row = y * width;
      for (let x = 0; x < width; x += 1) {
        if (indices[row + x] !== last[row + x]) {
          if (x < left) left = x;
          if (x > right) right = x;
          if (y < top) top = y;
          bottom = y;
        }
      }
    }
    if (right < 0) {
      if (duration) previous.duration += duration;
      return;
    }
    this.writeFrame(previous, false);
    this.pending = { indices, duration, bbox: [left, top, right + 1, bottom + 1] };
    this.kept += 1;
  }

  finish(): Uint8Array {
    if (!this.pending) throw new Error('GIF needs at least one frame');
    this.writeFrame(this.pending, this.kept === 1);
    this.pending = undefined;
    this.parts.push(Uint8Array.from([0x3b]));
    const total = this.parts.reduce((sum, part) => sum + part.length, 0);
    const result = new Uint8Array(total);
    let offset = 0;
    for (const part of this.parts) { result.set(part, offset); offset += part.length; }
    return result;
  }

  private writeFrame(frame: PendingFrame, single: boolean): void {
    const { width, height } = this;
    const delay = Math.trunc(frame.duration / 10);
    const [left, top, right, bottom] = frame.bbox ?? [0, 0, width, height];
    const frameWidth = right - left, frameHeight = bottom - top;
    let indices = frame.indices;
    if (frame.bbox && (frameWidth !== width || frameHeight !== height)) {
      indices = new Uint8Array(frameWidth * frameHeight);
      for (let y = 0; y < frameHeight; y += 1) indices.set(frame.indices.subarray((top + y) * width + left, (top + y) * width + right), y * frameWidth);
    }
    const interlace = single && Math.min(width, height) >= 16;
    const flags = (frame.bbox ? 0x80 | 7 : 0) | (interlace ? 0x40 : 0);
    this.parts.push(Uint8Array.from([0x21, 0xf9, 4, 1 << 2, ...u16(delay), 0, 0, 0x2c, ...u16(left), ...u16(top), ...u16(frameWidth), ...u16(frameHeight), flags]));
    if (frame.bbox) this.parts.push(this.palette);
    this.parts.push(Uint8Array.from([8]));
    if (interlace) {
      const ordered = new Uint8Array(indices.length);
      let target = 0;
      for (const [start, step] of [[0, 8], [4, 8], [2, 4], [1, 2]] as const) {
        for (let y = start; y < frameHeight; y += step) { ordered.set(indices.subarray(y * frameWidth, (y + 1) * frameWidth), target); target += frameWidth; }
      }
      indices = ordered;
    }
    this.parts.push(encodeLzw(indices));
  }
}
