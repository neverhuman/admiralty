import { useEffect, useLayoutEffect, useMemo, useRef, useState, type CSSProperties } from "react";
import { routeHref, useRoute } from "../../router";
import {
  age,
  isStale,
  type GitCheckout,
  type ViewProject,
  type ViewsSnapshot,
} from "./feed";
import { gitRangeWidth, layoutGit, type CommitRange, type GitLayout } from "./geometry";
import { useSize, type Selection } from "./LiveViews";
import { changeParams, palette } from "./FleetAtlas";
import GitChat from "./GitChat";
import { captureGitAnchor, restoreGitAnchor, fitGitZoom } from "./git-viewport";
import { createChatMemory } from "./operator-chat";
import { useGitHealth, type GitHealthReport } from "./gitHealth";

let viewMemory = { zoom: 0.9, left: 0, top: 0, follow: true, inventory: false };
const scopeViews = new Map<string, typeof viewMemory>();
const colors = [
  palette.gemini,
  palette.claude,
  palette.luna,
  palette.sol,
  palette.local,
  palette.astra,
];
function shortRef(ref: string) {
  return ref.replace(/^refs\/(heads|remotes|tags)\//, "");
}
function working(checkout: GitCheckout) {
  const s = checkout.status;
  return !s
    ? "working state unknown"
    : s.conflicts
      ? `${s.conflicts} conflicts`
      : s.staged + s.unstaged + s.untracked
        ? `${s.staged} staged · ${s.unstaged} unstaged · ${s.untracked} untracked entries`
        : "clean (submodules excluded)";
}
function GitHealthBadge({ report, error, now, onToggle }: { report?: GitHealthReport; error: string; now: number; onToggle: (open: boolean) => void }) {
  const stale = !!report && isStale(report.observed_utc, now);
  const timeUncertain = !!report && Date.parse(report.observed_utc) > now + 30_000;
  const state = stale || timeUncertain || error ? "unknown" : report?.state ?? "unknown";
  const amount = (value: GitHealthReport["branches"][number]["ahead"]) => value ? `${value.lower_bound ? "≥" : ""}${value.value}` : "unknown";
  return <details className={`git-health git-health-${state}`} onToggle={(event) => onToggle(event.currentTarget.open)}>
    <summary>Git health · {stale ? "stale" : state}{report?.issues.length ? ` · ${report.issues.length}` : ""}</summary>
    <div className="git-health-detail">
      {!report ? <p>{error || "Health report not observed for this project."}</p> : <>
        {error && <p role="status">Health coverage / notification: {error}</p>}
        <p>{age(report.observed_utc, now)} · Integration: <code>{report.integration_ref ?? "not configured"}</code></p>
        <p>{report.observed_commits} commits · {report.observed_refs} refs · {report.observed_checkouts} checkouts observed.</p>
        {(report.shallow || report.truncated || report.missing_parent_oids.length > 0 || report.observation_error) && <p>Coverage: {[
          report.shallow ? "shallow ancestry" : "", report.truncated ? "bounded window" : "", report.missing_parent_oids.length ? `${report.missing_parent_oids.length} missing parents` : "", report.observation_error ? "source error" : "",
        ].filter(Boolean).join(" · ")}. Unknown facts cannot establish healthy Git.</p>}
        {report.issues.slice(0, 30).map((issue, index) => <article key={`${issue.code}:${index}`}><b>{issue.severity} · {issue.code}</b><p>{issue.detail}</p>{issue.ref_name && <code>{issue.ref_name}</code>}{issue.checkout_id && <code>{issue.checkout_id}</code>}</article>)}
        {report.issues.length > 30 && <p>Showing 30 of {report.issues.length} issues.</p>}
        {report.branches.slice(0, 50).map((branch) => <p key={branch.ref_name}><code>{branch.ref_name}</code><br />{branch.relation} · ahead {amount(branch.ahead)} / behind {amount(branch.behind)}</p>)}
        {report.branches.length > 50 && <p>Showing 50 of {report.branches.length} branches.</p>}
        <p>Uses cached observations. No Git writes or automatic reconciliation.</p>
      </>}
    </div>
  </details>;
}
function GitCanvas({
  project,
  layout,
  zoom,
  left,
  top,
  width,
  height,
  selection,
  onSelect,
  onToggleRange,
}: {
  project: ViewProject;
  layout: GitLayout;
  zoom: number;
  left: number;
  top: number;
  width: number;
  height: number;
  selection: Selection | null;
  onSelect: (s: Selection) => void;
  onToggleRange: (range: CommitRange) => void;
}) {
  const ref = useRef<HTMLCanvasElement>(null),
    [hover, setHover] = useState("");
  const selected =
    selection?.kind === "commit" && selection.project.id === project.id
      ? selection.commit.oid
      : selection?.kind === "checkout" && selection.project.id === project.id
        ? selection.checkout.head
        : null;
  useEffect(() => {
    const canvas = ref.current;
    if (!canvas) return;
    const ratio = Math.min(2, window.devicePixelRatio || 1);
    canvas.width = Math.ceil(width * ratio);
    canvas.height = Math.ceil(height * ratio);
    const ctx = canvas.getContext("2d");
    if (!ctx) return;
    ctx.scale(ratio, ratio);
    ctx.clearRect(0, 0, width, height);
    const styles = getComputedStyle(canvas),
      text = styles.getPropertyValue("--lv-text").trim() || "#dce5ef",
      muted = styles.getPropertyValue("--lv-muted").trim() || "#98a8bb",
      line = styles.getPropertyValue("--lv-line").trim() || "#273446";
    ctx.strokeStyle = line;
    ctx.lineWidth = 0.6;
    ctx.setLineDash([2, 6]);
    for (let x = 32 - (left % (72 * zoom)); x < width; x += 72 * zoom) {
      ctx.beginPath();
      ctx.moveTo(x, 0);
      ctx.lineTo(x, height);
      ctx.stroke();
    }
    ctx.setLineDash([]);
    const screen = (m: { x: number; y: number }) => ({
      x: m.x * zoom - left,
      y: m.y * zoom - top,
    });
    for (const m of layout.marks) {
      const c = screen(m);
      for (const id of m.commit.parents) {
        const p = layout.byOid.get(id);
        if (!p) {
          if (c.x > -60 && c.x < width + 60 && c.y > -40 && c.y < height + 40) {
            ctx.strokeStyle = muted;
            ctx.setLineDash([3, 4]);
            ctx.beginPath();
            ctx.moveTo(c.x - 28, c.y);
            ctx.lineTo(c.x - 8, c.y);
            ctx.stroke();
            ctx.setLineDash([]);
            ctx.font = "11px ui-monospace,monospace";
            ctx.fillStyle = muted;
            ctx.fillText("…", c.x - 40, c.y + 4);
          }
          continue;
        }
        const a = screen(p);
        if (
          Math.max(a.x, c.x) < -50 ||
          Math.min(a.x, c.x) > width + 50 ||
          Math.max(a.y, c.y) < -50 ||
          Math.min(a.y, c.y) > height + 50
        )
          continue;
        ctx.strokeStyle = colors[m.lane % colors.length];
        ctx.globalAlpha = 0.6;
        ctx.lineWidth = 1.7;
        ctx.beginPath();
        ctx.moveTo(a.x, a.y);
        const bend = Math.max(20, (c.x - a.x) / 2);
        ctx.bezierCurveTo(a.x + bend, a.y, c.x - bend, c.y, c.x, c.y);
        ctx.stroke();
        ctx.globalAlpha = 1;
      }
    }
    const labels: { x: number; y: number; text: string; color: string; priority: number }[] = [];
    for (const m of layout.marks) {
      const c = screen(m);
      if (c.x < -40 || c.x > width + 40 || c.y < -40 || c.y > height + 40)
        continue;
      if (m.range) continue;
      const color = colors[m.lane % colors.length];
      ctx.fillStyle = color;
      ctx.beginPath();
      ctx.arc(c.x, c.y, m.commit.parents.length > 1 ? 6 : 4, 0, Math.PI * 2);
      ctx.fill();
      if (selected === m.commit.oid) {
        ctx.strokeStyle = text;
        ctx.lineWidth = 2;
        ctx.beginPath();
        ctx.arc(c.x, c.y, 10, 0, Math.PI * 2);
        ctx.stroke();
      }
      if (zoom >= 0.65) {
        ctx.fillStyle = muted;
        ctx.font = "11px ui-monospace,monospace";
        ctx.fillText(m.commit.oid.slice(0, 7), c.x + 11, c.y + 4);
      }
      const refs =
        project.git?.refs.filter((r) => r.commit_oid === m.commit.oid) ?? [];
      if (refs.length) {
        const full = refs.map((r) => shortRef(r.name)).join(" · ");
        labels.push({ x: c.x, y: c.y, text: full.length > 42 ? `${full.slice(0, 41)}…` : full,
          color, priority: selected === m.commit.oid ? 2 : refs.some((r) => r.name === "refs/heads/main" || r.name === "refs/heads/master") ? 1 : 0 });
      }
      const trees =
        project.git?.checkouts.filter((t) => t.head === m.commit.oid) ?? [];
      if (trees.length) {
        ctx.strokeStyle = palette.sol;
        ctx.lineWidth = 1.8;
        ctx.beginPath();
        ctx.moveTo(c.x, c.y + 10);
        ctx.lineTo(c.x + 5, c.y + 16);
        ctx.lineTo(c.x, c.y + 22);
        ctx.lineTo(c.x - 5, c.y + 16);
        ctx.closePath();
        ctx.stroke();
        const dirty = trees.some(
          (t) =>
            t.status &&
            t.status.staged + t.status.unstaged + t.status.untracked > 0,
        );
        if (dirty) {
          ctx.setLineDash([3, 3]);
          ctx.beginPath();
          ctx.moveTo(c.x + 6, c.y + 16);
          ctx.lineTo(c.x + 24, c.y + 16);
          ctx.stroke();
          ctx.strokeRect(c.x + 24, c.y + 12, 8, 8);
          ctx.setLineDash([]);
        }
      }
    }
    const occupied: { x: number; y: number; width: number }[] = [];
    ctx.font = "600 11px ui-monospace,monospace";
    for (const label of labels.sort((a, b) => b.priority - a.priority || a.x - b.x)) {
      const w = Math.min(width, 270, ctx.measureText(label.text).width + 16);
      const x = Math.max(0, Math.min(width - w, label.x - 8)), y = label.y - 30;
      if (occupied.some((box) => Math.abs(box.y - y) < 24 && x < box.x + box.width + 8 && x + w + 8 > box.x)) continue;
      occupied.push({ x, y, width: w });
      ctx.fillStyle = line;
      ctx.beginPath();
      ctx.roundRect(x, y, w, 20, 4);
      ctx.fill();
      ctx.fillStyle = label.color;
      if (w > 16) ctx.fillText(label.text, x + 8, y + 14, w - 16);
    }
  }, [project, layout, zoom, left, top, width, height, selected]);
  const nearest = (x: number, y: number) => {
    let distance = 24,
      found: null | (typeof layout.marks)[number] = null;
    for (const m of layout.marks) {
      if (m.range) continue;
      const d = Math.hypot(m.x * zoom - left - x, m.y * zoom - top - y);
      if (d < distance) {
        distance = d;
        found = m;
      }
    }
    return found;
  };
  return (
    <>
      <canvas
        ref={ref}
        style={{ left, top: top + 48, width, height }}
        className="git-canvas"
        tabIndex={0}
        role="img"
        aria-label={`${project.name}: ${layout.orderedCommits.length} commits, ${layout.ranges.filter((r) => !r.expanded).length} collapsed ranges. Arrow keys navigate ancestry; Enter inspects. Full details available in Commit inventory.`}
        onPointerMove={(e) => {
          const r = e.currentTarget.getBoundingClientRect(),
            mark = nearest(e.clientX - r.left, e.clientY - r.top);
          setHover(
            mark
              ? `${mark.commit.oid.slice(0, 10)} · ${mark.commit.subject}${project.git?.refs.filter((r) => r.commit_oid === mark.commit.oid).map((r) => ` · ${shortRef(r.name)}`).join("") ?? ""}`
              : "",
          );
        }}
        onPointerLeave={() => setHover("")}
        onClick={(e) => {
          const r = e.currentTarget.getBoundingClientRect(),
            mark = nearest(e.clientX - r.left, e.clientY - r.top);
          if (mark) onSelect({ kind: "commit", project, commit: mark.commit });
        }}
        onKeyDown={(e) => {
          if (
            ![
              "ArrowLeft",
              "ArrowRight",
              "ArrowUp",
              "ArrowDown",
              "Enter",
            ].includes(e.key)
          )
            return;
          e.preventDefault();
          const current =
            layout.byOid.get(selected ?? "") ?? layout.marks.at(-1);
          if (!current) return;
          let next = current;
          if (e.key === "ArrowLeft")
            next = layout.byOid.get(current.commit.parents[0]) ?? current;
          if (e.key === "ArrowRight")
            next =
              layout.marks.find((m) =>
                m.commit.parents.includes(current.commit.oid),
              ) ?? current;
          if (e.key === "ArrowUp" || e.key === "ArrowDown") {
            const index = layout.marks.indexOf(current);
            next =
              layout.marks[
                Math.max(
                  0,
                  Math.min(
                    layout.marks.length - 1,
                    index + (e.key === "ArrowUp" ? -1 : 1),
                  ),
                )
              ];
          }
          if (next.range) onToggleRange(next.range);
          else onSelect({ kind: "commit", project, commit: next.commit });
        }}
      />
      {layout.ranges.map((range) => {
        const x = range.x * zoom - left;
        const y = range.y * zoom - top;
        const buttonWidth = Math.min(width, range.expanded ? 156 : gitRangeWidth(range.commits.length, zoom));
        const narrow = !range.expanded && buttonWidth < 304;
        const compact = !range.expanded && buttonWidth < 144;
        const tiny = !range.expanded && buttonWidth < 68;
        if (x + buttonWidth / 2 < 0 || x - buttonWidth / 2 > width || y < 0 || y > height)
          return null;
        const first = range.commits[0].oid.slice(0, 7);
        const last = range.commits.at(-1)!.oid.slice(0, 7);
        return (
          <button
            key={range.id}
            className={`git-commit-range ${range.expanded ? "is-expanded" : ""} ${narrow ? "is-narrow" : ""} ${compact ? "is-compact" : ""} ${tiny ? "is-tiny" : ""}`}
            style={{ left: left + Math.max(0, Math.min(width - buttonWidth, x - buttonWidth / 2)), top: top + 48 + y - (range.expanded ? 38 : tiny ? 13 : narrow ? 26 : 19), width: buttonWidth }}
            aria-expanded={range.expanded}
            aria-label={range.expanded ? `− Collapse ${range.commits.length} commits` : `${first} → ${last} + ${range.commits.length} hidden. Expand commits`}
            title={`${range.commits[0].oid} → ${range.commits.at(-1)!.oid} · ${range.commits.length} linear commits. ${range.expanded ? "Collapse range" : "Expand range"}.`}
            onClick={() => onToggleRange(range)}
          >
            {range.expanded ? (
              <>− Collapse {range.commits.length} commits</>
            ) : tiny ? (
              <span>+{range.commits.length}</span>
            ) : compact ? (
              <><code><span>{first}</span><span>→ {last}</span></code><span>+ {range.commits.length} hidden</span></>
            ) : (
              <><code>{first} → {last}</code><span>+ {range.commits.length} hidden</span></>
            )}
          </button>
        );
      })}
      {hover && (
        <div
          className="git-hover"
          style={{ left: left + 12, top: top + height - 30 }}
        >
          {hover}
        </div>
      )}
    </>
  );
}
export default function GitFlow({
  snapshot,
  paused,
  onPause: setPaused,
  selection,
  onSelect,
  now,
  token,
  onAccess,
  chatScopeEpoch,
  chatDisabledReason,
  healthEnabled,
}: {
  snapshot: ViewsSnapshot;
  paused: ViewsSnapshot | null;
  onPause: (value: ViewsSnapshot | null) => void;
  selection: Selection | null;
  onSelect: (s: Selection | null) => void;
  now: number;
  token?: string;
  onAccess: () => void;
  chatScopeEpoch: number;
  chatDisabledReason: string;
  healthEnabled: boolean;
}) {
  const chatMemory = useMemo(() => createChatMemory(), [token, chatScopeEpoch, snapshot.simulated]);
  const [chatRevision, setChatRevision] = useState(0);
  const [healthOpen, setHealthOpen] = useState<ReadonlySet<string>>(new Set());
  const health = useGitHealth(token, chatScopeEpoch, healthEnabled && !paused);
  const route = useRoute(),
    filter =
      route.params.get("scope") ??
      (route.params.has("focus") ? "" : (route.params.get("project") ?? "")),
    query = route.params.get("q") ?? "";
  const initialView = scopeViews.get(filter) ?? viewMemory;
  const [zoom, setZoom] = useState(initialView.zoom),
    [scroll, setScroll] = useState({
      left: initialView.left,
      top: initialView.top,
    }),
    [inventory, setInventory] = useState(initialView.inventory),
    [shown, setShown] = useState(100),
    [follow, setFollow] = useState(initialView.follow),
    [details, setDetails] = useState(false),
    [fitLimited, setFitLimited] = useState(false),
    [cameraRevision, setCameraRevision] = useState(0),
    [expanded, setExpanded] = useState<ReadonlySet<string>>(new Set()),
    [collapsed, setCollapsed] = useState<ReadonlySet<string>>(new Set());
  const frozen = paused ?? snapshot,
    viewport = useSize<HTMLDivElement>(),
    scrollRef = useRef<HTMLDivElement>(null),
    manual = useRef(false),
    previous = useRef(snapshot.revision),
    [changes, setChanges] = useState(0);
  const projects = useMemo(
    () =>
      frozen.projects.filter(
        (p) =>
          (!filter || p.id === filter) &&
          (!query ||
            [
              p.name,
              p.id,
              ...(p.git?.refs.map((r) => r.name) ?? []),
              ...(p.git?.commits.map((c) => `${c.oid} ${c.subject}`) ?? []),
            ]
              .join(" ")
              .toLowerCase()
              .includes(query.toLowerCase())),
      ),
    [frozen, filter, query],
  );
  const focusId = route.params.get("focus") ?? "",
    branch = route.params.get("branch") ?? "",
    focusProject = route.params.get("project") ?? "",
    selectedOid = selection?.kind === "commit" ? selection.commit.oid : selection?.kind === "checkout" ? selection.checkout.head ?? "" : "",
    selectedProject = selection && selection.kind !== "agent" ? selection.project.id : "";
  const rangeAnchor = useRef<{ projectId: string; oid: string; screenX: number } | null>(null);
  const buildBands = (scale: number) => {
    let top = 0;
    const needle = query.trim().toLowerCase();
    return projects.map((project) => {
      const revealed = new Set(project.git?.commits.filter((c) => {
        const key = `${project.id}:${c.oid}`;
        return expanded.has(key) || (!collapsed.has(key) && (
          (c.oid === focusId && (!focusProject || project.id === focusProject)) ||
          (project.id === selectedProject && c.oid === selectedOid) ||
          (needle && `${c.oid} ${c.subject}`.toLowerCase().includes(needle))
        ));
      }).map((c) => c.oid));
      const collapsedOids = new Set(project.git?.commits
        .filter((commit) => collapsed.has(`${project.id}:${commit.oid}`))
        .map((commit) => commit.oid));
      const layout = project.git ? layoutGit(project.git, revealed, scale, collapsedOids) : null;
      const height = Math.max(
        chatMemory.projects.get(project.id)?.open || healthOpen.has(project.id) ? 480 : 180,
        layout ? layout.height * scale + 76 : 180,
      );
      const row = { projectId: project.id, project, layout, top, height };
      top += height;
      return row;
    });
  };
  const bands = useMemo(() => buildBands(zoom), [projects, zoom, expanded, collapsed, focusId, focusProject, selectedOid, selectedProject, query, chatMemory, chatRevision, healthOpen]);
  const gutter = filter && !details ? 0 : viewport.width < 620 ? 160 : 240;
  const graphViewport = { width: Math.max(1, viewport.width - gutter), height: viewport.height };
  const pendingCamera = useRef<{ zoom: number; left: number; top: number } | null>(null);
  const lastView = useRef({ filter, zoom, left: scroll.left, top: scroll.top, follow, inventory });
  const applyCamera = (camera: { zoom: number; left: number; top: number }, following = false) => {
    rangeAnchor.current = null;
    manual.current = false;
    pendingCamera.current = camera;
    setFollow(following);
    setFitLimited(false);
    setZoom(camera.zoom);
    setCameraRevision((n) => n + 1);
  };
  const fitView = (enterScope = false) => {
    const next = fitGitZoom((scale) => {
      const measured = buildBands(scale);
      return {
        width: Math.max(0, ...measured.map((b) => (b.layout?.width ?? 0) * scale)),
        height: 0,
      };
    }, { width: Math.max(1, graphViewport.width - 24), height: Number.MAX_SAFE_INTEGER });
    const anchor = captureGitAnchor({ bands, zoom, scroll, viewport: graphViewport,
      selected: selectedProject && selectedOid ? { projectId: selectedProject, oid: selectedOid } : undefined });
    const measured = buildBands(next);
    const restored = anchor && restoreGitAnchor(anchor, measured, next);
    const selectedBand = measured.find((band) => band.projectId === selectedProject);
    const selectedMark = selectedBand?.layout?.byOid.get(selectedOid);
    const entryTop = selectedBand && selectedMark ? Math.max(0, selectedBand.top + 48 + selectedMark.y * next - graphViewport.height / 2) : 0;
    applyCamera({ zoom: next, left: 0, top: enterScope ? entryTop : restored?.top ?? scroll.top });
    setFitLimited(measured.some((band) => (band.layout?.width ?? 0) * next > graphViewport.width - 24));
  };
  const zoomTo = (next: number) => {
    const anchor = captureGitAnchor({ bands, zoom, scroll, viewport: graphViewport,
      selected: selectedProject && selectedOid ? { projectId: selectedProject, oid: selectedOid } : undefined });
    const restored = anchor && restoreGitAnchor(anchor, buildBands(next), next);
    applyCamera({ zoom: next, left: restored?.left ?? scroll.left * next / zoom, top: restored?.top ?? scroll.top });
  };
  const changeScope = (id: string) => {
    const params = new URLSearchParams(window.location.hash.split("?")[1]);
    if (id) params.set("scope", id); else {
      params.delete("scope");
      // A project+branch link without an inspector uses the legacy scope fallback.
      // Explicit overview navigation exits that scope as well.
      if (!params.has("focus")) { params.delete("project"); params.delete("branch"); }
    }
    if (id && focusProject && focusProject !== id) { params.delete("focus"); params.delete("branch"); }
    if (!params.has("focus") && !params.has("branch")) {
      if (id) params.set("project", id); else params.delete("project");
    }
    window.location.hash = routeHref("git", Object.fromEntries(params));
    setShown(100);
  };
  useLayoutEffect(() => {
    const before = lastView.current;
    if (before.filter !== filter) {
      scopeViews.set(before.filter, before);
      const remembered = scopeViews.get(filter);
      if (remembered) applyCamera({ zoom: remembered.zoom, left: remembered.left, top: remembered.top }, remembered.follow);
      else fitView(true);
    }
    lastView.current = { filter, zoom, left: scroll.left, top: scroll.top, follow, inventory };
  });
  useLayoutEffect(() => {
    const camera = pendingCamera.current, el = scrollRef.current;
    if (!camera || !el || camera.zoom !== zoom) return;
    pendingCamera.current = null;
    el.scrollLeft = camera.left;
    el.scrollTop = camera.top;
    setScroll({ left: el.scrollLeft, top: el.scrollTop });
  }, [bands, zoom, cameraRevision, inventory, gutter]);
  useEffect(() => {
    if (!focusId) return;
    // A new explicit focus reveals its whole range; routine source updates do not reset manual intent.
    const reveal = new Set<string>();
    for (const project of frozen.projects) {
      if (focusProject && project.id !== focusProject) continue;
      const range = project.git ? layoutGit(project.git).byOid.get(focusId)?.range : undefined;
      for (const oid of range?.commits.map((commit) => commit.oid) ?? [focusId])
        reveal.add(`${project.id}:${oid}`);
    }
    setCollapsed((before) => {
      const next = new Set([...before].filter((key) => !reveal.has(key)));
      return next.size === before.size ? before : next;
    });
  }, [focusId, focusProject]);
  useLayoutEffect(() => {
    const anchor = rangeAnchor.current, el = scrollRef.current;
    if (!anchor || !el) return;
    const mark = bands.find((b) => b.project.id === anchor.projectId)?.layout?.byOid.get(anchor.oid);
    if (mark) {
      el.scrollLeft = Math.max(0, mark.x * zoom - anchor.screenX);
      setScroll({ left: el.scrollLeft, top: el.scrollTop });
    }
    rangeAnchor.current = null;
  }, [bands, zoom]);
  const toggleRange = (projectId: string, range: CommitRange) => {
    rangeAnchor.current = { projectId, oid: range.commits[0].oid, screenX: range.x * zoom - scroll.left };
    setFollow(false);
    setCollapsed((before) => {
      const next = new Set(before);
      for (const c of range.commits)
        if (range.expanded) next.add(`${projectId}:${c.oid}`);
        else next.delete(`${projectId}:${c.oid}`);
      return next;
    });
    if (range.expanded && selection?.kind === "commit" && selection.project.id === projectId &&
      range.commits.some((c) => c.oid === selection.commit.oid)) onSelect(null);
    setExpanded((before) => {
      const next = new Set(before);
      for (const c of range.commits)
        if (range.expanded) next.delete(`${projectId}:${c.oid}`);
        else next.add(`${projectId}:${c.oid}`);
      return next;
    });
  };
  const width = Math.max(
      graphViewport.width,
      ...bands.map((b) => (b.layout?.width ?? 0) * zoom),
    ),
    height = bands.reduce((n, b) => n + b.height, 0);
  const restored = useRef<HTMLDivElement | null>(null);
  useLayoutEffect(() => {
    const el = scrollRef.current;
    if (!el) {
      restored.current = null;
    } else if (viewport.width > 0 && restored.current !== el) {
      restored.current = el;
      el.scrollLeft = scroll.left;
      el.scrollTop = scroll.top;
      // The source or viewport may have shrunk while the graph was hidden.
      setScroll({ left: el.scrollLeft, top: el.scrollTop });
    }
  }, [viewport.element, viewport.width, inventory]);
  useEffect(() => {
    viewMemory = {
      zoom,
      left: scroll.left,
      top: scroll.top,
      follow,
      inventory,
    };
    if (!pendingCamera.current) {
      const el = scrollRef.current;
      scopeViews.set(filter, { ...viewMemory, left: el?.scrollLeft ?? scroll.left, top: el?.scrollTop ?? scroll.top });
    }
  }, [filter, zoom, scroll, follow, inventory]);
  const latest = () => {
    const el = scrollRef.current;
    if (el) {
      el.scrollLeft = el.scrollWidth - el.clientWidth;
      setScroll({ left: el.scrollLeft, top: el.scrollTop });
    }
    setChanges(0);
  };
  useEffect(() => {
    if (!inventory && follow && !paused && !pendingCamera.current) latest();
  }, [width, follow, paused, inventory]);
  useEffect(() => {
    if (previous.current !== snapshot.revision) {
      if (!follow || paused) setChanges((n) => n + 1);
      previous.current = snapshot.revision;
    }
  }, [snapshot.revision, follow, paused]);
  const appliedFocus = useRef("");
  useEffect(() => {
    const el = scrollRef.current;
    const key = JSON.stringify([focusProject, focusId, branch]);
    if (!el || appliedFocus.current === key) return;
    if (!focusId && !branch) { appliedFocus.current = key; return; }
    const band = bands.find(
      (b) =>
        (!focusProject || b.project.id === focusProject) && (
          b.project.git?.commits.some((c) => c.oid === focusId) ||
          b.project.git?.checkouts.some(
            (c) => c.id === focusId || (branch && c.branch === branch),
          )
        ),
    );
    if (!band?.layout) return;
    const tree = band.project.git?.checkouts.find(
      (t) => t.id === focusId || (branch && t.branch === branch),
    );
    const mark = band.layout.byOid.get(tree?.head ?? focusId);
    if (mark && !mark.range) {
      appliedFocus.current = key;
      setFollow(false);
      el.scrollLeft = Math.max(0, mark.x * zoom - (el.clientWidth - gutter) / 2);
      el.scrollTop = Math.max(band.top, band.top + 48 + mark.y * zoom - el.clientHeight / 2);
      setScroll({ left: el.scrollLeft, top: el.scrollTop });
    }
  }, [focusId, focusProject, branch, bands, zoom, inventory]);
  const commits = bands
    .flatMap((b) =>
      (b.layout?.orderedCommits ?? []).map((commit) => ({
        project: b.project,
        commit,
      })),
    )
    .reverse();
  const allCommits = projects.reduce(
      (n, p) => n + (p.git?.commits.length ?? 0),
      0,
    ),
    checkouts = projects.reduce(
      (n, p) => n + (p.git?.checkouts.length ?? 0),
      0,
    );
  return (
    <section className={`git-view ${filter ? "is-project-focused" : ""}`} aria-label="Git Flow" style={{ "--git-gutter-width": `${gutter}px` } as CSSProperties}>
      <div className="lv-toolbar">
        <label className="lv-search">
          <span aria-hidden="true" data-icon="⌕" />
          <input
            aria-label="Search Git projects and commits"
            placeholder="Find project, branch, commit…"
            value={query}
            onChange={(e) => {
              changeParams({ q: e.target.value });
              setCollapsed(new Set());
              setShown(100);
            }}
          />
        </label>
        <select
          aria-label="Filter Git project"
          value={filter}
          onChange={(e) => changeScope(e.target.value)}
        >
          <option value="">All projects</option>
          {frozen.projects.map((p) => (
            <option value={p.id} key={p.id}>
              {p.name}
            </option>
          ))}
        </select>
        <span className="lv-axis-pill">ANCESTRY → COMMIT ORDER</span>
        <span className="lv-toolbar-spacer" />
        <button
          className={`lv-button ${inventory ? "active" : ""}`}
          aria-pressed={inventory}
          onClick={() => setInventory(!inventory)}
        >
          {inventory ? "⑂ Graph" : "☷ Inventory"}
        </button>
        {!inventory && <button className="lv-button" onClick={() => {
          setFollow(false);
          const all = projects.flatMap((p) => p.git?.commits.map((c) => `${p.id}:${c.oid}`) ?? []);
          const expand = bands.some((b) => b.layout?.ranges.some((r) => !r.expanded));
          setCollapsed((before) => {
            const next = new Set(before);
            for (const key of all) if (expand) next.delete(key); else next.add(key);
            return next;
          });
          setExpanded((before) => {
            const next = new Set(before);
            for (const key of all) if (expand) next.add(key); else next.delete(key);
            return next;
          });
          if (!expand && selection?.kind === "commit" && bands.some((band) =>
            band.project.id === selection.project.id && band.layout?.ranges.some((range) =>
              range.commits.some((commit) => commit.oid === selection.commit.oid)))) onSelect(null);
        }}>
          {bands.some((b) => b.layout?.ranges.some((r) => !r.expanded)) ? "Expand all" : "Collapse linear"}
        </button>}
        <button
          className="lv-button"
          onClick={() => {
            if (paused) {
              setPaused(null);
              setChanges(0);
            } else setPaused(snapshot);
          }}
        >
          {paused ? "▶ Resume live" : "Ⅱ Pause"}
        </button>
        <button
          className={`lv-button ${follow ? "active" : ""}`}
          aria-pressed={follow}
          onClick={() => {
            setFollow(!follow);
            if (!follow) latest();
          }}
        >
          Follow latest
        </button>
      </div>
      <div className="git-ruler">
        <div className="git-scope-summary">
          {filter && <button className="lv-text-button" onClick={() => changeScope("")}>← All projects</button>}
          {filter && <strong>{frozen.projects.find((p) => p.id === filter)?.name ?? filter}</strong>}
          <span className="git-axis-icon" aria-hidden="true" data-icon="⑂" />
          <b>{projects.length}</b> {projects.length === 1 ? "project" : "projects"} <span> / </span>
          <b>{allCommits}</b> commits <span> / </span>
          <b>{checkouts}</b> checkouts
          {filter && !inventory && <button className="lv-text-button" aria-pressed={details} onClick={() => setDetails(!details)}>{details ? "Hide" : "Show"} project details</button>}
        </div>
        <span>
          Parents always precede children. Each repository has its own
          coordinates.
        </span>
        {!inventory && <div className="git-zoom" role="group" aria-label="Git viewport controls">
          <button aria-label="Zoom Git out" onClick={() => zoomTo(Math.max(0.05, zoom / 1.2))}>
            <span data-icon="−" aria-hidden="true" />
          </button>
          <output aria-label="Current Git zoom">{Math.round(zoom * 100)}%</output>
          <button aria-label="Zoom Git in" onClick={() => zoomTo(Math.min(2, zoom * 1.2))}>
            <span data-icon="+" aria-hidden="true" />
          </button>
          <button className="git-reset-zoom" aria-label="Reset Git zoom to 100%" title="Actual size · 1" onClick={() => zoomTo(1)}>
            <span aria-hidden="true">[ ]</span> 100%
          </button>
          <button title="Fit loaded history width; scroll vertically for branches · 0" onClick={() => fitView()}>Fit width</button>
          <button onClick={latest}>Latest →</button>
        </div>}

      </div>
      {paused && (
        <div className="lv-paused">
          Historical inspection paused at this observed snapshot. New source
          changes accumulate without moving the view.
        </div>
      )}
      {changes > 0 && (
        <button
          className="git-new-changes"
          onClick={() => {
            setPaused(null);
            setFollow(true);
            latest();
          }}
        >
          {changes} new source cuts · Return to latest →
        </button>
      )}
      {inventory ? (
        <div className="git-inventory">
          <div className="git-inventory-checkouts">
            <h2>Checkouts & worktrees</h2>
            {projects.map((p) => (
              <section key={p.id}>
                <h3>{p.name}</h3>
                {p.git?.checkouts.length ? (
                  p.git.checkouts.map((c) => (
                    <button
                      className="git-checkout-row"
                      key={c.id}
                      onClick={() =>
                        onSelect({ kind: "checkout", project: p, checkout: c })
                      }
                    >
                      <span
                        className="git-checkout-glyph"
                        aria-hidden="true"
                        data-icon="◇"
                      />
                      <span>
                        <b>{c.name}</b>
                        <small>
                          {c.kind} ·{" "}
                          {c.branch ??
                            (c.detached ? "detached HEAD" : "unborn HEAD")}
                        </small>
                      </span>
                      <code>{c.head?.slice(0, 8) ?? "No commit"}</code>
                      <small>{working(c)}</small>
                    </button>
                  ))
                ) : (
                  <p>Git checkouts not observed.</p>
                )}
              </section>
            ))}
          </div>
          <h2>Commit inventory · {commits.length} loaded</h2>
          <p>
            Search checks every loaded commit. Window boundaries remain
            explicit; this is not a complete historical repository export.
          </p>
          <table>
            <thead>
              <tr>
                <th>Commit</th>
                <th>Project / subject</th>
                <th>Parents</th>
                <th>Committer time</th>
              </tr>
            </thead>
            <tbody>
              {commits.slice(0, shown).map(({ project, commit: c }) => (
                <tr key={`${project.id}:${c.oid}`}>
                  <td>
                    <button
                      className="lv-text-button"
                      onClick={() =>
                        onSelect({ kind: "commit", project, commit: c })
                      }
                    >
                      {c.oid.slice(0, 10)}
                    </button>
                  </td>
                  <td>
                    {project.name}
                    <small>{c.subject}</small>
                  </td>
                  <td>
                    {c.parents.map((p) => p.slice(0, 7)).join(", ") ||
                      "Root / shallow boundary"}
                  </td>
                  <td>{c.committed_utc}</td>
                </tr>
              ))}
            </tbody>
          </table>
          {commits.length > shown && (
            <button
              className="lv-button"
              onClick={() => setShown((n) => n + 200)}
            >
              Show next {Math.min(200, commits.length - shown)} commits
            </button>
          )}
        </div>
      ) : (
        <div className="git-surface" ref={viewport.ref} onKeyDown={(event) => {
          const target = event.target as HTMLElement;
          if (event.ctrlKey || event.metaKey || event.altKey || target.closest("input, textarea, select, button, [contenteditable=true]")) return;
          if (event.key === "0" || event.key === "1" || event.key === "+" || event.key === "-") {
            event.preventDefault();
            if (event.key === "0") fitView();
            if (event.key === "1") zoomTo(1);
            if (event.key === "+") zoomTo(Math.min(2, zoom * 1.2));
            if (event.key === "-") zoomTo(Math.max(0.05, zoom / 1.2));
          }
        }}>
          <div
            className="git-scroll"
            ref={scrollRef}
            onWheel={(e) => {
              if (e.deltaX || e.shiftKey) manual.current = true;
            }}
            onPointerDown={() => {
              manual.current = true;
            }}
            onScroll={(e) => {
              const el = e.currentTarget;
              setScroll({ left: el.scrollLeft, top: el.scrollTop });
              if (manual.current) {
                setFollow(false);
                manual.current = false;
              }
            }}
          >
            <div
              className="git-world"
              style={{
                width: width + gutter,
                height: Math.max(height, viewport.height),
              }}
            >
              {bands.map((b) => {
                const localTop = Math.max(0, scroll.top - b.top - 64),
                  visibleHeight = Math.max(
                    1,
                    Math.min(b.height - 64 - localTop, viewport.height),
                  );
                const inViewport =
                  b.top + b.height >= scroll.top - 200 &&
                  b.top <= scroll.top + viewport.height + 200;
                return (
                  <section
                    key={b.project.id}
                    className="git-band"
                    style={{ height: b.height }}
                    aria-label={`${b.project.name} repository`}
                  >
                    <div className="git-gutter" hidden={gutter === 0}>
                      <header>
                        <span className="git-project-symbol" aria-hidden="true" data-icon={b.project.name.slice(0, 1).toUpperCase()} />
                        <div>
                          <h2>{b.project.name}</h2>
                          <span>
                            {b.project.git?.node ?? "Source not configured"}
                          </span>
                        </div>
                      </header>
                      {!filter && <button className="lv-button git-focus-project" onClick={() => changeScope(b.project.id)} aria-label={`Focus project: ${b.project.name}`}>Focus project <span aria-hidden="true">↗</span></button>}
                      <div className="git-source-age">
                        {b.project.git
                          ? age(b.project.git.observed_utc, now)
                          : "No Git observation"}
                      </div>
                      <GitHealthBadge
                        report={health.reports[b.project.id]?.instance_id === b.project.git?.instance_id ? health.reports[b.project.id] : undefined}
                        error={health.error}
                        now={now}
                        onToggle={(open) => setHealthOpen((previous) => {
                          if (previous.has(b.project.id) === open) return previous;
                          const next = new Set(previous);
                          if (open) next.add(b.project.id); else next.delete(b.project.id);
                          return next;
                        })}
                      />
                      {b.project.git?.error && (
                        <p className="git-source-error">
                          {b.project.git.error}
                        </p>
                      )}
                      <div className="git-checkout-list">
                        {b.project.git?.checkouts.slice(0, 8).map((c) => (
                          <button
                            key={c.id}
                            onClick={() =>
                              onSelect({
                                kind: "checkout",
                                project: b.project,
                                checkout: c,
                              })
                            }
                          >
                            <span aria-hidden="true" data-icon="◇" />
                            <div>
                              <b>
                                {c.branch ??
                                  (c.detached
                                    ? "Detached HEAD"
                                    : "Unborn HEAD")}
                              </b>
                              <small>
                                {c.name} · {working(c)}
                              </small>
                            </div>
                            {c.locked && (
                              <span title="Administrative lock">⚿</span>
                            )}
                          </button>
                        ))}
                        {(b.project.git?.checkouts.length ?? 0) > 8 && (
                          <button onClick={() => setInventory(true)}>
                            View all {b.project.git?.checkouts.length} checkouts
                            →
                          </button>
                        )}
                      </div>
                    </div>
                    <div
                      className="git-graph-field"
                      style={{ height: b.height }}
                    >
                      <div
                        className="git-band-caption"
                        style={{ left: scroll.left + 16 }}
                      >
                        <span>
                          {b.project.git
                            ? `${b.project.git.refs.length} refs · ${b.project.git.commits.length} commits`
                            : "NOT OBSERVED"}
                        </span>
                        {b.project.git?.shallow && <b>SHALLOW ANCESTRY</b>}
                        {b.project.git?.truncated && <b>BOUNDED WINDOW</b>}
                        {!!b.layout?.boundaries && (
                          <span>{b.layout.boundaries} parent boundaries</span>
                        )}
                      </div>
                      {b.layout && b.project.git?.commits.length ? (
                        inViewport && (
                          <GitCanvas
                            project={b.project}
                            layout={b.layout}
                            zoom={zoom}
                            left={scroll.left}
                            top={localTop}
                            width={graphViewport.width}
                            height={visibleHeight}
                            selection={selection}
                            onSelect={onSelect}
                            onToggleRange={(range) => toggleRange(b.project.id, range)}
                          />
                        )
                      ) : (
                        <div
                          className="git-no-data"
                          style={{ left: scroll.left + 40 }}
                        >
                          <span aria-hidden="true" data-icon="⑂" />
                          <h3>
                            {b.project.git
                              ? b.project.git.error
                                ? "Git source unavailable"
                                : "No committed history yet"
                              : "Git repository not observed"}
                          </h3>
                          <p>
                            {b.project.git?.error
                              ? "Last successful facts are retained when available."
                              : b.project.git
                                ? "Unborn and empty repositories remain represented."
                                : "Enable the Rust observer for this project. Remote-only branches remain unobserved."}
                          </p>
                        </div>
                      )}
                    </div>
                  </section>
                );
              })}
              {!bands.length && (
                <div className="lv-empty">
                  <h2>No matching projects</h2>
                  <p>
                    Clear project or search filters to see all repositories.
                  </p>
                </div>
              )}
            </div>
          </div>
          <div className="git-chat-overlays">
            {bands.map((band) => {
              const top = Math.max(12, band.top - scroll.top + 52);
              const available = Math.min(viewport.height - top - 12, band.top + band.height - scroll.top - top - 12);
              const visible = available >= 60 && band.top < scroll.top + viewport.height - 60;
              return <div key={`${chatMemory.id}:${band.project.id}`} className="git-chat-anchor" hidden={!visible} style={{ top, maxHeight: Math.max(60, available) }}>
                <GitChat
                  projectId={band.project.id}
                  projectName={band.project.name}
                  token={token}
                  memory={chatMemory}
                  active={visible && !snapshot.simulated}
                  disabledReason={chatDisabledReason}
                  onAccess={onAccess}
                  onToggle={() => setChatRevision((revision) => revision + 1)}
                />
              </div>;
            })}
          </div>
        </div>
      )}
      <div className="lv-view-note">
        <span>
          <i className="git-legend-dot" /> Commit &nbsp; ─ Actual parent &nbsp;
          ◇ Checkout HEAD &nbsp; ▫ Working changes &nbsp; ⋯ Missing ancestry &nbsp; [＋] Linear range
        </span>
        {fitLimited && <span role="status">History is wider than this view at minimum zoom; scroll horizontally.</span>}
        <span>{filter ? "Scroll vertically for branches" : "Scroll vertically for projects"} · 0 Fit width · 1 Actual size · + / − Zoom</span>
      </div>
    </section>
  );
}
