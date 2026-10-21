// Hermetic tests for the TypeScript demo codecs that replaced Pillow. Parity
// with Pillow 12 was also cross-checked on the committed docs/demos media.
import assert from 'node:assert/strict';
import { mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { test } from 'node:test';
import { deflateSync } from 'node:zlib';
import { decodeGifFrames, decodeLzw, encodeLzw, PaletteGifWriter, parseGif, quantize } from './gif.ts';
import type { GifFrame } from './gif.ts';
import { verifyGifOptimization } from './media.ts';
import { decodePng, readPngInfo } from './png.ts';
import { cliHelp, CliExit, parseCli, PyFloat, pyFixed, pyFloat, pyFloatRepr, pyJsonDumps, pySplitlines, pyStrip } from './pycompat.ts';
import type { CliSpec } from './pycompat.ts';
import { decodeWebpFrames, parseWebp } from './webp.ts';

// ---------------------------------------------------------------- PNG

function crc32(bytes: Uint8Array): number {
  let crc = 0xffffffff;
  for (const byte of bytes) {
    crc ^= byte;
    for (let bit = 0; bit < 8; bit += 1) crc = (crc >>> 1) ^ (0xedb88320 & -(crc & 1));
  }
  return (crc ^ 0xffffffff) >>> 0;
}

function chunk(type: string, data: Uint8Array): Uint8Array {
  const body = new Uint8Array(4 + data.length);
  body.set(Array.from(type, (character) => character.charCodeAt(0)));
  body.set(data, 4);
  const out = new Uint8Array(12 + data.length);
  const view = new DataView(out.buffer);
  view.setUint32(0, data.length);
  out.set(body, 4);
  view.setUint32(8 + data.length, crc32(body));
  return out;
}

function concat(parts: Uint8Array[]): Uint8Array {
  const out = new Uint8Array(parts.reduce((sum, part) => sum + part.length, 0));
  let offset = 0;
  for (const part of parts) { out.set(part, offset); offset += part.length; }
  return out;
}

/** Encodes raw scanlines (already packed per the PNG format) with one filter type. */
function filterRows(rows: Uint8Array[], bytesPerPixel: number, filter: number): Uint8Array {
  const out: number[] = [];
  rows.forEach((row, y) => {
    const previous = y > 0 ? rows[y - 1] : undefined;
    out.push(filter);
    row.forEach((value, x) => {
      const left = x >= bytesPerPixel ? row[x - bytesPerPixel] ?? 0 : 0;
      const up = previous?.[x] ?? 0;
      const upLeft = previous && x >= bytesPerPixel ? previous[x - bytesPerPixel] ?? 0 : 0;
      const estimate = left + up - upLeft;
      const paeth = Math.abs(estimate - left) <= Math.abs(estimate - up) && Math.abs(estimate - left) <= Math.abs(estimate - upLeft)
        ? left : Math.abs(estimate - up) <= Math.abs(estimate - upLeft) ? up : upLeft;
      const predictor = [0, left, up, (left + up) >> 1, paeth][filter] ?? 0;
      out.push((value - predictor) & 0xff);
    });
  });
  return Uint8Array.from(out);
}

function png(width: number, height: number, depth: number, colorType: number, scanlines: Uint8Array, extra: Uint8Array[] = [], interlace = 0): Uint8Array {
  const header = new Uint8Array(13);
  const view = new DataView(header.buffer);
  view.setUint32(0, width); view.setUint32(4, height);
  header.set([depth, colorType, 0, 0, interlace], 8);
  return concat([Uint8Array.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]), chunk('IHDR', header), ...extra,
    chunk('IDAT', deflateSync(scanlines)), chunk('IEND', new Uint8Array())]);
}

test('PNG RGB decoding reverses every scanline filter', () => {
  const width = 5, height = 4;
  const rows = Array.from({ length: height }, (_, y) => Uint8Array.from({ length: width * 3 }, (_, x) => (x * 37 + y * 91 + 13) & 0xff));
  const expected = concat(rows);
  for (let filter = 0; filter <= 4; filter += 1) {
    const image = decodePng(png(width, height, 8, 2, filterRows(rows, 3, filter)));
    assert.deepEqual([image.width, image.height], [width, height]);
    assert.deepEqual(image.rgb, expected, `filter ${filter}`);
  }
  // RGBA drops alpha; 16-bit samples keep their most significant byte.
  const rgba = decodePng(png(2, 1, 8, 6, filterRows([Uint8Array.from([1, 2, 3, 4, 5, 6, 7, 8])], 4, 1)));
  assert.deepEqual(rgba.rgb, Uint8Array.from([1, 2, 3, 5, 6, 7]));
  const deep = decodePng(png(1, 1, 16, 6, filterRows([Uint8Array.from([0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc, 0xde, 0xf0])], 8, 0)));
  assert.deepEqual(deep.rgb, Uint8Array.from([0x12, 0x56, 0x9a]));
});

