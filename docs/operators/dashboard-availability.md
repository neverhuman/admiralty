# Dashboard connection and availability

The live dashboard is a long-running Rust service on the production host's
loopback port18890. Local CI uses isolated ephemeral ports and rejects18890/18891
in its smoke harness. Building and running local CI should leave production
available; only the single deployment owner controls its cutover.

For a local browser, keep an authenticated SSH forwarding connection open:

```sh
ssh -N -o ExitOnForwardFailure=yes -o ServerAliveInterval=30 \
  -L 127.0.0.1:18890:127.0.0.1:18890 xbabe2
```

Use `http://127.0.0.1:18890/#/detail`. If18890 already has an owner, inspect it
before starting a second forward; do not terminate another user's session.
The hash route never reaches the HTTP server. Unknown routes currently fall
back to detail, so an accidental trailing `]` does not explain a transport stall.

On19October2026 at20:49UTC the server returned `/healthz`, `/` and the real feed
with status200 while the Mac's existing forward intermittently timed out. The
server delivered the feed in55ms; a separate owned diagnostic forward took
9.64s for1.58MB. The actual browser's first `/v1/legacy` transfer hit12,002.9ms
and aborted. Its retry completed in5,686.3ms and rendered real fleet data.
The deployment watcher remained on d858a9a; Root did not restart production or
the user's SSH process. This was unexpected connection degradation, unrelated
to a local build stopping the service.

The current client aborts each snapshot request after12seconds and schedules a
retry15seconds after completion. Initial loading can therefore last across a
failed attempt. A health response or status200 headers alone cannot verify that
the entire feed arrived and rendered. Verify a completed `/v1/legacy` request,
visible project data and advancing producer time in the actual browser.

An instrumented browser load at21:07UTC reproduced first-contentful-paint at
5.6seconds, followed by a12second snapshot abort and no populated project tabs
after32seconds. That failure is retained alongside successful retries.

The repair negotiates lossless gzip on snapshots and text assets, gives Vite's
versioned JavaScript/CSS immutable caching and replaces the generic abort text
with a clear timeout explanation. Compression runs in the blocking read task
under the shared read semaphore. Authentication, decompressed source bounds,
source age and the12second request deadline remain enforced. Identity clients
and explicit `gzip;q=0` remain supported. Local tests do not establish production
availability: repeat the rendered browser timing through the same localhost
connection after the sole Commodore's qualified cutover.

The first production fix eliminated the abort but still took5.24seconds through
the actual localhost tunnel: the shell painted at1.09seconds and the compressed
snapshot consumed3.99seconds. Local captured-data rendering took0.53–0.57seconds,
so local timings alone did not establish an acceptable production load.

The progressive detail reader uses `/v1/legacy/detail/initial` for current
project counts, flags/warnings, role/model observations and source timestamps.
History and full packet/project evidence are explicitly marked as loading,
then `/v1/legacy/detail` replaces the whole snapshot after enrichment. A later
source timestamp is retained as later, without mixing records between the two
responses. Enrichment failure keeps the initial facts and their original age;
normal retries remain sequential with their own12second deadline. Once complete,
manual refresh requests full detail directly and keeps the previous view.

The detail routes omit the separate Git graph, which this view does not use.
The original `/v1/legacy` and `/legacy/status.json` retain every source field for
Git Flow and other existing readers. Older/synthetic servers without the new
routes fall back once to the original endpoint. Both projections use the same
authentication and bounded source validation; they grant no native authority.

The next production timings disproved a simple old-tunnel diagnosis: a fresh
independent forward also showed slow page loads, while direct browser requests
without images returned in80–300ms. The page loaded two1254px source PNGs
(909,933 and960,632bytes) for29–46px icons. One controlled browser comparison
returned the initial snapshot in249ms without a logo transfer,1,947ms with that
transfer, then149ms without it. The large image and JSON channels share the same
SSH TCP connection; delayed first-byte timing alone did not isolate a bad tunnel.
The user's interactive SSH process remained untouched.

