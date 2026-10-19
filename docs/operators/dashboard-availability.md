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