test('PNG grayscale, palette and Adam7 interlacing follow Pillow conversions', () => {
  // 2-bit gray scales by 85; 16-bit gray clamps like Pillow's I;16 -> RGB.
  assert.deepEqual(decodePng(png(4, 1, 2, 0, Uint8Array.from([0, 0b00011011]))).rgb, Uint8Array.from([0, 0, 0, 85, 85, 85, 170, 170, 170, 255, 255, 255]));
  assert.deepEqual(decodePng(png(2, 1, 16, 0, Uint8Array.from([0, 0x00, 0x7f, 0x01, 0x00]))).rgb, Uint8Array.from([127, 127, 127, 255, 255, 255]));
  // A short PLTE leaves Pillow's grayscale ramp for the remaining indices.
  const palette = chunk('PLTE', Uint8Array.from([10, 20, 30, 40, 50, 60]));
  assert.deepEqual(decodePng(png(3, 1, 4, 3, Uint8Array.from([0, 0x01, 0x50]), [palette])).rgb,
    Uint8Array.from([10, 20, 30, 40, 50, 60, 5, 5, 5]));
  // Adam7: build each pass's scanlines from a known 9x9 image.
  const width = 9, height = 9;
  const pixel = (x: number, y: number) => [(x * 29 + y) & 0xff, (y * 31 + x) & 0xff, (x * y) & 0xff];
  const passes: Array<[number, number, number, number]> = [[0, 0, 8, 8], [4, 0, 8, 8], [0, 4, 4, 8], [2, 0, 4, 4], [0, 2, 2, 4], [1, 0, 2, 2], [0, 1, 1, 2]];
  const parts: Uint8Array[] = [];
  for (const [x0, y0, dx, dy] of passes) {
    const rows: Uint8Array[] = [];
    for (let y = y0; y < height; y += dy) {
      const row: number[] = [];
      for (let x = x0; x < width; x += dx) row.push(...pixel(x, y));
      if (row.length) rows.push(Uint8Array.from(row));
    }
    if (rows.length) parts.push(filterRows(rows, 3, 4));
  }
  const interlaced = decodePng(png(width, height, 8, 2, concat(parts), [], 1));
  const expected: number[] = [];
  for (let y = 0; y < height; y += 1) for (let x = 0; x < width; x += 1) expected.push(...pixel(x, y));
  assert.deepEqual(interlaced.rgb, Uint8Array.from(expected));
  assert.equal(readPngInfo(png(width, height, 8, 2, concat(parts), [], 1)).interlaced, true);
  assert.throws(() => decodePng(Uint8Array.from([1, 2, 3])), /not a PNG file/);
});

// ---------------------------------------------------------------- GIF

test('GIF LZW round-trips long, repetitive and table-filling streams', () => {
  let seed = 7;
  const random = () => { seed = (seed * 1103515245 + 12345) >>> 0; return seed >>> 24; };
  const inputs = [Uint8Array.from([5]), new Uint8Array(70_000).fill(9), Uint8Array.from({ length: 50_000 }, random),
    Uint8Array.from({ length: 30_000 }, (_, index) => (index % 7) * 30)];
  for (const indices of inputs) {
    const data = encodeLzw(indices);
    const frame: GifFrame = { left: 0, top: 0, width: indices.length, height: 1, interlaced: false, palette: undefined,
      transparency: undefined, duration: undefined, disposal: 0, minCodeSize: 8, data };
    assert.deepEqual(decodeLzw(frame), indices);
  }
});

function paletteBytes(): Uint8Array {
  return Uint8Array.from({ length: 768 }, (_, index) => (index * 7 + 3) & 0xff);
}

