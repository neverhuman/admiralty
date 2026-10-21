// WebP container parsing (libwebp demux semantics) and animation compositing
// (a port of libwebp's WebPAnimDecoder, which Pillow uses). Lossless VP8L frame
// bitstreams are decoded by FFmpeg, already a required capture tool.
import type { RgbImage } from './png.ts';

export interface WebpFrame {
  left: number; top: number; width: number; height: number;
  duration: number;
  disposeToBackground: boolean;
  blend: boolean;
  hasAlpha: boolean;
  lossless: boolean;
  /** The frame's image chunks (ALPH/VP8/VP8L) including chunk headers. */
  chunks: Uint8Array[];
}

export interface WebpFile {
  canvasWidth: number;
  canvasHeight: number;
  loop: number;
  frames: WebpFrame[];
}

/** Decodes one standalone still WebP to straight RGBA (width * height * 4 bytes). */
export type StillDecoder = (still: Uint8Array, width: number, height: number) => Uint8Array;

const text = (bytes: Uint8Array, start: number, length: number) => String.fromCharCode(...bytes.subarray(start, start + length));
const le16 = (bytes: Uint8Array, at: number) => (bytes[at] ?? 0) | ((bytes[at + 1] ?? 0) << 8);
const le24 = (bytes: Uint8Array, at: number) => le16(bytes, at) | ((bytes[at + 2] ?? 0) << 16);
const le32 = (bytes: Uint8Array, at: number) => (le24(bytes, at) + (bytes[at + 3] ?? 0) * 0x1000000);

export function isWebp(bytes: Uint8Array): boolean {
  return text(bytes, 0, 4) === 'RIFF' && text(bytes, 8, 4) === 'WEBP' && ['VP8 ', 'VP8L', 'VP8X'].includes(text(bytes, 12, 4));
}

interface Chunk { fourcc: string; start: number; payload: Uint8Array; end: number }

function readChunk(bytes: Uint8Array, start: number, limit: number): Chunk {
  if (start + 8 > limit) throw new Error('truncated WebP chunk header');
  const size = le32(bytes, start + 4);
  const end = start + 8 + size + (size & 1);
  if (start + 8 + size > limit) throw new Error('truncated WebP chunk');
  return { fourcc: text(bytes, start, 4), start, payload: bytes.subarray(start + 8, start + 8 + size), end: Math.min(end, limit) };
}

interface Features { width: number; height: number; hasAlpha: boolean; lossless: boolean }

function bitstreamFeatures(chunk: Chunk): Features {
  const data = chunk.payload;
  if (chunk.fourcc === 'VP8L') {
    if (data.length < 5 || data[0] !== 0x2f) throw new Error('invalid VP8L header');
    const bits = le32(data, 1);
    if (bits >>> 29 !== 0) throw new Error('unsupported VP8L version');
    return { width: (bits & 0x3fff) + 1, height: ((bits >>> 14) & 0x3fff) + 1, hasAlpha: ((bits >>> 28) & 1) === 1, lossless: true };
  }
  if (data.length < 10 || data[3] !== 0x9d || data[4] !== 0x01 || data[5] !== 0x2a || ((data[0] ?? 0) & 1) !== 0) {
    throw new Error('invalid VP8 header');
  }
  return { width: le16(data, 6) & 0x3fff, height: le16(data, 8) & 0x3fff, hasAlpha: false, lossless: false };
}

/** Collects one frame's ALPH/VP8/VP8L chunks the way libwebp's StoreFrame does. */
function storeFrame(bytes: Uint8Array, start: number, limit: number): { features: Features; hasAlpha: boolean; chunks: Uint8Array[]; next: number } {
  let offset = start, alpha: Chunk | undefined, image: Chunk | undefined;
  while (offset < limit) {
    const chunk = readChunk(bytes, offset, limit);
    if (chunk.fourcc === 'ALPH' && !alpha && !image) alpha = chunk;
    else if ((chunk.fourcc === 'VP8 ' || chunk.fourcc === 'VP8L') && !image) {
      if (chunk.fourcc === 'VP8L' && alpha) throw new Error('VP8L frame cannot carry an ALPH chunk');
      image = chunk;
    } else break;
    offset = chunk.end;
  }
  if (!image) throw new Error('WebP frame has no image bitstream');
  const features = bitstreamFeatures(image);
  const chunks = [alpha, image].filter((chunk): chunk is Chunk => Boolean(chunk)).map((chunk) => bytes.subarray(chunk.start, chunk.end));
  return { features, hasAlpha: Boolean(alpha) || features.hasAlpha, chunks, next: offset };
}

