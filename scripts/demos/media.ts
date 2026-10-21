// Shared demo-media helpers: tool invocation, image opening and pixel proofs.
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { accessSync, constants, readFileSync, realpathSync, statSync } from 'node:fs';
import path from 'node:path';
import { decodeGifFrames, gifSizeAt, isGif, parseGif } from './gif.ts';
import { decodePng, isPng, readPngInfo } from './png.ts';
import type { RgbImage } from './png.ts';
import { pyStrip } from './pycompat.ts';
import type { CaptureManifest } from './shapes.ts';
import { parseChecked } from './shapes.ts';
import { decodeWebpFrames, isWebp, parseWebp } from './webp.ts';

export const TOOL_TIMEOUT_MS = 900_000;
const TOOL_MAX_BUFFER_BYTES = 256 * 1024 * 1024;

export type { CaptureFrame, CaptureManifest } from './shapes.ts';

/** Python truthiness for JSON values. */
export function truthy(value: unknown): boolean {
  if (Array.isArray(value)) return value.length > 0;
  if (value && typeof value === 'object') return Object.keys(value).length > 0;
  return Boolean(value);
}

/** `pathlib.Path(directory) / name` without lexical normalization of `..`. */
export function joinPath(directory: string, name: string): string {
  if (path.isAbsolute(name) || directory === '') return name;
  return directory.endsWith('/') ? `${directory}${name}` : `${directory}/${name}`;
}

/** `pathlib.Path(value).resolve()` (non-strict): resolves symlinks of the existing prefix. */
export function resolvePath(value: string): string {
  const absolute = path.resolve(value);
  try {
    return realpathSync(absolute);
  } catch {
    const parent = path.dirname(absolute);
    return parent === absolute ? absolute : path.join(resolvePath(parent), path.basename(absolute));
  }
}

export function sha256File(file: string): string {
  return createHash('sha256').update(readFileSync(file)).digest('hex');
}

/** Read a JSON file and require `guard` to accept it (see `shapes.ts`). */
export function readJson<T>(file: string, guard: (value: unknown) => value is T, what: string): T {
  return parseChecked(readFileSync(file, 'utf8'), guard, `${what} ${file}`);
}

/** `shutil.which`: the first executable regular file named `tool` on PATH. */
export function which(tool: string): string | undefined {
  const executable = (candidate: string) => {
    try {
      accessSync(candidate, constants.X_OK);
      return !statSync(candidate).isDirectory();
    } catch { return false; }
  };
  if (tool.includes('/')) return executable(tool) ? tool : undefined;
  for (const directory of (process.env.PATH ?? '/bin:/usr/bin').split(':')) {
    const candidate = path.join(directory || '.', tool);
    if (executable(candidate)) return candidate;
  }
  return undefined;
}

const universalNewlines = (text: string) => text.replace(/\r\n?/g, '\n');

/**
 * Runs an encoder tool with no stdin and a 900 s deadline. A nonzero exit
 * raises `<tool> failed: <last 3000 stderr characters>`; stdout is returned stripped.
 */
export function run(args: ReadonlyArray<string>): string {
  const [command = '', ...rest] = args;
  const result = spawnSync(command, rest, {
    encoding: 'utf8', timeout: TOOL_TIMEOUT_MS, maxBuffer: TOOL_MAX_BUFFER_BYTES,
    stdio: ['ignore', 'pipe', 'pipe'], killSignal: 'SIGKILL',
  });
  if (result.error) {
    if ((result.error as Error & { code?: string }).code === 'ETIMEDOUT') {
      throw new Error(`Command '${command}' timed out after ${TOOL_TIMEOUT_MS / 1000} seconds`);
    }
    throw result.error;
  }
  if (result.status !== 0) {
    throw new Error(`${command} failed: ${universalNewlines(result.stderr ?? '').slice(-3000)}`);
  }
  return pyStrip(universalNewlines(result.stdout ?? ''));
}

/** Decodes a standalone lossless WebP still to RGBA with FFmpeg (no files, no shell). */
export function ffmpegDecodeStill(still: Uint8Array, width: number, height: number): Uint8Array {
  const expected = width * height * 4;
  const result = spawnSync('ffmpeg', ['-hide_banner', '-loglevel', 'error', '-nostdin', '-f', 'webp_pipe', '-i', 'pipe:0',
    '-f', 'rawvideo', '-pix_fmt', 'rgba', '-frames:v', '1', 'pipe:1'], {
    input: still, timeout: TOOL_TIMEOUT_MS, maxBuffer: expected + 1024 * 1024, stdio: ['pipe', 'pipe', 'pipe'], killSignal: 'SIGKILL',
  });
  if (result.error) throw result.error;
  if (result.status !== 0) throw new Error(`ffmpeg failed: ${String(result.stderr ?? '').slice(-3000)}`);
  const pixels = result.stdout;
  if (!pixels || pixels.length !== expected) throw new Error('ffmpeg decoded a WebP frame to an unexpected size');
  return new Uint8Array(pixels.buffer, pixels.byteOffset, pixels.length);
}