function expectedRgb(indices: Uint8Array, palette: Uint8Array): Uint8Array {
  const out = new Uint8Array(indices.length * 3);
  indices.forEach((value, index) => out.set(palette.subarray(value * 3, value * 3 + 3), index * 3));
  return out;
}

test('GIF writer merges identical frames, crops changes and decodes back exactly', () => {
  const width = 20, height = 18, palette = paletteBytes();
  const base = Uint8Array.from({ length: width * height }, (_, index) => index % 251);
  const changed = base.slice(); changed[5 * width + 7] = 250; changed[9 * width + 12] = 3;
  const writer = new PaletteGifWriter(width, height, palette);
  writer.add(base, 1500); writer.add(base.slice(), 100); writer.add(changed, 80); writer.add(changed.slice(), 20);
  const gif = parseGif(writer.finish());
  assert.equal(gif.loop, 0);
  assert.equal(gif.frames.length, 2);
  assert.deepEqual(gif.frames.map((frame) => frame.duration), [1600, 100]);
  assert.deepEqual(gif.frames.map((frame) => [frame.left, frame.top, frame.width, frame.height]), [[0, 0, 20, 18], [7, 5, 6, 5]]);
  assert.deepEqual(gif.frames.map((frame) => frame.disposal), [1, 1]);
  assert.ok(gif.frames[1]?.palette, 'delta frames carry a local copy of the palette');
  const frames = [...decodeGifFrames(gif)];
  assert.deepEqual(frames.map((frame) => frame.rgb), [expectedRgb(base, palette), expectedRgb(changed, palette)]);
  // A single remaining frame is written as an interlaced still, like Pillow.
  const still = new PaletteGifWriter(width, height, palette);
  still.add(base, 40); still.add(base.slice(), 60);
  const single = parseGif(still.finish());
  assert.deepEqual(single.frames.map((frame) => [frame.interlaced, frame.duration]), [[true, 100]]);
  assert.deepEqual([...decodeGifFrames(single)][0]?.rgb, expectedRgb(base, palette));
});

test('GIF compositing applies transparency and disposal like Pillow', () => {
  // 4x1 canvas; global palette: 0 black, 1 red, 2 green, 3 blue.
  const palette = Uint8Array.from([0, 0, 0, 255, 0, 0, 0, 255, 0, 0, 0, 255]);
  const frame = (left: number, pixels: number[], flags: number, transparency = 0) => [0x21, 0xf9, 4, flags, 10, 0, transparency, 0,
    0x2c, left, 0, 0, 0, pixels.length, 0, 1, 0, 0, 8, ...Array.from(encodeLzw(Uint8Array.from(pixels)))];
  const bytes = Uint8Array.from([0x47, 0x49, 0x46, 0x38, 0x39, 0x61, 4, 0, 1, 0, 0x81, 1, 0, ...palette,
    ...frame(0, [1, 1, 1, 1], 0), // red everywhere
    ...frame(1, [2, 3], (2 << 2) | 1, 3), // green at x=1, blue index transparent; then dispose to background
    ...frame(2, [3], 3 << 2), // blue at x=2, then restore previous
    ...frame(3, [2], 0), 0x3b]);
  const gif = parseGif(bytes);
  const frames = [...decodeGifFrames(gif)].map((image) => Array.from(image.rgb));
  assert.deepEqual(frames, [
    [255, 0, 0, 255, 0, 0, 255, 0, 0, 255, 0, 0],
    [255, 0, 0, 0, 255, 0, 255, 0, 0, 255, 0, 0],
    // Frame 1 disposed its 2-pixel rectangle to its transparency color (index 3, blue).
    [255, 0, 0, 0, 0, 255, 0, 0, 255, 255, 0, 0],
    // Frame 2 restored the pixel it covered before frame 3 drew green at x=3.
    [255, 0, 0, 0, 0, 255, 0, 0, 255, 0, 255, 0],
  ]);
  assert.deepEqual(gif.frames.map((item) => item.duration), [100, 100, 100, 100]);
  assert.equal(gif.loop, undefined);
});