/** Parses canvas, loop count and frames as libwebp's demuxer reports them. */
export function parseWebp(bytes: Uint8Array): WebpFile {
  if (!isWebp(bytes)) throw new Error('not a WEBP file');
  const riffEnd = Math.min(bytes.length, 8 + le32(bytes, 4));
  const first = readChunk(bytes, 12, riffEnd);
  if (first.fourcc !== 'VP8X') {
    const stored = storeFrame(bytes, 12, riffEnd);
    const { width, height } = stored.features;
    return { canvasWidth: width, canvasHeight: height, loop: 1, frames: [{ left: 0, top: 0, width, height, duration: 0,
      disposeToBackground: false, blend: true, hasAlpha: stored.hasAlpha, lossless: stored.features.lossless, chunks: stored.chunks }] };
  }
  if (first.payload.length < 10) throw new Error('invalid VP8X chunk');
  const flags = first.payload[0] ?? 0;
  const animated = (flags & 0x02) !== 0;
  const canvasWidth = le24(first.payload, 4) + 1, canvasHeight = le24(first.payload, 7) + 1;
  let loop = 1, animSeen = false;
  const frames: WebpFrame[] = [];
  for (let offset = first.end; offset < riffEnd;) {
    const chunk = readChunk(bytes, offset, riffEnd);
    if (chunk.fourcc === 'ANIM') {
      if (chunk.payload.length < 6) throw new Error('invalid ANIM chunk');
      if (!animSeen) { loop = le16(chunk.payload, 4); animSeen = true; }
    } else if (chunk.fourcc === 'ANMF') {
      if (!animSeen) throw new Error('ANMF chunk precedes ANIM');
      const header = chunk.payload;
      if (header.length < 16) throw new Error('invalid ANMF chunk');
      const stored = storeFrame(bytes, chunk.start + 8 + 16, chunk.start + 8 + header.length);
      if (animated) {
        const bits = header[15] ?? 0;
        const declaredWidth = le24(header, 6) + 1, declaredHeight = le24(header, 9) + 1;
        if (declaredWidth !== stored.features.width || declaredHeight !== stored.features.height) {
          throw new Error('WebP frame size does not match its bitstream');
        }
        frames.push({ left: le24(header, 0) * 2, top: le24(header, 3) * 2, width: stored.features.width, height: stored.features.height,
          duration: le24(header, 12), disposeToBackground: (bits & 1) !== 0, blend: (bits & 2) === 0,
          hasAlpha: stored.hasAlpha, lossless: stored.features.lossless, chunks: stored.chunks });
      }
    } else if (chunk.fourcc === 'ALPH' || chunk.fourcc === 'VP8 ' || chunk.fourcc === 'VP8L') {
      if (animated || animSeen || frames.length) throw new Error('unexpected still image in animated WebP');
      const stored = storeFrame(bytes, offset, riffEnd);
      frames.push({ left: 0, top: 0, width: stored.features.width, height: stored.features.height, duration: 0,
        disposeToBackground: false, blend: true, hasAlpha: stored.hasAlpha, lossless: stored.features.lossless, chunks: stored.chunks });
      offset = stored.next;
      continue;
    }
    offset = chunk.end;
  }
  if (!frames.length) throw new Error('WebP contains no frames');
  for (const frame of frames) {
    const exact = !animated;
    if (exact ? frame.left || frame.top || frame.width !== canvasWidth || frame.height !== canvasHeight
      : frame.left + frame.width > canvasWidth || frame.top + frame.height > canvasHeight) {
      throw new Error('WebP frame lies outside the canvas');
    }
  }
  return { canvasWidth, canvasHeight, loop, frames };
}

/** Wraps a frame's image chunks as a standalone simple-format WebP file. */
export function stillWebp(frame: WebpFrame): Uint8Array {
  if (!frame.lossless) throw new Error('only lossless (VP8L) WebP frames can be decoded for pixel proof');
  const body = frame.chunks.reduce((sum, chunk) => sum + chunk.length, 0);
  const out = new Uint8Array(12 + body);
  out.set([0x52, 0x49, 0x46, 0x46], 0);
  const size = 4 + body;
  out.set([size & 0xff, (size >>> 8) & 0xff, (size >>> 16) & 0xff, (size >>> 24) & 0xff], 4);
  out.set([0x57, 0x45, 0x42, 0x50], 8);
  let offset = 12;
  for (const chunk of frame.chunks) { out.set(chunk, offset); offset += chunk.length; }
  return out;
}

