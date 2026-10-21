// Minimal PNG reader with Pillow `Image.open(...).convert("RGB")` semantics.
import { inflateSync } from 'node:zlib';

/** Packed 8-bit RGB pixels, row-major, no padding. */
export interface RgbImage { width: number; height: number; rgb: Uint8Array }

export interface PngInfo {
  width: number;
  height: number;
  bitDepth: number;
  colorType: number;
  interlaced: boolean;
}

const signature = [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];
const channels: Record<number, number> = { 0: 1, 2: 3, 3: 1, 4: 2, 6: 4 };
const validDepths: Record<number, number[]> = { 0: [1, 2, 4, 8, 16], 2: [8, 16], 3: [1, 2, 4, 8], 4: [8, 16], 6: [8, 16] };
const adam7: ReadonlyArray<readonly [number, number, number, number]> = [
  [0, 0, 8, 8], [4, 0, 8, 8], [0, 4, 4, 8], [2, 0, 4, 4], [0, 2, 2, 4], [1, 0, 2, 2], [0, 1, 1, 2],
];

export function isPng(bytes: Uint8Array): boolean {
  return signature.every((value, index) => bytes[index] === value);
}

function u32(bytes: Uint8Array, offset: number): number {
  if (offset + 4 > bytes.length) throw new Error('truncated PNG file');
  return ((bytes[offset] ?? 0) * 0x1000000) + (((bytes[offset + 1] ?? 0) << 16) | ((bytes[offset + 2] ?? 0) << 8) | (bytes[offset + 3] ?? 0));
}

interface Chunk { type: string; data: Uint8Array }

function* chunks(bytes: Uint8Array): Generator<Chunk> {
  if (!isPng(bytes)) throw new Error('not a PNG file');
  let offset = 8;
  while (offset < bytes.length) {
    const length = u32(bytes, offset);
    const type = String.fromCharCode(...bytes.subarray(offset + 4, offset + 8));
    const start = offset + 8;
    if (start + length + 4 > bytes.length) throw new Error(`truncated PNG chunk ${type}`);
    yield { type, data: bytes.subarray(start, start + length) };
    if (type === 'IEND') return;
    offset = start + length + 4;
  }
}

export function readPngInfo(bytes: Uint8Array): PngInfo {
  const first = chunks(bytes).next();
  if (first.done || first.value.type !== 'IHDR' || first.value.data.length < 13) throw new Error('PNG is missing IHDR');
  const data = first.value.data;
  const info = { width: u32(data, 0), height: u32(data, 4), bitDepth: data[8] ?? 0, colorType: data[9] ?? 0, interlaced: data[12] === 1 };
  if (!info.width || !info.height || !(validDepths[info.colorType] ?? []).includes(info.bitDepth) || (data[12] ?? 0) > 1) {
    throw new Error('unsupported PNG header');
  }
  return info;
}

function paeth(left: number, up: number, upLeft: number): number {
  const estimate = left + up - upLeft;
  const distanceLeft = Math.abs(estimate - left), distanceUp = Math.abs(estimate - up), distanceUpLeft = Math.abs(estimate - upLeft);
  if (distanceLeft <= distanceUp && distanceLeft <= distanceUpLeft) return left;
  return distanceUp <= distanceUpLeft ? up : upLeft;
}

/** Reverses PNG scanline filters; returns rows without their filter bytes. */
function unfilter(data: Uint8Array, offset: number, rowBytes: number, rows: number, bytesPerPixel: number): Uint8Array {
  const out = new Uint8Array(rowBytes * rows);
  for (let y = 0; y < rows; y += 1) {
    const filter = data[offset + y * (rowBytes + 1)];
    const source = offset + y * (rowBytes + 1) + 1;
    const row = y * rowBytes, previous = row - rowBytes;
    out.set(data.subarray(source, source + rowBytes), row);
    if (filter === 0) continue;
    if (filter === 1) {
      for (let x = bytesPerPixel; x < rowBytes; x += 1) out[row + x] = (out[row + x]! + out[row + x - bytesPerPixel]!) & 0xff;
    } else if (filter === 2) {
      if (y > 0) for (let x = 0; x < rowBytes; x += 1) out[row + x] = (out[row + x]! + out[previous + x]!) & 0xff;
    } else if (filter === 3) {
      for (let x = 0; x < rowBytes; x += 1) {
        const left = x >= bytesPerPixel ? out[row + x - bytesPerPixel]! : 0;
        const up = y > 0 ? out[previous + x]! : 0;
        out[row + x] = (out[row + x]! + ((left + up) >> 1)) & 0xff;
      }
    } else if (filter === 4) {
      for (let x = 0; x < rowBytes; x += 1) {
        const left = x >= bytesPerPixel ? out[row + x - bytesPerPixel]! : 0;
        const up = y > 0 ? out[previous + x]! : 0;
        const upLeft = y > 0 && x >= bytesPerPixel ? out[previous + x - bytesPerPixel]! : 0;
        out[row + x] = (out[row + x]! + paeth(left, up, upLeft)) & 0xff;
      }
    } else {
      throw new Error(`invalid PNG filter type ${String(filter)}`);
    }
  }
  return out;
}