test('GIF optimization proof accepts equal pixels and rejects any change', () => {
  const directory = mkdtempSync(path.join(tmpdir(), 'admiralty-gif-proof-'));
  try {
    const width = 16, height = 16, palette = paletteBytes();
    const first = Uint8Array.from({ length: width * height }, (_, index) => index & 0xff);
    const second = first.slice(); second[40] = 9;
    const write = (name: string, frames: Uint8Array[]) => {
      const writer = new PaletteGifWriter(width, height, palette);
      frames.forEach((frame, index) => writer.add(frame, 100 + index * 10));
      const file = path.join(directory, name);
      writeFileSync(file, writer.finish());
      return file;
    };
    const reference = write('reference.gif', [first, second]);
    assert.equal(verifyGifOptimization(reference, write('same.gif', [first, second])), 2);
    const third = second.slice(); third[41] = 1;
    assert.throws(() => verifyGifOptimization(reference, write('changed.gif', [first, third])), /Pixel mismatch: GIF optimized frame 1/);
  } finally { rmSync(directory, { recursive: true, force: true }); }
});

test('palette quantization reproduces Pillow cache-cell nearest colors and ties', () => {
  const palette = new Uint8Array(768);
  palette.set([0, 0, 0, 3, 3, 3, 200, 0, 0, 200, 0, 0], 0);
  const image = { width: 4, height: 1, rgb: Uint8Array.from([3, 3, 3, 4, 4, 4, 199, 1, 1, 255, 255, 255]) };
  // (3,3,3) maps through its cell origin (0,0,0); (4,4,4) is nearer index 1;
  // index 2 wins the tie with its duplicate 3; white picks the nearest entry.
  assert.deepEqual(quantize(image, palette), Uint8Array.from([0, 1, 2, 2]));
});

// ---------------------------------------------------------------- WebP

function riffChunk(fourcc: string, payload: number[]): number[] {
  const size = payload.length;
  return [...Array.from(fourcc, (character) => character.charCodeAt(0)), size & 0xff, (size >> 8) & 0xff, (size >> 16) & 0xff, size >>> 24,
    ...payload, ...(size & 1 ? [0] : [])];
}

const le24 = (value: number) => [value & 0xff, (value >> 8) & 0xff, (value >> 16) & 0xff];

function vp8l(width: number, height: number, alpha: boolean): number[] {
  const bits = (width - 1) | ((height - 1) << 14) | (alpha ? 1 << 28 : 0);
  return riffChunk('VP8L', [0x2f, bits & 0xff, (bits >> 8) & 0xff, (bits >> 16) & 0xff, bits >>> 24, 0]);
}

function animation(frames: Array<{ x: number; y: number; w: number; h: number; duration: number; dispose: boolean; blend: boolean; alpha: boolean }>, loop: number): Uint8Array {
  const body = [...Array.from('WEBP', (character) => character.charCodeAt(0)),
    ...riffChunk('VP8X', [0x12, 0, 0, 0, ...le24(3), ...le24(0)]),
    ...riffChunk('ANIM', [0, 0, 0, 0, loop & 0xff, loop >> 8]),
    ...frames.flatMap((frame) => riffChunk('ANMF', [...le24(frame.x / 2), ...le24(frame.y / 2), ...le24(frame.w - 1), ...le24(frame.h - 1),
      ...le24(frame.duration), (frame.dispose ? 1 : 0) | (frame.blend ? 0 : 2), ...vp8l(frame.w, frame.h, frame.alpha)]))];
  return Uint8Array.from([0x52, 0x49, 0x46, 0x46, body.length & 0xff, (body.length >> 8) & 0xff, (body.length >> 16) & 0xff, body.length >>> 24, ...body]);
}

