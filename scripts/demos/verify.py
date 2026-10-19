#!/usr/bin/env python3
"""Verify published demos offline, optionally repeating original-PNG pixel proof."""
import argparse
import json
import re
from pathlib import Path

from PIL import Image

from encode import sha256, verify_webp

SCENES = ("fleet-console", "git-flow", "fleet-atlas")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--dir", type=Path, default=Path("docs/demos"))
    parser.add_argument("--readme", type=Path, default=Path("README.md"))
    parser.add_argument("--capture", type=Path, action="append", default=[],
                        help="Original run or scene folder; repeat for multiple runs")
    args = parser.parse_args()
    captures = {}
    for root in args.capture:
        paths = [root / "capture.json"] if (root / "capture.json").exists() else root.glob("*/capture.json")
        for path in paths:
            captures[json.loads(path.read_text())["scene"]] = path
    readme = args.readme.read_text()
    for scene in SCENES:
        receipt = json.loads((args.dir / f"{scene}.json").read_text())
        if receipt["scene"] != scene or receipt["source"]["kind"] != "live-production" or receipt["blocked_requests"]:
            raise RuntimeError(f"Invalid production provenance: {scene}")
        size = (receipt["viewport"]["pixel_width"], receipt["viewport"]["pixel_height"])
        for extension in ("gif", "webp", "png"):
            name = f"{scene}.{extension}"
            path, artifact = args.dir / name, receipt["artifacts"][name]
            if path.stat().st_size != artifact["bytes"] or sha256(path) != artifact["sha256"]:
                raise RuntimeError(f"Artifact size/hash mismatch: {name}")
            with Image.open(path) as image:
                if image.size != size:
                    raise RuntimeError(f"Unexpected dimensions: {name}")
                if extension != "png":
                    if image.n_frames != receipt[f"{extension}_frames"] or image.info.get("loop") != 0:
                        raise RuntimeError(f"Animation frames/loop changed: {name}")
                    duration = 0
                    for index in range(image.n_frames):
                        image.seek(index)
                        image.load()
                        duration += image.info["duration"]
                    if duration != receipt["duration_ms"]:
                        raise RuntimeError(f"Animation duration changed: {name}")
                    if artifact["bytes"] > receipt["encoding"]["web_budget_mib_each"] * 1048576:
                        raise RuntimeError(f"Web size budget exceeded: {name}")
            if extension == "gif" and not re.search(rf'<img\b[^>]*src="docs/demos/{re.escape(name)}"', readme):
                raise RuntimeError(f"README is missing the GIF embed: {name}")
            if extension in ("webp", "png") and f"docs/demos/{name}" not in readme:
                raise RuntimeError(f"README is missing the full-color/static link: {name}")
        if scene in captures:
            path = captures[scene]
            if sha256(path) != receipt["capture_manifest_sha256"]:
                raise RuntimeError(f"Original capture manifest changed: {scene}")
            capture = json.loads(path.read_text())
            for frame in capture["frames"]:
                if sha256(path.parent / frame["file"]) != frame["sha256"]:
                    raise RuntimeError(f"Original source frame changed: {scene}/{frame['file']}")
            verify_webp(args.dir / f"{scene}.webp", capture, path.parent)
        print(f"{scene}: hashes, {size[0]}×{size[1]}, timing, loops, web budget, README links verified"
              + ("; original RGB pixels verified" if scene in captures else ""))


if __name__ == "__main__":
    main()
