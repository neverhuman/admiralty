#!/usr/bin/env node
/** Verify published demos offline, optionally repeating original-PNG pixel proof. */
import { existsSync, readdirSync, readFileSync, statSync } from 'node:fs';
import path from 'node:path';
import { joinPath, openImage, readJson, sha256File, truthy, verifyWebp } from './media.ts';
import { isCaptureManifest, isReceipt } from './shapes.ts';
import { parseCli, runCli } from './pycompat.ts';
import type { CliSpec } from './pycompat.ts';

const SCENES = ['fleet-console', 'git-flow', 'fleet-atlas'] as const;


/** `Path.glob` yields nothing for a missing or non-directory root. */
function listDirectory(root: string): string[] {
  try { return readdirSync(root); } catch { return []; }
}

const escapeRegExp = (text: string) => text.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');

const spec: CliSpec = {
  prog: path.basename(process.argv[1] ?? 'verify.ts'),
  description: 'Verify published demos offline, optionally repeating original-PNG pixel proof.',
  positionals: [],
  options: [
    { flag: '--dir', dest: 'dir', default: 'docs/demos' },
    { flag: '--readme', dest: 'readme', default: 'README.md' },
    { flag: '--capture', dest: 'capture', append: true, default: [], help: 'Original run or scene folder; repeat for multiple runs' },
  ],
};

await runCli(() => {
  const args = parseCli(spec, process.argv.slice(2));
  const dir = String(args.dir), readmePath = String(args.readme);
  const captures = new Map<string, string>();
  for (const root of args.capture as string[]) {
    const paths = existsSync(joinPath(root, 'capture.json')) ? [joinPath(root, 'capture.json')]
      : listDirectory(root).map((entry) => joinPath(joinPath(root, entry), 'capture.json')).filter((file) => existsSync(file));
    for (const file of paths) captures.set(readJson(file, isCaptureManifest, 'Capture manifest').scene, file);
  }
  const readme = readFileSync(readmePath, 'utf8');
  for (const scene of SCENES) {
    const receipt = readJson(joinPath(dir, `${scene}.json`), isReceipt, 'Receipt');
    if (receipt.scene !== scene || receipt.source.kind !== 'live-production' || truthy(receipt.blocked_requests)) {
      throw new Error(`Invalid production provenance: ${scene}`);
    }
    const size = [receipt.viewport.pixel_width, receipt.viewport.pixel_height] as const;
    for (const extension of ['gif', 'webp', 'png'] as const) {
      const name = `${scene}.${extension}`;
      const file = joinPath(dir, name), artifact = receipt.artifacts[name];
      if (!artifact) throw new Error(`receipt has no artifact entry: ${name}`);
      if (statSync(file).size !== artifact.bytes || sha256File(file) !== artifact.sha256) {
        throw new Error(`Artifact size/hash mismatch: ${name}`);
      }
      // The artifact bytes are pinned by the receipt hash above, so container
      // metadata (every frame header, duration and loop) suffices here; pixel
      // decoding is repeated against the original PNGs with --capture.
      const image = openImage(file);
      if (image.width !== size[0] || image.height !== size[1]) throw new Error(`Unexpected dimensions: ${name}`);
      if (extension !== 'png') {
        if (image.frameCount !== receipt[`${extension}_frames`] || image.loop !== 0) {
          throw new Error(`Animation frames/loop changed: ${name}`);
        }
        let duration = 0;
        for (const [index, frameDuration] of image.durations.entries()) {
          if (frameDuration === undefined) throw new Error(`Animation frame ${index} has no duration: ${name}`);
          duration += frameDuration;
        }
        if (duration !== receipt.duration_ms) throw new Error(`Animation duration changed: ${name}`);
        if (artifact.bytes > receipt.encoding.web_budget_mib_each * 1048576) throw new Error(`Web size budget exceeded: ${name}`);
      }
      // Python's `\b` is Unicode-aware; mirror it after the ASCII tag name.
      if (extension === 'gif' && !new RegExp(`<img(?![\\p{L}\\p{N}_])[^>]*src="docs/demos/${escapeRegExp(name)}"`, 'u').test(readme)) {
        throw new Error(`README is missing the GIF embed: ${name}`);
      }
      if ((extension === 'webp' || extension === 'png') && !readme.includes(`docs/demos/${name}`)) {
        throw new Error(`README is missing the full-color/static link: ${name}`);
      }
    }
    const capturePath = captures.get(scene);
    if (capturePath) {
      if (sha256File(capturePath) !== receipt.capture_manifest_sha256) throw new Error(`Original capture manifest changed: ${scene}`);
      const capture = readJson(capturePath, isCaptureManifest, 'Capture manifest');
      const parent = path.dirname(capturePath);
      for (const frame of capture.frames) {
        if (sha256File(joinPath(parent, frame.file)) !== frame.sha256) throw new Error(`Original source frame changed: ${scene}/${frame.file}`);
      }
      verifyWebp(joinPath(dir, `${scene}.webp`), capture, parent);
    }
    console.log(`${scene}: hashes, ${size[0]}×${size[1]}, timing, loops, web budget, README links verified`
      + (capturePath ? '; original RGB pixels verified' : ''));
  }
});
