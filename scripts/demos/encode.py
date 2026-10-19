#!/usr/bin/env python3
"""Encode original PNG captures; prove lossless WebP and lossless GIF optimization."""
import argparse
import hashlib
import json
import shutil
import subprocess
import tempfile
from pathlib import Path

from PIL import Image, ImageChops


def run(args):
    result = subprocess.run([str(arg) for arg in args], capture_output=True, text=True, timeout=900)
    if result.returncode:
        raise RuntimeError(f"{args[0]} failed: {result.stderr[-3000:]}")
    return result.stdout.strip()


def sha256(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def check_equal(actual, expected, description):
    if actual.size != expected.size or ImageChops.difference(actual.convert("RGB"), expected.convert("RGB")).getbbox():
        raise RuntimeError(f"Pixel mismatch: {description}")


def verify_webp(path, capture, directory):
    with Image.open(path) as animation:
        if animation.info.get("loop") != 0:
            raise RuntimeError("WebP infinite loop was not preserved")
        # The encoder may combine identical consecutive PNGs. Compare every source
        # interval against every overlapping decoded interval, including timing.
        encoded_index, remaining, duration = -1, 0, 0
        for index, frame in enumerate(capture["frames"]):
            source_remaining = frame["duration_ms"]
            with Image.open(directory / frame["file"]) as original:
                while source_remaining:
                    if not remaining:
                        encoded_index += 1
                        if encoded_index >= animation.n_frames:
                            raise RuntimeError("WebP ended before the source timeline")
                        animation.seek(encoded_index)
                        animation.load()
                        remaining = animation.info["duration"]
                        if remaining <= 0:
                            raise RuntimeError("Invalid WebP frame duration")
                    check_equal(animation, original, f"WebP interval versus source PNG {index}")
                    overlap = min(remaining, source_remaining)
                    remaining -= overlap
                    source_remaining -= overlap
                    duration += overlap
        if remaining or encoded_index + 1 != animation.n_frames:
            raise RuntimeError("WebP extends beyond the source timeline")
        return duration, animation.n_frames


def verify_gif_optimization(before, after):
    with Image.open(before) as reference, Image.open(after) as optimized:
        if reference.n_frames != optimized.n_frames or optimized.info.get("loop") != 0:
            raise RuntimeError("GIF optimizer changed frame count or looping")
        for index in range(reference.n_frames):
            reference.seek(index)
            optimized.seek(index)
            if reference.info.get("duration") != optimized.info.get("duration"):
                raise RuntimeError(f"GIF optimizer changed frame {index} timing")
            check_equal(optimized, reference, f"GIF optimized frame {index}")
        return optimized.n_frames


def encode(directory, output, max_mib):
    manifest_path = directory / "capture.json"
    capture = json.loads(manifest_path.read_text())
    name = capture["scene"]
    if capture["schema"] != 1 or capture["source"]["kind"] != "live-production" or capture["blocked_requests"]:
        raise ValueError("Expected a completed, read-only production capture")
    if name not in {"fleet-console", "git-flow", "fleet-atlas"} or not capture["frames"]:
        raise ValueError("Unknown or empty scene")
    paths = []
    for frame in capture["frames"]:
        path = directory / frame["file"]
        if path.parent != directory or not frame["file"].endswith(".png") or sha256(path) != frame["sha256"]:
            raise ValueError("Source frame path/hash mismatch")
        if not 20 <= frame["duration_ms"] <= 10000 or frame["duration_ms"] % 10:
            raise ValueError("Frame durations must be 20–10000 ms, in 10 ms increments")
        with Image.open(path) as image:
            if image.size != (capture["viewport"]["pixel_width"], capture["viewport"]["pixel_height"]):
                raise ValueError("Source frame dimensions changed")
        paths.append(path)

    output.mkdir(parents=True, exist_ok=True)
    # Work outside the published directory. Publish only after all verification passes.
    with tempfile.TemporaryDirectory(prefix="admiralty-encode-") as work:
        work = Path(work)
        palette_path = work / "palette.png"
        print(f"{name}: building a palette from every original RGB frame", flush=True)
        # No video intermediate, YUV conversion, scaling, saturation adjustment or lossy pre-pass.
        run(["ffmpeg", "-hide_banner", "-loglevel", "error", "-y", "-pattern_type", "glob",
             "-i", directory / "*.png", "-vf", "palettegen=max_colors=256:reserve_transparent=0:stats_mode=full",
             "-frames:v", "1", "-update", "1", palette_path])
        palette = Image.new("P", (1, 1))
        with Image.open(palette_path) as image:
            palette.putpalette(list(image.convert("RGB").tobytes()))
        quantized, durations = [], []
        for path, frame in zip(paths, capture["frames"]):
            with Image.open(path) as image:
                # Global palette avoids temporal color flicker. No dithering noise around UI text.
                quantized.append(image.convert("RGB").quantize(palette=palette, dither=Image.Dither.NONE))
            durations.append(frame["duration_ms"])
        raw_gif, final_gif, webp = work / "raw.gif", work / f"{name}.gif", work / f"{name}.webp"
        quantized[0].save(raw_gif, save_all=True, append_images=quantized[1:], duration=durations,
                          loop=0, disposal=1, optimize=False)
        for image in quantized:
            image.close()
        run(["gifsicle", "--careful", "--optimize=3", "--no-comments", "--no-names", "--no-interlace",
             raw_gif, "-o", final_gif])
        gif_frames = verify_gif_optimization(raw_gif, final_gif)
        print(f"{name}: GIF optimization preserves every decoded palette pixel; encoding full-color WebP", flush=True)
        # Encode directly from PNGs, never from the already quantized GIF.
        command = ["img2webp", "-min_size", "-loop", "0", "-lossless", "-m", "6", "-q", "100", "-exact"]
        for path, duration in zip(paths, durations):
            command += ["-d", duration, path]
        run(command + ["-o", webp])
        duration, webp_frames = verify_webp(webp, capture, directory)
        for artifact in (final_gif, webp):
            if artifact.stat().st_size > max_mib * 1024 * 1024:
                raise RuntimeError(f"{artifact.name} exceeds {max_mib} MiB web budget; shorten the scene or reduce motion, then recapture")
        poster = work / f"{name}.png"
        shutil.copyfile(paths[0], poster)
        artifacts = {artifact.name: {"bytes": artifact.stat().st_size, "sha256": sha256(artifact)}
                     for artifact in (final_gif, webp, poster)}
        receipt = {
            "schema": 1, "scene": name, "captured_utc": capture["captured_utc"],
            "source": capture["source"], "viewport": capture["viewport"], "editing": capture["editing"],
            "focus_project": capture.get("focus_project"),
            "steps": capture["steps"], "duration_ms": duration, "source_frames": len(paths),
            "gif_frames": gif_frames, "webp_frames": webp_frames, "capture_manifest_sha256": sha256(manifest_path),
            "allowed_requests": capture["allowed_requests"], "blocked_requests": [],
            "encoding": {
                "webp": "Full-color RGB, lossless from original PNGs; every decoded frame is pixel-identical to its PNG; durations and infinite loop verified",
                "gif": "Global 256-color palette from all PNGs; quantization is lossy; gifsicle -O3 is pixel-lossless relative to that palette GIF; every decoded optimized frame verified",
                "resize": False, "color_profile": "Browser forced to sRGB; no YUV or video intermediate",
                "web_budget_mib_each": max_mib,
                "tools": {"ffmpeg": run(["ffmpeg", "-version"]).splitlines()[0],
                          "gifsicle": run(["gifsicle", "--version"]).splitlines()[0],
                          "img2webp": run(["img2webp", "-version"]), "pillow": Image.__version__},
            }, "artifacts": artifacts,
        }
        for artifact in (final_gif, webp, poster):
            shutil.copyfile(artifact, output / artifact.name)
        (output / f"{name}.json").write_text(json.dumps(receipt, indent=2) + "\n")
        print(f"{name}: {duration / 1000:.2f}s, GIF {final_gif.stat().st_size / 1048576:.2f} MiB, lossless WebP {webp.stat().st_size / 1048576:.2f} MiB", flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("capture", type=Path, help="Run directory containing scene folders (or a single scene folder)")
    parser.add_argument("--out", type=Path, default=Path("docs/demos"))
    parser.add_argument("--max-mib", type=float, default=15)
    args = parser.parse_args()
    for tool in ("ffmpeg", "gifsicle", "img2webp"):
        if not shutil.which(tool):
            parser.error(f"Missing {tool}; see docs/operators/demo-capture.md")
    if not 0 < args.max_mib <= 50:
        parser.error("--max-mib must be positive and at most 50")
    directory = args.capture.resolve()
    scenes = [directory] if (directory / "capture.json").exists() else sorted(p.parent for p in directory.glob("*/capture.json"))
    if not scenes:
        parser.error("No completed capture.json files found")
    for scene in scenes:
        encode(scene, args.out.resolve(), args.max_mib)


if __name__ == "__main__":
    main()
