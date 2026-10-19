# Dashboard icon assets

The UI displays these marks at29–46CSS pixels. Both derivatives are192x192,
covering at least4x display density without downloading1254px source images.
Original art stays in `../../public/brand/`; the source images must remain intact.
Vite imports produce content-hashed URLs for immutable caching.

Rebuild from the repository root with ImageMagick7 and libwebp1.6:

```sh
scratch="$(mktemp -d /tmp/admiralty-brand.XXXXXX)"
magick apps/dashboard/public/brand/admiralty-source.png -resize 192x192 "$scratch/admiralty.png"
magick apps/dashboard/public/brand/grokbot.png -resize 192x192 "$scratch/grokbot.png"
cwebp -lossless -exact -q 100 -m 6 -metadata icc "$scratch/admiralty.png" -o apps/dashboard/src/assets/admiralty-mark.webp
cwebp -lossless -exact -q 100 -m 6 -metadata icc "$scratch/grokbot.png" -o apps/dashboard/src/assets/grokbot-mark.webp
```

Lossless describes the WebP encoding of the
resized image; it does not claim that resizing retains every original pixel.
Decode each output with `dwebp`, then compare against its resized PNG with
`magick compare -metric AE reference.png decoded.png null:`; the result must be0.
Both source outputs were independently checked this way.

| Asset | Source bytes | UI bytes | UI SHA256 |
| --- | ---: | ---: | --- |
| Admiralty |909933|27820|`120aa58989e2d86ff33bd2befa31c0cfeb6b290ca870aed30d197db04125654a`|
| Grokbot |960632|14682|`dd4dc49a6ba573e565114953c3c92738c9003d0513788c4dd8abd6bc06bea051`|

Keep each icon under32KiB. The actual browser journey enforces this transfer
budget, valid versioned image paths and absence of source-PNG requests.
