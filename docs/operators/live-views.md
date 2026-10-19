# Git Flow and Fleet Atlas

Open **Git Flow** or **Fleet Atlas** in the dashboard navigation. Direct routes are `#/git` and `#/atlas`. Existing detail, workplans, commands and models remain available. Both views are read-only and use the same in-memory session token as native API reads.

## Connect real sources

Run the native Rust observer with your existing private configuration:

```sh
admiralty-observe --config /absolute/private/observe.json
```

Every configured `projects[].repository` contributes a bounded Git observation to `status.json`. The observer publishes a project catalog even if a tracker has no tasks. Point `admiraltyd --legacy-status-file` and `--legacy-index-file` at the existing observer output and retained detailed HTML as described in [native startup](native-startup.md). The `pnpm live` launcher already supplies these source arguments. Rebuild/restart the observer and server to load this implementation; opening the browser alone cannot add Git data to an older observer.

New view reads always require the owner session token on a private server. `--legacy-public-read` does not grant access to `/v2/views` or `/v2/views/events`. Tokens travel in authorization headers, never URLs or persistent browser storage. `pnpm demo` displays an isolated, labeled synthetic topology and graph; it does not scan real repositories.

The server shares one source reader across viewers. It checks published producer state and native task state every two seconds and publishes a complete new cut only when source content/cursor changes. The browser receives authenticated SSE, with 15-second keepalives and bounded reconnect backoff. Reconnecting atomically replaces the current projection. This is current-state resynchronization, not a retained event journal or arbitrary history replay. Observation latency includes the configured observer cycle (default 15 seconds), collection time, and up to two seconds of server ingestion.

## Git Flow

Projects stack vertically. One horizontal scroller moves through their independent ancestry coordinates. **Parents always precede children**; horizontal alignment across repositories is not a shared clock. Selecting a mark or checkout opens its evidence inspector. Use **Inventory** for keyboard access to every loaded commit and checkout.

* Filled dots are exact commit objects; solid curves are actual ordered parent edges.
* Branch/tag badges attach to verified commit targets. Noncommit refs remain in the source contract and are not drawn as fake commits.
* Diamonds attach checkout HEADs. Detached, unborn, main and linked checkouts keep their own meanings. Dirty state is a separate hollow attachment.
* Dotted stubs indicate missing parents outside the current window, shallow history or unavailable objects. A shallow boundary is never presented as a proven root.
* **Pause** freezes the current observed snapshot. New source cuts accumulate behind **Return to latest**. **Follow latest** follows the current frontier; manual horizontal navigation turns it off.
* Commit selection exposes full OID, parents, committer time, current refs and source age. Checkout selection exposes administrative lock/prunable state and status coverage. A branch assignment link is source-reported association, not proof that an agent authored a commit.

Current collection bounds: 400 commits, 256 refs and 64 checkouts per configured repository; 2 MiB per Git command; three seconds per command; twelve seconds per repository; four concurrent repository scans; twenty seconds for the Git phase. Time-limited working-state scans may leave individual checkouts unknown. Truncation is labeled and every known missing parent keeps a boundary stub. The stream can reduce commit windows to 64 if its eight-MiB limit requires it; it does not silently discard agent identities. Excess inventory produces an explicit source capacity error.

Git runs with fixed arguments, no optional index locks, hooks, fsmonitor, lazy fetch, replacement objects or terminal prompts. It never fetches, checks out, commits, prunes or writes refs. A repository with configured clean filters has unknown working state because `status` can otherwise invoke external filter commands. Untracked directories count as entries, submodule contents are excluded, and unknown status is not zero. Public metadata includes checkout basenames and opaque path-derived identities, not absolute source paths, author emails, commit bodies or diffs. Unrepresentable UTF-8 metadata fails visibly rather than aliasing checkout identities. All Git transports are denied during observation. Original parents come from bounded raw Git records, including shallow commit boundaries; see [Git log's raw format](https://git-scm.com/docs/git-log#_pretty_formats).

## Fleet Atlas

The command spine contains Owner, Grokbot/watcher and Admiral. Project boards contain Commodores (project captains), observed workers and separately labeled native task assignments. Unassigned observations get their own board. Solid edges show configured supervision; the watcher's dashed edge is advisory. Neither edge grants command authority.

**Observed agents** counts reported process/session observations. A configured manager with no process evidence remains **Presence unknown**. Native task ownership becomes a **Task assignment**, not a fabricated running process. Exact reported model, requested model, model source and model observation time remain separate. A fresh source snapshot does not refresh a cached model timestamp. Screen names never determine models. Grokbot stays unobserved until the feed actually contains a matching observation.

If multiple projects claim the same full process/session identity, the observation stays in **Unassigned** with a project-binding source notice. Task and branch associations are unknown until the source conflict is resolved. Shared captain identities do not establish an observed captain for either project; separate configured roles keep presence unknown. Check overlapping worker prefixes or explicit lane/manager mappings in the observer configuration.

Drag background or scroll to pan. Use zoom controls or Ctrl/Command + wheel to zoom around the pointer. **Fit fleet** shows the whole map; project titles and minimap groups focus a project. Search matches all loaded identities, including offscreen agents, task IDs, models and nodes. Enter focuses the first match. Filters dim map entries and restrict the roster. Slots stay stable across ordinary activity changes and new arrivals. Selection can be shared through an opaque URL and browser Back/Forward works.

**Roster** exposes all identities with progressive rows and complete search. Narrow screens default to the roster. On the map, `+`/`-` zoom, `0` fits, arrow keys pan and `O` opens the roster. Focusable cards and roster rows open the same inspector. Reduced motion removes transitions. Fullscreen hides application chrome; if browser fullscreen is denied, application fullscreen remains usable. Escape or **Exit** leaves fullscreen.

## Source truth and supported scope

The connection pill describes transport connectivity. Source coverage in the bottom strip describes collector health, timestamps and missing inventory. Facts older than three minutes are labeled stale. An unavailable host inventory retains last-known agents and original timestamps; it does not infer an agent exit. A malformed/missing producer feed retains prior observations with an explicit error. A complete later inventory may reconcile absence.

All configured local projects and discoverable supplied screen observations are represented within bounded stream capacity. Remote repository DAGs require a separately configured local source; the observer does not fetch remote mirrors. Unconfigured hosts, opaque provider child sessions, native runtime inventory not exposed by the task projection, historical branch movements and arbitrary fleet replay are explicitly unqualified. A commit, running process, requested model, green check or delivery claim does not establish trusted task acceptance.

The implementation plan and deferred capability packets are in [LIVE_VIEWS_PLAN.md](../plans/admiralty-v2/LIVE_VIEWS_PLAN.md). Qualification evidence distinguishes real temporary Git/server runs, synthetic scale fixtures, browser accessibility audits and production rollout. Large-fleet frame-time targets and a production soak remain measured gates; they are not inferred from a passing build.