/** The subset of a Pillow `Image` that the demo tools rely on. */
export interface MediaImage {
  format: 'PNG' | 'GIF' | 'WEBP';
  /** Size immediately after opening (Pillow `Image.open(...).size`). */
  width: number;
  height: number;
  frameCount: number;
  /** `info.get("loop")`. */
  loop: number | undefined;
  /** Per-frame `info["duration"]` in milliseconds (undefined when absent). */
  durations: Array<number | undefined>;
  /** Composited frames converted to RGB, in order. */
  frames(): Generator<RgbImage>;
}

/** Opens PNG, GIF or WebP by content, like `PIL.Image.open`. */
export function openImage(file: string): MediaImage {
  const bytes = new Uint8Array(readFileSync(file));
  if (isPng(bytes)) {
    const info = readPngInfo(bytes);
    return { format: 'PNG', width: info.width, height: info.height, frameCount: 1, loop: undefined, durations: [undefined],
      *frames() { yield decodePng(bytes); } };
  }
  if (isGif(bytes)) {
    const gif = parseGif(bytes);
    const [width, height] = gifSizeAt(gif, 0);
    return { format: 'GIF', width, height, frameCount: gif.frames.length, loop: gif.loop,
      durations: gif.frames.map((frame) => frame.duration), frames: () => decodeGifFrames(gif) };
  }
  if (isWebp(bytes)) {
    const webp = parseWebp(bytes);
    return { format: 'WEBP', width: webp.canvasWidth, height: webp.canvasHeight, frameCount: webp.frames.length, loop: webp.loop,
      durations: webp.frames.map((frame) => frame.duration), frames: () => decodeWebpFrames(webp, ffmpegDecodeStill) };
  }
  throw new Error(`cannot identify image file '${file}'`);
}

/** Opens the first frame of an image and returns it as RGB. */
export function decodeRgb(file: string): RgbImage {
  const first = openImage(file).frames().next();
  if (first.done) throw new Error(`no image data in '${file}'`);
  return first.value;
}

function sameRgb(actual: RgbImage, expected: RgbImage): boolean {
  if (actual.width !== expected.width || actual.height !== expected.height) return false;
  const left = actual.rgb, right = expected.rgb;
  if (left.length !== right.length) return false;
  for (let index = 0; index < left.length; index += 1) if (left[index] !== right[index]) return false;
  return true;
}

export function checkEqual(actual: RgbImage, expected: RgbImage, description: string): void {
  if (!sameRgb(actual, expected)) throw new Error(`Pixel mismatch: ${description}`);
}

/**
 * Proves every displayed WebP interval equals the original PNG for the same
 * time span. The encoder may merge identical consecutive PNGs, so each source
 * interval is compared against every overlapping decoded interval, including timing.
 */
export function verifyWebp(file: string, capture: Pick<CaptureManifest, 'frames'>, directory: string): [number, number] {
  const animation = openImage(file);
  if (animation.loop !== 0) throw new Error('WebP infinite loop was not preserved');
  const decoded = animation.frames();
  try {
    let encodedIndex = -1, remaining = 0, duration = 0;
    let current: RgbImage | undefined;
    for (const [index, frame] of capture.frames.entries()) {
      let sourceRemaining = frame.duration_ms;
      const source = openImage(joinPath(directory, frame.file));
      let original: RgbImage | undefined;
      while (sourceRemaining) {
        if (!remaining) {
          encodedIndex += 1;
          if (encodedIndex >= animation.frameCount) throw new Error('WebP ended before the source timeline');
          const next = decoded.next();
          if (next.done) throw new Error('failed to decode next frame in WebP file');
          current = next.value;
          const frameDuration = animation.durations[encodedIndex];
          if (frameDuration === undefined) throw new Error('WebP frame has no duration');
          remaining = frameDuration;
          if (remaining <= 0) throw new Error('Invalid WebP frame duration');
        }
        if (!original) {
          const first = source.frames().next();
          if (first.done) throw new Error(`no image data in '${frame.file}'`);
          original = first.value;
        }
        if (!current) throw new Error('WebP ended before the source timeline');
        checkEqual(current, original, `WebP interval versus source PNG ${index}`);
        const overlap = Math.min(remaining, sourceRemaining);
        remaining -= overlap;
        sourceRemaining -= overlap;
        duration += overlap;
      }
    }
    if (remaining || encodedIndex + 1 !== animation.frameCount) throw new Error('WebP extends beyond the source timeline');
    return [duration, animation.frameCount];
  } finally {
    decoded.return(undefined);
  }
}

/** Proves gifsicle kept frame count, looping, timing and every decoded pixel. */
export function verifyGifOptimization(before: string, after: string): number {
  const reference = openImage(before), optimized = openImage(after);
  if (reference.frameCount !== optimized.frameCount || optimized.loop !== 0) {
    throw new Error('GIF optimizer changed frame count or looping');
  }
  const referenceFrames = reference.frames(), optimizedFrames = optimized.frames();
  for (let index = 0; index < reference.frameCount; index += 1) {
    if (reference.durations[index] !== optimized.durations[index]) throw new Error(`GIF optimizer changed frame ${index} timing`);
    const expected = referenceFrames.next(), actual = optimizedFrames.next();
    if (expected.done || actual.done) throw new Error('no more images in GIF file');
    checkEqual(actual.value, expected.value, `GIF optimized frame ${index}`);
  }
  return optimized.frameCount;
}