UI imports now use192px WebP derivatives, encoded losslessly after resizing.
They total42,502bytes (97.7% less), preserve alpha and match every decoded pixel
of the resized PNG references. Original1254px assets remain available. Vite emits
content-hashed image URLs; the server supplies `image/webp` and immutable caching,
including hashes with internal or trailing hyphens. Unversioned assets remain
`no-cache`. Brand marks have low fetch priority and asynchronous decoding;
watcher icons load lazily. The existing browser journey enforces a32KiB per-icon
budget and rejects source-PNG requests. See the [asset recipe](../../apps/dashboard/src/assets/README.md).
The [sanitized measurement receipt](../../receipts/integration/dashboard-image-contention-20261019.json) retains the overlap control and pixel checks.
Production before/after navigation timings remain a separate deployment check.

After the image cutover at3375621, actual Chrome155 on the original localhost
forward showed current facts in1,697ms while fetching the new JavaScript bundle,
then556ms and543ms on repeats. Full detail/history finished in2,761ms,1,934ms
and1,550ms respectively. Every run rendered six project tabs with no error;
repeat JavaScript, CSS and both image transfers were zero bytes. These are
three observed loads, not a percentile or a guarantee of network latency.

The independent full-detail comparison then caught nine one-ULP differences in
floating-point observations, up to1.42e-14, while keys/counts/timestamps matched.
The original full endpoint was byte-preserved; the projected reader used
serde_json's default fast float parsing. The workspace now enables its existing
`float_roundtrip` feature so parsing is correctly rounded before reserialization.
The regression compares actual source literals with Rust's independently rounded
`str::parse::<f64>()` bit pattern, including small values and signed zero, through
both projections and a serialized reopen. The original failure is retained in
[the precision receipt](../../receipts/integration/dashboard-decimal-roundtrip-before-20261019.json).
No approximate equality replaces the final production fidelity check. Existing
provider cost readers also share this feature, so the complete workspace/native
reopen/backup and browser matrix must pass before push; no schema or authority
changes are introduced.

At receiving main `b958736f36bfc656961c30af5df5499c6c1dd239`, deployed on `:18890`
by the sole deployment owner and independently verified through the existing user
SSH forward (`PID 4977`, Chrome 155), `/v1/legacy/detail/initial` compacts initial
card and table numbers while keeping `compact_initial: true` and
`complete_detail: false` explicit until the operator requests `/v1/legacy/detail`.
Same-generation initial values and on-demand complete detail match the original
producer fields with zero floating-point tolerance (`sha256: 99aec907e6b819b0859fa303615b7ec2f477c24c549f1b2aec341e8e169a0313`).
Initial current facts render in `281.3 / 539.9 / 492.5ms`, on-demand complete
detail loads in `1,841.3 / 1,826.0 / 1,683.1ms`, repeat loads transfer `0` network
bytes for cached JS/CSS/images, and lossless resized WebP brand icons remain
`42,502` bytes vs `1,870,565` bytes (`97.7%` smaller). Earlier tunnel timeout
failures and hosted `xbabe3` browser readiness variance remain retained as separate
operational evidence; this dashboard availability proof does not qualify general
native coding execution or full release acceptance beyond `P00/P01`.

At 02:37UTC on 20 October, the old local SSH process and its `:18890` listener
were absent while the remote service remained healthy. Restoring one loopback
forward made the detailed page available again; the actual browser rendered
fresh producer data, project tabs and all thirteen plots. One HTTP-only sample
over that forward measured 37.83ms for current facts and 61.48ms for full detail;
these are request timings, not new browser-render measurements or percentiles.
No production service or configuration changed. The local tunnel is an operator
connection and must be running separately from the source build. Preserve its
active control socket while using it; a successful build cannot keep an absent
SSH tunnel alive.