test('WebP compositing follows libwebp key frames, blending and background disposal', () => {
  // 4x1 canvas. Frame 0: opaque red. Frame 1 (2px at x=0, disposed to background):
  // green + transparent. Frame 2 (4px, blended): pixels transparent except x=3 blue.
  const webp = parseWebp(animation([
    { x: 0, y: 0, w: 4, h: 1, duration: 100, dispose: false, blend: false, alpha: false },
    { x: 0, y: 0, w: 2, h: 1, duration: 80, dispose: true, blend: true, alpha: true },
    { x: 0, y: 0, w: 4, h: 1, duration: 60, dispose: false, blend: true, alpha: true },
  ], 0));
  assert.deepEqual([webp.canvasWidth, webp.canvasHeight, webp.loop, webp.frames.length], [4, 1, 0, 3]);
  assert.deepEqual(webp.frames.map((frame) => frame.duration), [100, 80, 60]);
  const red = [255, 0, 0, 255], green = [0, 255, 0, 255], hidden = [9, 9, 9, 0], blue = [0, 0, 255, 255];
  const stills = [[...red, ...red, ...red, ...red], [...green, ...hidden], [...hidden, ...hidden, ...hidden, ...blue]];
  let call = 0;
  const frames = [...decodeWebpFrames(webp, (_still, width, height) => {
    const pixels = Uint8Array.from(stills[call++] ?? []);
    assert.equal(pixels.length, width * height * 4);
    return pixels;
  })].map((image) => Array.from(image.rgb));
  assert.deepEqual(frames, [
    [255, 0, 0, 255, 0, 0, 255, 0, 0, 255, 0, 0],
    // Transparent pixel at x=1 blends with the previous canvas (red).
    [0, 255, 0, 255, 0, 0, 255, 0, 0, 255, 0, 0],
    // Inside frame 1's disposed rectangle nothing blends: hidden RGB stays as decoded.
    [9, 9, 9, 9, 9, 9, 255, 0, 0, 0, 0, 255],
  ]);
  assert.throws(() => parseWebp(Uint8Array.from([1, 2, 3])), /not a WEBP file/);
});

// ---------------------------------------------------------------- Python compatibility

test('Python-compatible formatting matches json.dumps, repr, format and str helpers', () => {
  assert.equal(pyJsonDumps({ a: [1, { b: 'café' }], c: new PyFloat(15), d: [] }),
    '{\n  "a": [\n    1,\n    {\n      "b": "caf\\u00e9"\n    }\n  ],\n  "c": 15.0,\n  "d": []\n}');
  assert.deepEqual([15, 0.1, 1e-5, 1e16, 2.5e-7, 123456.75].map(pyFloatRepr), ['15.0', '0.1', '1e-05', '1e+16', '2.5e-07', '123456.75']);
  assert.deepEqual([0.125, 0.375, 2.675, 11.08, 1.005, -0.001].map((value) => pyFixed(value, 2)), ['0.12', '0.38', '2.67', '11.08', '1.00', '-0.00']);
  assert.deepEqual([pyFloat(' 1_0.5 '), pyFloat('inf'), pyFloat('1e1')], [10.5, Infinity, 10]);
  assert.throws(() => pyFloat('0x10'));
  assert.throws(() => pyFloat(''));
  assert.equal(pyStrip(' \t x\n'), 'x');
  assert.deepEqual(pySplitlines('a\r\nb\n\nc\n'), ['a', 'b', '', 'c']);
});

test('argparse-compatible CLI parsing keeps flags, prefixes, errors and help', () => {
  const spec: CliSpec = { prog: 'encode.ts', description: 'Demo.', positionals: [{ dest: 'capture', help: 'Run directory' }],
    options: [{ flag: '--out', dest: 'out', default: 'docs/demos' },
      { flag: '--max-mib', dest: 'max_mib', type: { name: 'float', parse: (text) => pyFloat(text) }, default: 15 }] };
  assert.deepEqual(parseCli(spec, ['run']), { out: 'docs/demos', max_mib: 15, capture: 'run' });
  assert.deepEqual(parseCli(spec, ['--max=2.5', '--o', 'x', 'run']), { out: 'x', max_mib: 2.5, capture: 'run' });
  const invoke = (argv: string[]) => {
    try { parseCli(spec, argv); } catch (error) { if (error instanceof CliExit) return error; throw error; }
    assert.fail('expected an argparse exit');
  };
  assert.deepEqual([invoke([]).status, invoke([]).stderr], [2, 'usage: encode.ts [-h] [--out OUT] [--max-mib MAX_MIB] capture\nencode.ts: error: the following arguments are required: capture\n']);
  assert.match(invoke(['run', '--max-mib', 'big']).stderr, /error: argument --max-mib: invalid float value: 'big'\n$/);
  assert.match(invoke(['run', '--out']).stderr, /error: argument --out: expected one argument\n$/);
  assert.match(invoke(['run', 'extra', '--bogus']).stderr, /error: unrecognized arguments: extra --bogus\n$/);
  assert.match(invoke(['--bogus', 'run', 'extra']).stderr, /error: unrecognized arguments: --bogus extra\n$/);
  assert.equal(invoke(['--help']).status, 0);
  assert.equal(invoke(['-h']).stdout, cliHelp(spec));
  assert.match(cliHelp(spec), /positional arguments:\n {2}capture +Run directory\n/);
});
