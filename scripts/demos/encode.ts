#!/usr/bin/env node
/** Encode original PNG captures; prove lossless WebP and lossless GIF optimization. */
import { copyFileSync, existsSync, mkdirSync, mkdtempSync, readdirSync, rmSync, statSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { PaletteGifWriter, quantize } from './gif.ts';
import {
  decodeRgb, joinPath, openImage, readJson, resolvePath, run, sha256File, truthy, verifyGifOptimization, verifyWebp, which,
} from './media.ts';
import { isCaptureManifest } from './shapes.ts';
import { cliError, parseCli, PyFloat, pyFixed, pyFloat, pyJsonDumps, pySplitlines, runCli } from './pycompat.ts';
import type { CliSpec } from './pycompat.ts';

const description = 'Encode original PNG captures; prove lossless WebP and lossless GIF optimization.';
const scenesAllowed = new Set(['fleet-console', 'git-flow', 'fleet-atlas']);

/** `--max-mib` keeps Python's distinction between the int default and a parsed float. */
type Budget = number | PyFloat;
const budgetValue = (budget: Budget) => budget instanceof PyFloat ? budget.value : budget;
const budgetText = (budget: Budget) => budget instanceof PyFloat ? budget.text : String(budget);

function firstLine(text: string): string {
  const [line] = pySplitlines(text);
  if (line === undefined) throw new Error('tool version output is empty');
  return line;
}

function encode(directory: string, output: string, maxMib: Budget): void {
  const manifestPath = joinPath(directory, 'capture.json');
  const capture = readJson(manifestPath, isCaptureManifest, 'Capture manifest');
  const name = capture.scene;
  if (capture.schema !== 1 || capture.source.kind !== 'live-production' || truthy(capture.blocked_requests)) {
    throw new Error('Expected a completed, read-only production capture');
  }
  if (!scenesAllowed.has(name) || !truthy(capture.frames)) throw new Error('Unknown or empty scene');
  const paths: string[] = [];
  for (const frame of capture.frames) {
    // pathlib keeps `..`, so only a plain file name stays directly in the scene directory.
    const parts = frame.file.split('/').filter((part) => part !== '' && part !== '.');
    const single = !path.isAbsolute(frame.file) && parts.length === 1 && parts[0] !== '..';
    const file = joinPath(directory, parts.join('/'));
    if (!single || !frame.file.endsWith('.png') || sha256File(file) !== frame.sha256) {
      throw new Error('Source frame path/hash mismatch');
    }
    const duration = frame.duration_ms;
    if (typeof duration !== 'number') throw new TypeError('Frame durations must be numbers');
    if (!(duration >= 20 && duration <= 10000) || duration % 10) {
      throw new Error('Frame durations must be 20–10000 ms, in 10 ms increments');
    }
    const image = openImage(file);
    if (image.width !== capture.viewport.pixel_width || image.height !== capture.viewport.pixel_height) {
      throw new Error('Source frame dimensions changed');
    }
    paths.push(file);
  }

  mkdirSync(output, { recursive: true });
  // Work outside the published directory. Publish only after all verification passes.
  const work = mkdtempSync(path.join(tmpdir(), 'admiralty-encode-'));
  try {
    const palettePath = path.join(work, 'palette.png');
    console.log(`${name}: building a palette from every original RGB frame`);
    // No video intermediate, YUV conversion, scaling, saturation adjustment or lossy pre-pass.
    run(['ffmpeg', '-hide_banner', '-loglevel', 'error', '-y', '-pattern_type', 'glob',
      '-i', joinPath(directory, '*.png'), '-vf', 'palettegen=max_colors=256:reserve_transparent=0:stats_mode=full',
      '-frames:v', '1', '-update', '1', palettePath]);
    const palette = decodeRgb(palettePath).rgb;
    const durations = capture.frames.map((frame) => frame.duration_ms);
    const rawGif = path.join(work, 'raw.gif'), finalGif = path.join(work, `${name}.gif`), webp = path.join(work, `${name}.webp`);
    // Global palette avoids temporal color flicker. No dithering noise around UI text.
    const writer = new PaletteGifWriter(capture.viewport.pixel_width, capture.viewport.pixel_height, palette);
    for (const [index, file] of paths.entries()) writer.add(quantize(decodeRgb(file), palette), durations[index] ?? 0);
    writeFileSync(rawGif, writer.finish());
    run(['gifsicle', '--careful', '--optimize=3', '--no-comments', '--no-names', '--no-interlace',
      rawGif, '-o', finalGif]);
    const gifFrames = verifyGifOptimization(rawGif, finalGif);
    console.log(`${name}: GIF optimization preserves every decoded palette pixel; encoding full-color WebP`);
    // Encode directly from PNGs, never from the already quantized GIF.
    const command = ['img2webp', '-min_size', '-loop', '0', '-lossless', '-m', '6', '-q', '100', '-exact'];
    for (const [index, file] of paths.entries()) command.push('-d', String(durations[index]), file);
    run([...command, '-o', webp]);
    const [duration, webpFrames] = verifyWebp(webp, capture, directory);
    for (const artifact of [finalGif, webp]) {
      if (statSync(artifact).size > budgetValue(maxMib) * 1024 * 1024) {
        throw new Error(`${path.basename(artifact)} exceeds ${budgetText(maxMib)} MiB web budget; shorten the scene or reduce motion, then recapture`);
      }
    }
    const poster = path.join(work, `${name}.png`);
    copyFileSync(paths[0] ?? '', poster);
    const artifacts = Object.fromEntries([finalGif, webp, poster].map((artifact) => [path.basename(artifact),
      { bytes: statSync(artifact).size, sha256: sha256File(artifact) }]));
    const receipt = {
      schema: 1, scene: name, captured_utc: capture.captured_utc,
      source: capture.source, viewport: capture.viewport, editing: capture.editing,
      focus_project: capture.focus_project ?? null,
      steps: capture.steps, duration_ms: duration, source_frames: paths.length,
      gif_frames: gifFrames, webp_frames: webpFrames, capture_manifest_sha256: sha256File(manifestPath),
      allowed_requests: capture.allowed_requests, blocked_requests: [],
      encoding: {
        webp: 'Full-color RGB, lossless from original PNGs; every decoded frame is pixel-identical to its PNG; durations and infinite loop verified',
        gif: 'Global 256-color palette from all PNGs; quantization is lossy; gifsicle -O3 is pixel-lossless relative to that palette GIF; every decoded optimized frame verified',
        resize: false, color_profile: 'Browser forced to sRGB; no YUV or video intermediate',
        web_budget_mib_each: maxMib,
        tools: { ffmpeg: firstLine(run(['ffmpeg', '-version'])),
          gifsicle: firstLine(run(['gifsicle', '--version'])),
          img2webp: run(['img2webp', '-version']), node: process.version },
      }, artifacts,
    };
    for (const artifact of [finalGif, webp, poster]) copyFileSync(artifact, path.join(output, path.basename(artifact)));
    writeFileSync(path.join(output, `${name}.json`), `${pyJsonDumps(receipt)}\n`);
    console.log(`${name}: ${pyFixed(duration / 1000, 2)}s, GIF ${pyFixed(statSync(finalGif).size / 1048576, 2)} MiB, lossless WebP ${pyFixed(statSync(webp).size / 1048576, 2)} MiB`);
  } finally {
    rmSync(work, { recursive: true, force: true });
  }
}

const spec: CliSpec = {
  prog: path.basename(process.argv[1] ?? 'encode.ts'),
  description,
  positionals: [{ dest: 'capture', help: 'Run directory containing scene folders (or a single scene folder)' }],
  options: [
    { flag: '--out', dest: 'out', default: 'docs/demos' },
    { flag: '--max-mib', dest: 'max_mib', type: { name: 'float', parse: (text) => new PyFloat(pyFloat(text)) }, default: 15 },
  ],
};

await runCli(() => {
  const args = parseCli(spec, process.argv.slice(2));
  for (const tool of ['ffmpeg', 'gifsicle', 'img2webp']) {
    if (!which(tool)) cliError(spec, `Missing ${tool}; see docs/operators/demo-capture.md`);
  }
  const maxMib = args.max_mib as Budget;
  if (!(budgetValue(maxMib) > 0 && budgetValue(maxMib) <= 50)) cliError(spec, '--max-mib must be positive and at most 50');
  const directory = resolvePath(String(args.capture));
  const scenes = existsSync(joinPath(directory, 'capture.json')) ? [directory]
    : readdirSync(directory).map((entry) => joinPath(directory, entry))
      .filter((scene) => existsSync(joinPath(scene, 'capture.json'))).sort();
  if (!scenes.length) cliError(spec, 'No completed capture.json files found');
  for (const scene of scenes) encode(scene, resolvePath(String(args.out)), maxMib);
});
