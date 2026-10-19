# Production README captures

The README uses three short production walkthroughs: `fleet-console`, `git-flow`
and `fleet-atlas`. The capture reads the running deployment through its existing
HTTP loopback tunnel or HTTPS origin. It does not start a preview server, replace
producer data, or launch providers. The committed captures were taken from the
explicitly public producer view on 19 October 2026; the access banner is part of
the recording.

## Tools and setup

Use Node 24+, Chrome/Chromium, Python 3 with Pillow, FFmpeg, Gifsicle and the WebP
tools (`img2webp`). The encoder invokes the WebP tools directly; an FFmpeg build
without a WebP encoder is sufficient. No new JavaScript dependency is needed.

On macOS with Homebrew:

```sh
rtk proxy brew install ffmpeg gifsicle webp
rtk proxy python3 -m venv .capture/venv
rtk proxy .capture/venv/bin/python -m pip install -r scripts/demos/requirements.txt
```

On Debian/Ubuntu, the corresponding packages are `ffmpeg`, `gifsicle`, `webp`
and `python3-venv`. Chrome is detected at the standard macOS/Linux locations;
set `ADMIRALTY_CAPTURE_CHROME` to an executable path for another installation.
The browser uses its own temporary profile and never attaches to your normal
browser session.

## Record and encode

From the repository root, use a new output directory for each take:

```sh
rtk pnpm demo:capture \
  --url http://127.0.0.1:18890 \
  --project admiralty \
  --out .capture/production-20261019

rtk proxy .capture/venv/bin/python scripts/demos/encode.py \
  .capture/production-20261019 --out docs/demos

rtk proxy .capture/venv/bin/python scripts/demos/verify.py \
  --capture .capture/production-20261019
```

`pnpm demo:encode <capture-directory>` and `pnpm demo:verify` are shortcuts when
`python3` already has Pillow. Offline verification needs no production access.
For example, after setup:

```sh
rtk proxy env PATH="$PWD/.capture/venv/bin:$PATH" pnpm demo:verify
```

By default, 1440 × 900 CSS pixels at a 2× device scale produce native
**2880 × 1800** PNG frames. `--width`, `--height` and `--scale` change the capture
viewport; encoding never resizes it. `--scene fleet-console`, `--scene git-flow`
or `--scene fleet-atlas` records one view. `--project` chooses the console/Git
project. The Atlas selects the first small observed project for readable cards;
`--atlas-project joil` chooses it explicitly.

If the deployment requires authentication, add
`--token-file /absolute/private/dashboard-token`. The token is applied through
Session access before any frames are recorded and stays in page memory across
hash navigation. The file must be an owned private regular file, and native
view access must succeed before recording begins. Token values, raw API responses, browser profiles and network
bodies are not written to receipts. Keep the token file outside Git.

Preflight refuses feeds marked synthetic, missing Git observations, invalid
project selections or source timestamps older than three minutes. Investigate
stale data first; `--allow-stale` explicitly permits a stale capture and records
that choice in its receipt. An unflagged feed is not independent authentication
of its origin: use the operator's known production tunnel/origin and visually
review the result. The receipt records a preflight source digest/time; live
refreshes during the walkthrough retain their own on-screen timestamps.

Capture refuses existing scene directories so an interrupted take cannot mix
old and new frames. Use a fresh directory after a failure. Browser readiness
failures identify the missing target; check the live UI/selector before retrying.
The scripts require the deployed Git Flow and Fleet Atlas UI, not an older bundle.

## Color, compression and proof

Chrome renders in sRGB and saves PNGs directly, with scrollbars hidden. A cursor
and click ring are the only visual overlays. Holds and transitions use authored durations: these are
edited walkthroughs, not real-time screen recordings. Production facts, labels,
timestamps and error/access notices are never rewritten or hidden.

The lossless WebP is encoded from original PNGs with `img2webp -lossless -exact`,
without a video, YUV or GIF intermediate. The encoder then decodes every output
interval and compares its RGB pixels against the original PNG. Identical
consecutive frames may be merged; the complete duration and displayed pixels
must still match. Infinite looping is checked too. WebP's
[lossless format](https://developers.google.com/speed/webp/docs/webp_lossless_bitstream_specification)
preserves full-color image data.

GIF requires palette quantization and cannot preserve an arbitrary RGB screenshot
losslessly. The encoder builds one 256-color palette from all original frames
using [FFmpeg palettegen](https://ffmpeg.org/ffmpeg-filters.html#palettegen),
without saturation changes or dither noise around text. Gifsicle then applies
lossless `-O3` optimization. Every optimized GIF frame is decoded and compared
with the unoptimized palette GIF, including timing. This proves the optimizer
did not introduce additional loss; it does not call quantization lossless.

Both formats must remain below 15 MiB per animation. Over-budget encoding fails
without publishing new files. Shorten the scene or reduce motion and recapture;
the encoder never silently lowers resolution or changes to lossy WebP. Current
captures are substantially below that ceiling. Each successful scene publishes:

| File | Purpose |
| --- | --- |
| `docs/demos/<scene>.webp` | Full-color lossless animation, preferred by the README. |
| `docs/demos/<scene>.gif` | Optimized GIF fallback. |
| `docs/demos/<scene>.png` | Original first frame for static/reduced-motion viewing. |
| `docs/demos/<scene>.json` | Source provenance, scene steps, dimensions, timing, tool versions, hashes and verification claims. |

The README uses GitHub-supported
[`picture` markup](https://docs.github.com/en/get-started/writing-on-github/getting-started-with-writing-and-formatting-on-github/basic-writing-and-formatting-syntax#images)
with a reduced-motion PNG, lossless WebP source and embedded GIF fallback.
All formats also have direct links.

Original PNG frames and `capture.json` remain in ignored `.capture/`. Preserve
the selected take if later pixel-proof reruns are needed. `demo:verify` always
checks committed hashes, dimensions, timing, looping, size budgets and README
references; add `--capture` to repeat proof against retained PNGs. That option
can be repeated when scenes came from different takes.

## Review before committing

Inspect each animation at native size and README size. Check project selection,
graph ancestry, readable model badges, inspectors, source age, click targets and
the loop transition. Confirm the visible production data is intended for the
repository's audience. The scripts do not claim arbitrary screen text is
secret-free. Raw captures are ignored; only the reviewed animations, posters
and receipts belong in the repository. Update README descriptions/date if the
scenes change.

The disposable browser permits only same-origin GET/HEAD traffic; writes and
outside-origin requests fail before delivery, and capture fails if anything was
blocked. This boundary has a real-browser regression test against an isolated
local HTTP server:

```sh
rtk proxy node --test scripts/demos/browser.test.mjs
rtk proxy node --check scripts/demos/capture.mjs
rtk proxy node --check scripts/demos/browser.mjs
rtk git diff --check
```

No production mutations, authenticated provider turns or deployment changes are
needed to refresh these recordings.
