# Interface typefaces

Self-hosted so the console renders identically offline and inside the single
`admiraltyd` binary (no font CDN, no network request at runtime).

| File | Family | Source |
| --- | --- | --- |
| `atkinson-hyperlegible-next-latin-wght-normal.woff2` | Atkinson Hyperlegible Next, variable weight 200–800, Latin subset | `@fontsource-variable/atkinson-hyperlegible-next@5.3.0` |
| `atkinson-hyperlegible-mono-latin-wght-normal.woff2` | Atkinson Hyperlegible Mono, variable weight 200–800, Latin subset | `@fontsource-variable/atkinson-hyperlegible-mono@5.3.0` |

Both are © The Atkinson Hyperlegible Next / Mono Project Authors (Braille
Institute) and licensed under the SIL Open Font License 1.1 (`OFL.txt`). The
files are copied byte-for-byte from the npm packages; nothing is subset or
modified here. Atkinson Hyperlegible was designed for low-vision readers: its
characters are deliberately distinct (0/O, 1/l/I, rn/m), which is the property
an operator console needs when reading commit hashes, counts and model names.

To refresh, `npm pack` the packages above and copy `files/*-latin-wght-normal.woff2`.
