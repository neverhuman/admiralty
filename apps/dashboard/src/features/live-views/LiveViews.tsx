import { useEffect, useRef, useState } from "react";
import { useRoute, routeHref } from "../../router";
import {
  useViews,
  age,
  isStale,
  type ViewAgent,
  type ViewProject,
  type GitCommit,
  type GitCheckout,
  type ViewsSnapshot,
} from "./feed";
import { ModelBadge } from "../../ui/ModelBadge";
import FleetAtlas from "./FleetAtlas";
import GitFlow from "./GitFlow";
import "./live-views.css";
import type { SlotRegistry } from "./geometry";

export type Selection =
  | { kind: "agent"; agent: ViewAgent }
  | { kind: "commit"; project: ViewProject; commit: GitCommit }
  | { kind: "checkout"; project: ViewProject; checkout: GitCheckout };
export function resolveSelection(
  snapshot: ViewsSnapshot,
  params: URLSearchParams,
): Selection | null {
  const id = params.get("focus"),
    project = snapshot.projects.find((p) => p.id === params.get("project"));
  const agent = snapshot.agents.find((a) => a.id === id);
  if (agent) return { kind: "agent", agent };
  const commit = project?.git?.commits.find((c) => c.oid === id);
  if (project && commit) return { kind: "commit", project, commit };
  const checkout = project?.git?.checkouts.find((c) => c.id === id);
  if (project && checkout) return { kind: "checkout", project, checkout };
  return null;
}
export function useSize<T extends HTMLElement>() {
  const [element, ref] = useState<T | null>(null);
  const [size, setSize] = useState({ width: 0, height: 0 });
  useEffect(() => {
    if (!element) return;
    const observer = new ResizeObserver(() =>
      setSize({ width: element.clientWidth, height: element.clientHeight }),
    );
    observer.observe(element);
    setSize({ width: element.clientWidth, height: element.clientHeight });
    return () => observer.disconnect();
  }, [element]);
  return { ref, element, ...size };
}
export default function LiveViews({
  mode,
  token,
  onAccess,
}: {
  mode: "git" | "atlas";
  token?: string;
  onAccess: () => void;
}) {
  const feed = useViews(token),
    route = useRoute(),
    root = useRef<HTMLDivElement>(null),
    slots = useRef<SlotRegistry>(new Map());
  useEffect(() => {
    slots.current.clear();
  }, [token]);
  const [pausedGit, setPausedGit] = useState<ViewsSnapshot | null>(null);
  useEffect(() => {
    setPausedGit(null);
  }, [token]);
  const [fullscreen, setFullscreen] = useState(false),
    [now, setNow] = useState(Date.now());
  useEffect(() => {
    const timer = setInterval(() => setNow(Date.now()), 10_000);
    return () => clearInterval(timer);
  }, []);
  useEffect(() => {
    const key = (e: KeyboardEvent) => {
      if (e.key === "Escape") setFullscreen(false);
    };
    window.addEventListener("keydown", key);
    return () => window.removeEventListener("keydown", key);
  }, []);
  useEffect(() => {
    const sync = () => {
      if (!document.fullscreenElement) setFullscreen(false);
    };
    document.addEventListener("fullscreenchange", sync);
    return () => document.removeEventListener("fullscreenchange", sync);
  }, []);
  const snapshot = feed.snapshot,
    presented = mode === "git" ? (pausedGit ?? snapshot) : snapshot,
    selection = presented ? resolveSelection(presented, route.params) : null;
  const sources = snapshot?.sources ?? [],
    gaps = sources.filter(
      (s) =>
        ["error", "unavailable", "not_configured", "conflict"].includes(
          s.state,
        ) || isStale(s.observed_utc, now),
    );
  const select = (value: Selection | null) => {
    const params = new URLSearchParams(window.location.hash.split("?")[1]);
    if (value) {
      if (
        mode === "git" &&
        !params.has("focus") &&
        params.get("project") &&
        !params.has("scope")
      )
        params.set("scope", params.get("project")!);
      params.set(
        "focus",
        value.kind === "agent"
          ? value.agent.id
          : value.kind === "commit"
            ? value.commit.oid
            : value.checkout.id,
      );
      const project =
        value.kind === "agent" ? value.agent.project_id : value.project.id;
      if (project) params.set("project", project);
      else params.delete("project");
    } else {
      params.delete("focus");
      if (mode === "git" && !params.has("scope")) params.delete("project");
    }
    window.location.hash = routeHref(mode, Object.fromEntries(params));
  };
  const toggleFullscreen = () => {
    if (fullscreen) {
      setFullscreen(false);
      if (document.fullscreenElement)
        void document.exitFullscreen().catch(() => undefined);
    } else {
      setFullscreen(true);
      void root.current?.requestFullscreen?.().catch(() => undefined);
    }
  };
  return (
    <div
      ref={root}
      className={`live-workspace ${fullscreen ? "is-fullscreen" : ""}`}
    >
      <header className="lv-header">
        <div className="lv-title">
          <span
            className="lv-glyph"
            aria-hidden="true"
            data-icon={mode === "atlas" ? "✦" : "⑂"}
          />
          <div>
            <div className="lv-overline">ADMIRALTY / OBSERVATORY</div>
            <h1>{mode === "atlas" ? "Fleet Atlas" : "Git Flow"}</h1>
          </div>
          <span className="lv-readonly">READ ONLY</span>
        </div>
        <div className="lv-header-actions">
          <span
            className={`lv-feed ${feed.state === "connected" ? "is-connected" : ""}`}
            role="status"
          >
            <i />
            {feed.publicFallback
              ? "Public producer view · access needed for native assignments"
              : feed.sessionAccessRequired
                ? "Session access required"
                : feed.state === "connected"
              ? "Connected"
              : feed.state === "paused"
                ? "Access / retry needed"
                : feed.state === "reconnecting"
                  ? "Reconnecting"
                  : "Connecting"}
          </span>
          <button
            className="lv-button"
            onClick={toggleFullscreen}
            aria-label={fullscreen ? "Exit fullscreen" : "Enter fullscreen"}
          >
            {fullscreen ? "↙ Exit" : "⛶ Fullscreen"}
          </button>
        </div>
      </header>
      {snapshot?.simulated && (
        <div className="lv-demo">
          SIMULATION · Every agent, repository and relationship in this
          demonstration is synthetic.
        </div>
      )}
      {feed.publicFallback && (
        <div className="lv-demo" role="status">
          <span>Showing the explicitly public producer snapshot. Private native assignments are unavailable; source timestamps are preserved.</span>
          {feed.error && <span>{feed.error}. Last public source timestamps are unchanged.</span>}
          <button onClick={onAccess} className="lv-button">
            Connect session access
          </button>
          <button onClick={feed.retry} className="lv-button">
            Retry private feed
          </button>
        </div>
      )}
      {feed.error && !feed.publicFallback && (
        <div className="lv-error" role="status">
          <span>{feed.error}. Source times preserved.</span>
          <button onClick={onAccess} className="lv-button">
            {feed.sessionAccessRequired ? "Connect session access" : "Session access"}
          </button>
          <button onClick={feed.retry} className="lv-button">
            {feed.sessionAccessRequired ? "Retry private feed" : "Retry feed"}
          </button>
        </div>
      )}
      {!snapshot && (
        <div className="lv-empty">
          <span aria-hidden="true">{feed.error ? "⌁" : "◌"}</span>
          <h2>
            {feed.error
              ? "Observation feed unavailable"
              : "Connecting to observations"}
          </h2>
          <p>
            {feed.error
              ? "Enter session access or retry when the native service is ready."
              : "Loading the complete project and agent inventory…"}
          </p>
        </div>
      )}
      {snapshot && (
        <div className={`lv-body ${selection ? "has-inspector" : ""}`}>
          {mode === "atlas" ? (
            <FleetAtlas
              slots={slots.current}
              snapshot={snapshot}
              selection={selection}
              onSelect={select}
              now={now}
            />
          ) : (
            <GitFlow
              snapshot={snapshot}
              paused={pausedGit}
              onPause={setPausedGit}
              selection={selection}
              onSelect={select}
              now={now}
            />
          )}{" "}
          {selection && (
            <Inspector
              selection={selection}
              snapshot={presented!}
              onClose={() => select(null)}
              now={now}
            />
          )}
        </div>
      )}
      <footer className="lv-status">
        <span>
          <i className={`lv-status-dot ${gaps.length ? "warn" : ""}`} />
          {snapshot
            ? `${snapshot.projects.length} projects · ${snapshot.agents.filter((a) => a.presence === "observed" && a.role !== "owner").length} observed agent identities`
            : "Awaiting first source cut"}
        </span>
        <details>
          <summary>
            {gaps.length
              ? `${gaps.length} source coverage notices`
              : "Source coverage & provenance"}
          </summary>
          <div className="lv-source-popover">
            {sources.map((s) => (
              <article key={s.id}>
                <b>{s.label}</b>
                <span>
                  {s.state} · {age(s.observed_utc, now)}
                </span>
                <p>{s.coverage}</p>
              </article>
            ))}
          </div>
        </details>
        <span>Observation ≠ task acceptance</span>
      </footer>
    </div>
  );
}
function Inspector({
  selection,
  snapshot,
  onClose,
  now,
}: {
  selection: Selection;
  snapshot: ViewsSnapshot;
  onClose: () => void;
  now: number;
}) {
  const ref = useRef<HTMLElement>(null);
  const identity =
    selection.kind === "agent"
      ? selection.agent.id
      : selection.kind === "commit"
        ? selection.commit.oid
        : selection.checkout.id;
  useEffect(() => {
    ref.current?.focus({ preventScroll: true });
  }, [identity]);
  let title = "",
    subtitle = "",
    fields: Record<string, string> = {},
    related: ViewAgent[] = [];
  if (selection.kind === "agent") {
    const a = selection.agent;
    title = a.name;
    subtitle = a.role === "commodore" ? "Commodore / project captain" : a.role;
    fields = {
      Presence:
        a.presence === "observed"
          ? "Observed process / session"
          : a.presence === "ended"
            ? "Reported ended"
            : "Presence unobserved",
      "Reported activity": a.state,
      Project: a.project_id ?? "Unassigned",
      Node: a.node ?? "Unknown",
      "Reported model": a.model ?? "Unknown",
      "Requested model": a.requested_model ?? "Unknown",
      "Model evidence": a.model_source,
      "Model observation time": a.model_observed_utc
        ? `${a.model_observed_utc} · ${age(a.model_observed_utc, now)}`
        : "Unknown; separate from source snapshot time",
      Effort: a.effort ?? "Unknown",
      Task: a.task ?? "Not bound",
      Branch: a.branch ?? "Not bound",
      Observed: a.observed_utc
        ? `${a.observed_utc} · ${age(a.observed_utc, now)}`
        : "Time unknown",
      Source: a.source,
      Identity: a.id,
    };
  } else if (selection.kind === "commit") {
    const c = selection.commit,
      p = selection.project;
    title = c.subject;
    subtitle = "Exact Git commit";
    fields = {
      Commit: c.oid,
      Parents:
        c.parents.join("\n") ||
        "No recorded parent (may be a shallow boundary)",
      "Committer time": c.committed_utc,
      Project: p.name,
      "Repository instance": p.git?.instance_id ?? "Unknown",
      Node: p.git?.node ?? "Unknown",
      Refs:
        p.git?.refs
          .filter((r) => r.commit_oid === c.oid)
          .map((r) => r.name)
          .join("\n") || "No current ref",
      Observed: `${p.git?.observed_utc} · ${age(p.git?.observed_utc, now)}`,
      Acceptance: "No task acceptance implied by this commit",
    };
  } else {
    const c = selection.checkout,
      p = selection.project;
    title = c.name;
    subtitle = `${c.kind} checkout`;
    fields = {
      Branch: c.branch ?? (c.detached ? "Detached HEAD" : "Unborn / unknown"),
      HEAD: c.head ?? "No observed commit",
      Node: p.git?.node ?? "Unknown",
      "Administrative lock": c.locked ? "Locked" : "Not reported locked",
      Prunable: c.prunable ? "Yes" : "No",
      "Working state": c.status
        ? `${c.status.staged} staged · ${c.status.unstaged} unstaged · ${c.status.untracked} untracked entries · ${c.status.conflicts} conflicts`
        : "Unknown; status scan unavailable or filtered",
      "Status scope":
        "Untracked directories summarized; submodule contents excluded",
      Observed: `${p.git?.observed_utc} · ${age(p.git?.observed_utc, now)}`,
      "Checkout identity": c.id,
    };
    related = snapshot.agents.filter(
      (a) => a.project_id === p.id && a.branch && a.branch === c.branch,
    );
  }
  return (
    <aside
      className="lv-inspector"
      ref={ref}
      tabIndex={-1}
      aria-label="Observation inspector"
      onKeyDown={(e) => {
        if (e.key === "Escape") onClose();
      }}
    >
      <header>
        <div className="lv-overline">OBSERVATION INSPECTOR</div>
        <button
          className="lv-icon"
          onClick={onClose}
          aria-label="Close inspector"
        >
          <span aria-hidden="true" data-icon="×" />
        </button>
      </header>
      <span className="lv-inspector-kind">{subtitle}</span>
      <h2>{title}</h2>
      {selection.kind === "agent" && (
        <ModelBadge
          model={selection.agent.model ?? "Reported model unknown"}
          effort={selection.agent.effort ?? undefined}
        />
      )}
      <div className="lv-inspector-links">
        {selection.kind === "agent" ? (
          <>
            <a
              href={routeHref("git", {
                project: selection.agent.project_id,
                branch: selection.agent.branch,
              })}
            >
              Open project in Git Flow ↗
            </a>
            {selection.agent.task && (
              <a href={routeHref("workplans")}>Open native workplans ↗</a>
            )}
          </>
        ) : (
          <a href={routeHref("atlas", { project: selection.project.id })}>
            Open project in Fleet Atlas ↗
          </a>
        )}
        <a href={routeHref("detail")}>Open detailed evidence ↗</a>
      </div>
      <dl>
        {Object.entries(fields).map(([key, value]) => (
          <div key={key}>
            <dt>{key}</dt>
            <dd>{value}</dd>
          </div>
        ))}
      </dl>
      {related.length > 0 && (
        <section>
          <h3>Reported branch assignments</h3>
          {related.map((a) => (
            <a
              key={a.id}
              href={routeHref("atlas", { project: a.project_id, focus: a.id })}
            >
              {a.name} ↗
            </a>
          ))}
        </section>
      )}
      <p className="lv-truth-note">
        Facts describe their source at the recorded time. A configured role,
        running process, commit or green check does not establish accepted work.
      </p>
    </aside>
  );
}