function blendPixel(canvas: Uint8Array, at: number, previous: Uint8Array): void {
  // libwebp BlendPixelNonPremult(src = canvas pixel, dst = previous canvas pixel).
  const srcA = canvas[at + 3] ?? 0;
  if (srcA === 0xff) return;
  if (srcA === 0) {
    canvas[at] = previous[at] ?? 0; canvas[at + 1] = previous[at + 1] ?? 0;
    canvas[at + 2] = previous[at + 2] ?? 0; canvas[at + 3] = previous[at + 3] ?? 0;
    return;
  }
  const dstA = previous[at + 3] ?? 0;
  const dstFactorA = (dstA * (256 - srcA)) >> 8;
  const blendA = (srcA + dstFactorA) & 0xff;
  const scale = Math.floor((1 << 24) / blendA);
  for (let channel = 0; channel < 3; channel += 1) {
    const unscaled = (canvas[at + channel] ?? 0) * srcA + (previous[at + channel] ?? 0) * dstFactorA;
    canvas[at + channel] = Math.floor((unscaled * scale) / 0x1000000) & 0xff;
  }
  canvas[at + 3] = blendA;
}

/**
 * Yields every composited canvas as RGB (alpha dropped, as Pillow's
 * `convert("RGB")` does), following WebPAnimDecoderGetNext exactly: key-frame
 * detection, transparent-black background disposal, and non-premultiplied
 * blending that skips pixels inside a rectangle disposed by the previous frame.
 */
export function* decodeWebpFrames(webp: WebpFile, decodeStill: StillDecoder): Generator<RgbImage> {
  const { canvasWidth: width, canvasHeight: height } = webp;
  const stride = width * 4;
  const current = new Uint8Array(width * height * 4);
  const disposed = new Uint8Array(width * height * 4);
  let previous: WebpFrame | undefined;
  let previousWasKey = false;
  for (const [index, frame] of webp.frames.entries()) {
    const full = (candidate: WebpFrame) => candidate.width === width && candidate.height === height;
    let key: boolean;
    if (index === 0) key = true;
    else if ((!frame.hasAlpha || !frame.blend) && full(frame)) key = true;
    else key = previous !== undefined && previous.disposeToBackground && (full(previous) || previousWasKey);
    if (key) current.fill(0);
    else current.set(disposed);
    const pixels = decodeStill(stillWebp(frame), frame.width, frame.height);
    if (pixels.length !== frame.width * frame.height * 4) throw new Error('WebP frame decoded to an unexpected size');
    for (let y = 0; y < frame.height; y += 1) {
      current.set(pixels.subarray(y * frame.width * 4, (y + 1) * frame.width * 4), (frame.top + y) * stride + frame.left * 4);
    }
    if (index > 0 && frame.blend && !key && previous) {
      for (let y = 0; y < frame.height; y += 1) {
        const canvasY = frame.top + y;
        const ranges: Array<[number, number]> = [];
        if (!previous.disposeToBackground) ranges.push([frame.left, frame.width]);
        else {
          // FindBlendRangeAtRow: only pixels outside the disposed rectangle blend.
          const srcMaxX = frame.left + frame.width, dstMaxX = previous.left + previous.width, dstMaxY = previous.top + previous.height;
          if (canvasY < previous.top || canvasY >= dstMaxY || frame.left >= dstMaxX || srcMaxX <= previous.left) {
            ranges.push([frame.left, frame.width]);
          } else {
            if (frame.left < previous.left) ranges.push([frame.left, previous.left - frame.left]);
            if (srcMaxX > dstMaxX) ranges.push([dstMaxX, srcMaxX - dstMaxX]);
          }
        }
        for (const [left, count] of ranges) {
          for (let x = left; x < left + count; x += 1) blendPixel(current, canvasY * stride + x * 4, disposed);
        }
      }
    }
    previous = frame;
    previousWasKey = key;
    const rgb = new Uint8Array(width * height * 3);
    for (let source = 0, target = 0; source < current.length; source += 4, target += 3) {
      rgb[target] = current[source] ?? 0; rgb[target + 1] = current[source + 1] ?? 0; rgb[target + 2] = current[source + 2] ?? 0;
    }
    disposed.set(current);
    if (frame.disposeToBackground) {
      for (let y = frame.top; y < frame.top + frame.height; y += 1) disposed.fill(0, y * stride + frame.left * 4, y * stride + (frame.left + frame.width) * 4);
    }
    yield { width, height, rgb };
  }
}
