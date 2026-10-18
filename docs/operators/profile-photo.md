# Profile photo

Open **Your profile** in the top-right corner of the dashboard. Choose a PNG,
JPEG or WebP image, review the square preview, and select **Save profile**. The
photo appears in the owner hierarchy card and the profile menu. **Remove photo**
restores the owner’s initial. **Cancel** discards the draft.

Images must be at most5 MB and20 megapixels. The browser crops the center to a
square and re-encodes a256×256 PNG; it keeps neither the original file nor its
metadata. Only the resized image and optional display name are saved in this
browser’s site storage. No photo is sent to the server, coding agents or model
providers. Private session tokens remain separate and are not stored here.

Profiles are local to the browser and origin, so `localhost` and `127.0.0.1` have
separate preferences. Shared account profiles and cross-device photo sync are
not implemented. The display name/photo are cosmetic preferences; they cannot
change the observed fleet owner or grant authority.

The Grokbot hierarchy mark is a separate1254×1254 RGBA PNG at
`apps/dashboard/public/brand/grokbot.png`, displayed at44 CSS pixels. Its source
and generation prompt are recorded in `docs/design/grokbot-asset.md`.