/** Decodes the default PNG image to RGB exactly as Pillow's `convert("RGB")`. */
export function decodePng(bytes: Uint8Array): RgbImage {
  const info = readPngInfo(bytes);
  const idat: Uint8Array[] = [];
  let palette: Uint8Array | undefined;
  for (const chunk of chunks(bytes)) {
    if (chunk.type === 'PLTE') palette = chunk.data;
    else if (chunk.type === 'IDAT') idat.push(chunk.data);
  }
  if (!idat.length) throw new Error('PNG has no image data');
  const compressed = new Uint8Array(idat.reduce((sum, part) => sum + part.length, 0));
  let cursor = 0;
  for (const part of idat) { compressed.set(part, cursor); cursor += part.length; }
  const data = inflateSync(compressed);
  const { width, height, bitDepth, colorType } = info;
  const samples = channels[colorType] ?? 1;
  const bitsPerPixel = samples * bitDepth;
  const bytesPerPixel = Math.max(1, bitsPerPixel >> 3);
  // Unused palette entries follow Pillow's grayscale-ramp palette initialization.
  const lookup = new Uint8Array(768);
  for (let index = 0; index < 256; index += 1) lookup.fill(index, index * 3, index * 3 + 3);
  if (palette) lookup.set(palette.subarray(0, Math.min(768, palette.length - (palette.length % 3))));
  const graySmall = bitDepth === 1 ? 255 : bitDepth === 2 ? 85 : bitDepth === 4 ? 17 : 1;
  const rgb = new Uint8Array(width * height * 3);
  const passes = info.interlaced ? adam7 : [[0, 0, 1, 1] as const];
  let offset = 0;
  for (const [x0, y0, dx, dy] of passes) {
    const passWidth = Math.ceil((width - x0) / dx), passHeight = Math.ceil((height - y0) / dy);
    if (passWidth <= 0 || passHeight <= 0) continue;
    const rowBytes = Math.ceil((passWidth * bitsPerPixel) / 8);
    if (offset + (rowBytes + 1) * passHeight > data.length) throw new Error('truncated PNG image data');
    const rows = unfilter(data, offset, rowBytes, passHeight, bytesPerPixel);
    offset += (rowBytes + 1) * passHeight;
    for (let y = 0; y < passHeight; y += 1) {
      const row = y * rowBytes;
      if (bitDepth === 8 && (colorType === 2 || colorType === 6) && dx === 1) {
        let target = (y0 + y * dy) * width * 3;
        for (let at = row, end = row + rowBytes; at < end; at += samples, target += 3) {
          rgb[target] = rows[at]!; rgb[target + 1] = rows[at + 1]!; rgb[target + 2] = rows[at + 2]!;
        }
        continue;
      }
      for (let x = 0; x < passWidth; x += 1) {
        const target = (((y0 + y * dy) * width) + x0 + x * dx) * 3;
        let red: number, green: number, blue: number;
        if (bitDepth < 8) {
          const bit = x * bitDepth;
          const sample = ((rows[row + (bit >> 3)] ?? 0) >> (8 - bitDepth - (bit & 7))) & ((1 << bitDepth) - 1);
          if (colorType === 3) {
            red = lookup[sample * 3] ?? 0; green = lookup[sample * 3 + 1] ?? 0; blue = lookup[sample * 3 + 2] ?? 0;
          } else {
            red = green = blue = sample * graySmall;
          }
        } else {
          const stride = bitDepth === 16 ? 2 : 1;
          const at = row + x * samples * stride;
          const sample = (channel: number) => rows[at + channel * stride] ?? 0;
          if (colorType === 2 || colorType === 6) {
            red = sample(0); green = sample(1); blue = sample(2);
          } else if (colorType === 3) {
            const index = sample(0);
            red = lookup[index * 3] ?? 0; green = lookup[index * 3 + 1] ?? 0; blue = lookup[index * 3 + 2] ?? 0;
          } else if (colorType === 0 && bitDepth === 16) {
            // Pillow opens 16-bit grayscale as I;16 and clamps it to 8 bits.
            red = green = blue = sample(0) === 0 ? rows[at + 1] ?? 0 : 255;
          } else {
            red = green = blue = sample(0);
          }
        }
        rgb[target] = red; rgb[target + 1] = green; rgb[target + 2] = blue;
      }
    }
  }
  return { width, height, rgb };
}
