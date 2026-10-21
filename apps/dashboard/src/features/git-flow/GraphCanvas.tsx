import { useEffect, useMemo, useRef, useState } from "react";
import type { ViewProject } from "../live-views/feed";
import { gitRangeWidth, type CommitRange, type GitLayout } from "../live-views/geometry";
import type { Selection } from "../live-views/LiveViews";
import { palette } from "../live-views/FleetAtlas";
import { GIT_LANE_SPACING, type GitGraphLayout, type GraphEdge } from "./model";
import { createGraphSpatialIndex } from "./scene";
import { parentCurve } from "./curves";
import { createGitMergeIndex } from "./merges";
import { placeGitLabels, type GitLabelCandidate, type GitPlacedLabel } from "./labels";
const colors = [palette.gemini, palette.claude, palette.luna, palette.sol, palette.local, palette.astra];
const shortRef = (ref: string) => ref.replace(/^refs\/(heads|remotes|tags)\//, "");
export default function GitCanvas({
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
  fixed = false,
  visibleOids,
  safeInset = 24,
  onHiddenRefs,
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
  fixed?: boolean;
  visibleOids?: ReadonlySet<string>;
  safeInset?: number;
  onHiddenRefs?: (count: number) => void;
}) {
  const graph = "edges" in layout ? layout as GitGraphLayout : null;
  const merges = useMemo(() => project.git ? createGitMergeIndex(project.git, graph?.backboneOids) : new Map(), [project.git, graph]);
  const returnCount = useMemo(() => graph?.edges.filter((edge) => edge.backboneReturn).length ?? 0, [graph]);
  const decorations = useMemo(() => {
    if (graph) return { refs: graph.refsByOid, checkouts: graph.checkoutsByOid };
    const refs = new Map<string, NonNullable<ViewProject["git"]>["refs"]>(), checkouts = new Map<string, NonNullable<ViewProject["git"]>["checkouts"]>();
    for (const ref of project.git?.refs ?? []) if (ref.commit_oid) { const values = refs.get(ref.commit_oid) ?? []; values.push(ref); refs.set(ref.commit_oid, values); }
    for (const checkout of project.git?.checkouts ?? []) if (checkout.head) { const values = checkouts.get(checkout.head) ?? []; values.push(checkout); checkouts.set(checkout.head, values); }
    return { refs, checkouts };
  }, [graph, project.git]);
  const familyColors = useMemo(() => {
    const indexed = new Map<string, string>();
    for (const path of graph?.paths ?? []) {
      let hash = 0; for (const character of path.id) hash = (hash * 31 + character.charCodeAt(0)) >>> 0;
      for (const oid of path.oids) indexed.set(oid, colors[hash % colors.length]);
    }
    return indexed;
  }, [graph]);
  const included = (oid: string) => !visibleOids || visibleOids.has(oid);
  const navigable = useMemo(() => visibleOids ? layout.marks.filter((mark) => visibleOids.has(mark.commit.oid) || mark.range?.commits.some((commit) => visibleOids.has(commit.oid))) : layout.marks, [layout, visibleOids]);
  const spatial = useMemo(() => graph ? createGraphSpatialIndex(visibleOids ? { ...graph,
    marks: graph.marks.filter((mark) => visibleOids.has(mark.commit.oid) || mark.range?.commits.some((commit) => visibleOids.has(commit.oid))),
    edges: graph.edges.filter((edge) => visibleOids.has(edge.childOid)),
  } : graph) : null, [graph, visibleOids]);
  const rangeControls = useMemo(() => layout.ranges.flatMap((range) => {
    if (visibleOids && !range.commits.some((commit) => visibleOids.has(commit.oid))) return [];
    let x = range.x * zoom - left;
    const y = range.y * zoom - top;
    const laneClearance = zoom * GIT_LANE_SPACING;
    if (laneClearance < 16) return []; // Canvas/keyboard retain low-zoom ranges in both views.
    const normalWidth = Math.min(width, range.expanded ? 156 : Math.max(28, Math.min(304, gitRangeWidth(range.commits.length, 1) * zoom)));
    // Match the actual control's reach: tiny 26 px, narrow 52 px, wide 38 px;
    // expanded controls also sit 38 px above their lane. Keep a 4 px gap.
    const requiredClearance = range.expanded ? 68 : normalWidth < 68 ? 30 : normalWidth < 304 ? 56 : 42;
    const compactControl = laneClearance < requiredClearance;
    const compactHeight = laneClearance < 30 ? 14 : 26;
    let buttonWidth = compactControl ? Math.min(width, Math.max(28, gitRangeWidth(range.commits.length, 1) * zoom)) : normalWidth;
    if (range.expanded && compactControl) {
      // Keep both revealed commits' 24 px pick radii clear. This control lives
      // between actual marks, never on a commit or in an adjacent lane.
      const next = layout.byOid.get(range.commits[1]?.oid ?? "");
      const gap = next ? (next.x - range.x) * zoom : 0;
      const available = gap - 48;
      if (available < 28) return []; // Global Collapse linear remains available.
      x += gap / 2;
      buttonWidth = Math.min(width, available, Math.max(28, String(range.commits.length).length * 6 + 12));
    }
    if (x + buttonWidth / 2 < 0 || x - buttonWidth / 2 > width || y < 0 || y > height) return [];
    const narrow = !range.expanded && buttonWidth < 304;
    const compact = !range.expanded && buttonWidth < 144;
    const tiny = compactControl || (!range.expanded && buttonWidth < 68);
    const offset = compactControl ? compactHeight / 2 : range.expanded ? 38 : tiny ? 13 : narrow ? 26 : 19;
    const buttonHeight = compactControl ? compactHeight : range.expanded || tiny ? 26 : narrow ? 52 : 38;
    return [{ range, compactControl, narrow, compact, tiny, rect: {
      x: x - buttonWidth / 2, y: y - offset, width: buttonWidth, height: buttonHeight,
    } }];
  }), [layout, visibleOids, fixed, zoom, left, top, width, height]);
  const markColor = (mark: GitLayout["marks"][number]) => familyColors.get(mark.commit.oid) ?? colors[mark.lane % colors.length];
  const ref = useRef<HTMLCanvasElement>(null),
    [hover, setHover] = useState("");
  const labelBoxes = useRef<GitPlacedLabel[]>([]);
  const [themeRevision, setThemeRevision] = useState(0);
  useEffect(() => {
    const root = ref.current?.closest(".app");
    if (!root) return;
    const observer = new MutationObserver(() => setThemeRevision((revision) => revision + 1));
    observer.observe(root, { attributes: true, attributeFilter: ["class"] });
    return () => observer.disconnect();
  }, []);
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
    const backingWidth = Math.ceil(width * ratio), backingHeight = Math.ceil(height * ratio);
    if (canvas.width !== backingWidth) canvas.width = backingWidth;
    if (canvas.height !== backingHeight) canvas.height = backingHeight;
    const ctx = canvas.getContext("2d");
    if (!ctx) return;
    ctx.setTransform(ratio, 0, 0, ratio, 0, 0);
    ctx.clearRect(0, 0, width, height);
    const styles = getComputedStyle(canvas),
      text = styles.getPropertyValue("--lv-text").trim() || "#dce5ef",
      muted = styles.getPropertyValue("--lv-muted").trim() || "#98a8bb",
      line = styles.getPropertyValue("--lv-line").trim() || "#273446";
    ctx.strokeStyle = line;
    ctx.lineWidth = 0.6;
    ctx.setLineDash([2, 6]);
    const gridStep = Math.max(12, 72 * zoom);
    for (let x = 32 - (left % gridStep); x < width; x += gridStep) {
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
    const camera = { k: zoom, x: -left, y: -top }, viewport = { width, height };
    const visibleMarks = spatial?.visibleMarks(camera, viewport) ?? layout.marks;
    const edges = spatial?.visibleEdges(camera, viewport) ?? graph?.edges ?? layout.marks.flatMap((child) => child.commit.parents.map((parentOid, parentIndex): GraphEdge => ({ id: `${child.commit.oid}:${parentIndex}`, child, childOid: child.commit.oid, parent: layout.byOid.get(parentOid) ?? null, parentOid, parentIndex, kind: "parent" })));
    for (const edge of edges) {
      if (!included(edge.childOid)) continue;
      const m = edge.child, p = edge.parent, c = screen(m);
      if (p === m) continue; // Compression aliases retain source edges, not loops.
      if (!p) {
        if (c.x > -60 && c.x < width + 60 && c.y > -40 && c.y < height + 40) {
          ctx.strokeStyle = muted; ctx.setLineDash([3, 4]); ctx.lineWidth = 1.5;
          ctx.beginPath(); ctx.moveTo(c.x - 28, c.y); ctx.lineTo(c.x - 8, c.y); ctx.stroke(); ctx.setLineDash([]);
          ctx.font = "11px ui-monospace,monospace"; ctx.fillStyle = muted; ctx.fillText("…", c.x - 40, c.y + 4);
        }
        continue;
      }
      const curve = parentCurve(edge);
      if (!curve) continue;
      const a = screen(curve.from), first = screen(curve.control1), second = screen(curve.control2);
      ctx.strokeStyle = edge.kind === "backbone" ? text : markColor(edge.parentIndex > 0 ? p : m);
      ctx.globalAlpha = edge.kind === "backbone" || edge.backboneReturn ? 1 : .65;
      ctx.lineWidth = edge.kind === "backbone" ? 3.5 : edge.backboneReturn ? 2.7 : edge.parentIndex > 0 ? 2 : 1.7;
      ctx.beginPath(); ctx.moveTo(a.x, a.y); ctx.bezierCurveTo(first.x, first.y, second.x, second.y, c.x, c.y); ctx.stroke();
      if (edge.backboneReturn && zoom >= .2) {
        ctx.fillStyle = ctx.strokeStyle;
        ctx.beginPath(); ctx.moveTo(c.x, c.y - 8); ctx.lineTo(c.x - 4, c.y - 15); ctx.lineTo(c.x + 4, c.y - 15); ctx.closePath(); ctx.fill();
      }
      ctx.globalAlpha = 1;
    }
    const labels: GitLabelCandidate[] = [];
    for (const m of visibleMarks) {
      if (!included(m.commit.oid) && !m.range?.commits.some((commit) => included(commit.oid))) continue;
      const c = screen(m);
      if (!spatial && (c.x < -40 || c.x > width + 40 || c.y < -40 || c.y > height + 40)) continue;
      if (m.range) {
        const span = gitRangeWidth(m.range.commits.length, 1) * zoom;
        ctx.strokeStyle = markColor(m);
        ctx.lineWidth = 2;
        ctx.beginPath(); ctx.moveTo(c.x - span / 2, c.y); ctx.lineTo(c.x + span / 2, c.y); ctx.stroke();
        continue;
      }
      const color = graph?.backboneOids.has(m.commit.oid) ? text : markColor(m);
      ctx.fillStyle = color;
      ctx.beginPath();
      ctx.arc(c.x, c.y, m.commit.parents.length > 1 ? 6 : 4, 0, Math.PI * 2);
      ctx.fill();
      if (merges.has(m.commit.oid)) {
        ctx.strokeStyle = text; ctx.lineWidth = 1.5;
        ctx.beginPath(); ctx.arc(c.x, c.y, 8, 0, Math.PI * 2); ctx.stroke();
      }
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
        decorations.refs.get(m.commit.oid) ?? [];
      if (refs.length) {
        const names = refs.map((r) => r.name).sort((a, b) => Number(b === graph?.backbone.refName) - Number(a === graph?.backbone.refName) || a.localeCompare(b));
        labels.push({ id: m.commit.oid, x: c.x, y: c.y, names, color,
          priority: selected === m.commit.oid ? 3 : names.includes(graph?.backbone.refName ?? "") ? 2 : decorations.checkouts.has(m.commit.oid) ? 1 : 0 });
      }
      const trees =
        decorations.checkouts.get(m.commit.oid) ?? [];
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
    ctx.font = "600 11px ui-monospace,monospace";
    const inset = fixed ? Math.min(safeInset, width / 4, height / 4) : 0;
    const placed = placeGitLabels(labels, { width, height }, inset, (value) => ctx.measureText(value).width, 200, rangeControls.map((control) => control.rect));
    labelBoxes.current = placed.labels;
    canvas.dataset.refLabelBoxes = JSON.stringify(placed.labels.map(({ x, y, width, height }) => ({ x, y, width, height })));
    // Offscreen refs also remain available in the complete ref selector.
    const allRefs = project.git?.refs.length ?? 0;
    const hidden = Math.max(0, allRefs - placed.labels.reduce((count, label) => count + label.refCount, 0));
    canvas.dataset.hiddenRefCount = String(hidden);
    onHiddenRefs?.(hidden);
    for (const label of placed.labels) {
      ctx.fillStyle = line;
      ctx.beginPath();
      ctx.roundRect(label.x, label.y, label.width, label.height, 4);
      ctx.fill();
      ctx.fillStyle = label.color;
      ctx.fillText(label.text, label.x + 8, label.y + 14);
    }
  }, [project, layout, zoom, left, top, width, height, selected, fixed, visibleOids, decorations, familyColors, graph, spatial, safeInset, themeRevision, onHiddenRefs, merges, rangeControls]);
  const nearest = (x: number, y: number) => {
    if (spatial) return spatial.hitTest({ k: zoom, x: -left, y: -top }, { x, y });
    let distance = 24,
      found: null | (typeof layout.marks)[number] = null;
    for (const m of layout.marks) {
      if (!included(m.commit.oid) && !m.range?.commits.some((commit) => included(commit.oid))) continue;
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
        style={{ left: fixed ? 0 : left, top: fixed ? 0 : top + 48, width, height }}
        className="git-canvas"
        tabIndex={0}
        role="img"
        data-backbone-ref={graph?.backbone.refName}
        data-backbone-oid={graph?.backbone.tipOid}
        data-backbone-y={graph?.byOid.get(graph.backbone.tipOid ?? "")?.y}
        data-zoom={zoom}
        data-backbone-return-count={returnCount}
        aria-label={`${project.name}: ${layout.orderedCommits.length} commits, ${layout.ranges.filter((r) => !r.expanded).length} collapsed ranges. Arrow keys navigate ancestry; Enter inspects. Full details available in Commit inventory.`}
        onPointerMove={(e) => {
          const r = e.currentTarget.getBoundingClientRect(),
            x = e.clientX - r.left, y = e.clientY - r.top,
            label = labelBoxes.current.find((box) => x >= box.x && x <= box.x + box.width && y >= box.y && y <= box.y + box.height),
            mark = nearest(x, y);
          setHover(
            label ? label.fullText : mark
              ? `${merges.has(mark.commit.oid) ? `Actual ${merges.get(mark.commit.oid)!.parentCount}-parent merge · ` : ""}${mark.commit.oid.slice(0, 10)} · ${mark.commit.subject}${decorations.refs.get(mark.commit.oid)?.map((r) => ` · ${shortRef(r.name)}`).join("") ?? ""}`
              : "",
          );
        }}
        onPointerLeave={() => setHover("")}
        onClick={(e) => {
          const r = e.currentTarget.getBoundingClientRect(),
            mark = nearest(e.clientX - r.left, e.clientY - r.top);
          if (mark?.range) onToggleRange(mark.range);
          else if (mark) onSelect({ kind: "commit", project, commit: mark.commit });
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
          const selectedMark = layout.byOid.get(selected ?? "");
          const current = selectedMark && navigable.includes(selectedMark) ? selectedMark : navigable.at(-1);
          if (!current) return;
          let next = current;
          if (e.key === "ArrowLeft")
            next = layout.byOid.get(current.commit.parents[0]) ?? current;
          if (e.key === "ArrowRight")
            next =
              navigable.find((m) =>
                m.commit.parents.includes(current.commit.oid),
              ) ?? current;
          if (e.key === "ArrowUp" || e.key === "ArrowDown") {
            const index = navigable.indexOf(current);
            next =
              navigable[
                Math.max(
                  0,
                  Math.min(
                    navigable.length - 1,
                    index + (e.key === "ArrowUp" ? -1 : 1),
                  ),
                )
              ];
          }
          if (next.range) onToggleRange(next.range);
          else onSelect({ kind: "commit", project, commit: next.commit });
        }}
      />
      {rangeControls.map(({ range, compactControl, narrow, compact, tiny, rect }) => {
        const first = range.commits[0].oid.slice(0, 7);
        const last = range.commits.at(-1)!.oid.slice(0, 7);
        return (
          <button
            key={range.id}
            data-anchor-oid={range.commits[0].oid}
            data-world-x={layout.byOid.get(range.commits[0].oid)?.x}
            data-world-y={layout.byOid.get(range.commits[0].oid)?.y}
            className={`git-commit-range ${range.expanded ? "is-expanded" : ""} ${narrow ? "is-narrow" : ""} ${compact ? "is-compact" : ""} ${tiny ? "is-tiny" : ""}`}
            style={{ left: (fixed ? 0 : left) + rect.x, top: (fixed ? 0 : top + 48) + rect.y, width: rect.width, height: rect.height, ...(compactControl ? {padding:"0 3px",fontSize:9} : {}) }}
            aria-expanded={range.expanded}
            aria-label={range.expanded ? `− Collapse ${range.commits.length} commits` : `${first} → ${last} + ${range.commits.length} hidden. Expand commits`}
            title={`${range.commits[0].oid} → ${range.commits.at(-1)!.oid} · ${range.commits.length} linear commits. ${range.expanded ? "Collapse range" : "Expand range"}.`}
            onClick={() => onToggleRange(range)}
          >
            {compactControl ? <span>{range.expanded ? "−" : "+"}{!range.expanded || rect.width >= String(range.commits.length).length * 6 + 12 ? range.commits.length : ""}</span> : range.expanded ? (
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
          style={{ left: (fixed ? 0 : left) + 12, top: (fixed ? 0 : top) + height - 30 }}
        >
          {hover}
        </div>
      )}
    </>
  );
}
