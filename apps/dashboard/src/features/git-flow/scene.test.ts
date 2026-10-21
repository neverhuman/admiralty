import { describe, expect, it } from "vitest";
import type { CommitMark } from "../live-views/geometry";
import type { GitGraphLayout, GitRepository } from "./model";
import { layoutGitGraph } from "./layout";
import { createGraphSpatialIndex, loadedAncestry } from "./scene";

const oid = (n: number) => n.toString(16).padStart(40, "0");
type Row = [id: number, x: number, y: number, parents: number[]];
function positions(rows: Row[]): GitGraphLayout {
  const marks: CommitMark[] = rows.map(([id, x, y, parents], rank) => ({
    x, y, lane: y === 0 ? 0 : 1, rank,
    commit: { oid: oid(id), parents: parents.map(oid), subject: `Commit ${id}`, committed_utc: "2026-10-21T00:00:00Z" },
  }));
  const byOid = new Map(marks.map((mark) => [mark.commit.oid, mark]));
  const edges: GitGraphLayout["edges"] = marks.flatMap((child) => child.commit.parents.map((parentOid, parentIndex) => ({
    id: `${child.commit.oid}:${parentIndex}`, parentOid, childOid: child.commit.oid, parent: byOid.get(parentOid) ?? null, child, parentIndex,
    kind: byOid.has(parentOid) ? parentIndex ? "merge-parent" : "parent" : "missing",
  })));
  return { marks, byOid, orderedCommits: marks.map((mark) => mark.commit), edges, ranges: [], paths: [], boundaries: edges.filter((edge) => !edge.parent).length,
    width: 1000, height: 1000, bounds: { minX: 0, minY: -1000, maxX: 1000, maxY: 0 },
    backbone: { kind: "unresolved", source: null, refName: null, tipOid: null, spine: [], missingParentOid: null, reason: "Spatial fixture" },
    backboneOids: new Set(), refsByOid: new Map(), checkoutsByOid: new Map() };
}
const camera = { k: 1, x: 0, y: 200 }, viewport = { width: 200, height: 200 };

describe("Git graph spatial index", () => {
  it("culls by both coordinates while preserving negative worldY and CSS margins", () => {
    const layout = positions([[1, 50, -150, []], [2, 300, -150, []], [3, 50, -500, []], [4, -20, -100, []]]), index = createGraphSpatialIndex(layout);
    expect(index.visibleMarks(camera, viewport, 0).map((mark) => mark.commit.oid)).toEqual([oid(1)]);
    expect(index.visibleMarks(camera, viewport).map((mark) => mark.commit.oid)).toEqual([oid(4), oid(1)]);
    expect(index.visibleMarks({ ...camera, y: 500 }, viewport, 0).map((mark) => mark.commit.oid)).toEqual([oid(3)]);
  });

  it("includes crossing actual edges even when both endpoint marks are offscreen", () => {
    const layout = positions([[1, -1000, -100, []], [2, 1000, -100, [1]], [3, -1000, -600, []], [4, 1000, -600, [3]]]), index = createGraphSpatialIndex(layout);
    expect(index.visibleMarks(camera, viewport, 0)).toEqual([]);
    expect(index.visibleEdges(camera, viewport, 0).map((edge) => edge.childOid)).toEqual([oid(2)]);
    expect(index.visibleEdges({ ...camera, y: 700 }, viewport, 0).map((edge) => edge.childOid)).toEqual([oid(4)]);
  });

  it("uses conservative cubic control hulls and missing-parent stub extents", () => {
    const layout = positions([[1, 100, -100, []], [2, 105, -100, [1]], [3, 230, -100, [99]]]), index = createGraphSpatialIndex(layout);
    // The minimum20world bend extends the short connector beyond its endpoints.
    expect(index.visibleEdges({ k: 1, x: -112, y: 200 }, { width: 5, height: 200 }, 0).some((edge) => edge.childOid === oid(2))).toBe(true);
    expect(index.visibleEdges(camera, viewport, 0).some((edge) => edge.kind === "missing" && edge.parentOid === oid(99))).toBe(true);
    expect(index.visibleEdges(camera, viewport, 0).every((edge) => layout.edges.includes(edge))).toBe(true);
  });

  it("keeps a collapsed range visible when only its envelope enters the viewport", () => {
    const layout = positions([[1, 300, -100, []]]), mark = layout.marks[0];
    mark.range = { id: "range", commits: [mark.commit, { ...mark.commit, oid: oid(2) }], expanded: false, x: mark.x, y: mark.y };
    expect(createGraphSpatialIndex(layout).visibleMarks(camera, viewport, 0)).toEqual([mark]);
  });

  it("uses constant CSS hit radii at fitted scale and4x, with stable OID tie-breaks", () => {
    const layout = positions([[2, 1000, -1000, []], [1, 1020, -1000, []]]), index = createGraphSpatialIndex(layout);
    const fit = { k: 0.1, x: 0, y: 200 };
    expect(index.hitTest(fit, { x: 101, y: 100 })?.commit.oid).toBe(oid(1));
    expect(index.hitTest(fit, { x: 100, y: 123 })?.commit.oid).toBe(oid(2));
    expect(index.hitTest(fit, { x: 100, y: 125 })).toBeNull();
    const enlarged = { k: 4, x: -4000, y: 4100 };
    expect(index.hitTest(enlarged, { x: 0, y: 123 })?.commit.oid).toBe(oid(2));
    expect(index.hitTest(enlarged, { x: 0, y: 125 })).toBeNull();
    expect(index.hitTest(enlarged, { x: 0, y: 100 }, 0)?.commit.oid).toBe(oid(2));
  });

  it("ignores compressed self aliases for painting without dropping source evidence", () => {
    const repo: GitRepository = { instance_id: "compressed", node: "test", observed_utc: "2026-10-21T00:00:00Z", shallow: false, truncated: false, refs: [], checkouts: [],
      commits: Array.from({ length: 12 }, (_, i) => ({ oid: oid(i), parents: i ? [oid(i - 1)] : [], subject: "Range fixture", committed_utc: "2026-10-21T00:00:00Z" })) };
    const layout = layoutGitGraph(repo), sourceEdges = [...layout.edges], index = createGraphSpatialIndex(layout);
    expect(layout.marks).toHaveLength(3);
    expect(layout.edges).toHaveLength(11);
    const visible = index.visibleEdges({ k: 1, x: 0, y: 200 }, { width: 1000, height: 400 });
    expect(visible).toHaveLength(2);
    expect(visible.every((edge) => edge.parent !== edge.child)).toBe(true);
    expect(layout.edges).toEqual(sourceEdges);
    expect(loadedAncestry(layout, oid(11)).size).toBe(12);
  });

  it("returns deterministic queries after shuffled source arrays and handles empty panes", () => {
    const rows: Row[] = [[3, 200, -100, [1, 2]], [2, 100, -150, []], [1, 100, -50, []]];
    const first = createGraphSpatialIndex(positions(rows)), second = createGraphSpatialIndex(positions([...rows].reverse()));
    expect(first.visibleMarks(camera, viewport).map((mark) => mark.commit.oid)).toEqual(second.visibleMarks(camera, viewport).map((mark) => mark.commit.oid));
    expect(first.visibleEdges(camera, viewport).map((edge) => edge.id)).toEqual(second.visibleEdges(camera, viewport).map((edge) => edge.id));
    expect(first.hitTest(camera, { x: 100, y: 100 }, 60)?.commit.oid).toBe(second.hitTest(camera, { x: 100, y: 100 }, 60)?.commit.oid);
    expect(first.visibleMarks(camera, { width: 0, height: 100 })).toEqual([]);
    expect(first.visibleEdges({ ...camera, k: 0 }, viewport)).toEqual([]);
    expect(first.hitTest({ ...camera, k: NaN }, { x: 0, y: 0 })).toBeNull();
    expect(createGraphSpatialIndex(positions([])).visibleEdges(camera, viewport)).toEqual([]);
  });

  it("examines bounded candidates in sparse100000-mark queries", () => {
    const layout = positions(Array.from({ length: 100000 }, (_, i) => [i, i * 112, -54, i ? [i - 1] : []] as Row));
    const index = createGraphSpatialIndex(layout), view = { k: 1, x: -50000 * 112, y: 100 };
    expect(index.visibleMarks(view, { width: 800, height: 200 }).length).toBeGreaterThan(0);
    expect(index.queryStats.markCandidates).toBeLessThan(20);
    expect(index.visibleEdges(view, { width: 800, height: 200 }).length).toBeGreaterThan(0);
    expect(index.queryStats.edgeCandidates).toBeLessThan(20);
    expect(index.hitTest(view, { x: 112, y: 46 })?.commit.oid).toBe(oid(50001));
    expect(index.queryStats.hitCandidates).toBeLessThan(4);
  }, 10000);
});

describe("loaded Git ancestry", () => {
  it("includes all actual octopus/merge parents once and excludes missing objects", () => {
    const layout = positions([[1, 0, 0, []], [2, 0, 0, [1]], [3, 0, 0, [1]], [4, 0, 0, [1]], [5, 0, 0, [2, 3, 4, 99]], [6, 0, 0, []]]);
    expect([...loadedAncestry(layout, oid(5))].sort()).toEqual([1, 2, 3, 4, 5].map(oid));
    expect([...loadedAncestry(layout, oid(1))]).toEqual([oid(1)]);
    expect([...loadedAncestry(layout, oid(99))]).toEqual([]);
    expect([...loadedAncestry(positions([]), oid(1))]).toEqual([]);
  });

  it("walks long loaded ancestry iteratively without following absent boundaries", () => {
    const layout = positions(Array.from({ length: 20000 }, (_, i) => [i, i, 0, i ? [i - 1] : [30000]] as Row));
    const result = loadedAncestry(layout, oid(19999));
    expect(result.size).toBe(20000);
    expect(result.has(oid(30000))).toBe(false);
  });
});
