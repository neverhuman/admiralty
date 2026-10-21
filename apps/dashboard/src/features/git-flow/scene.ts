import type { Camera, CommitMark, Point } from "../live-views/geometry";
import type { GitCommit, GitGraphLayout, GraphEdge } from "./model";
import { curveBounds, parentCurve } from "./curves";

type Viewport = { width: number; height: number };
type Extent = { left: number; top: number; right: number; bottom: number };
type EdgeHull = Extent & { edge: GraphEdge };
const compare = (a: string, b: string) => a < b ? -1 : a > b ? 1 : 0;
const validCamera = (camera: Camera) => Number.isFinite(camera.k) && camera.k > 0 && Number.isFinite(camera.x) && Number.isFinite(camera.y);
const padding = (margin: number) => Number.isFinite(margin) ? Math.max(0, margin) : 48;

function lowerBound<T>(values: readonly T[], x: number, get: (value: T) => number) {
  let low = 0, high = values.length;
  while (low < high) {
    const middle = (low + high) >>> 1;
    if (get(values[middle]) < x) low = middle + 1;
    else high = middle;
  }
  return low;
}
function upperBound<T>(values: readonly T[], x: number, get: (value: T) => number) {
  let low = 0, high = values.length;
  while (low < high) {
    const middle = (low + high) >>> 1;
    if (get(values[middle]) <= x) low = middle + 1;
    else high = middle;
  }
  return low;
}

function edgeHull(edge: GraphEdge): EdgeHull {
  const child = edge.child, parent = edge.parent;
  if (!parent) return { edge, left: child.x - 40, right: child.x - 8, top: child.y - 4, bottom: child.y + 4 };
  const hull = curveBounds(parentCurve(edge)!);
  return { edge, left: hull.minX, right: hull.maxX, top: hull.minY, bottom: hull.maxY };
}

/** Static indexes are tied to one immutable layout generation. Camera movement
 * queries them without rebuilding topology or scanning every edge.
 */
export function createGraphSpatialIndex(layout: GitGraphLayout) {
  const marks = [...layout.marks].sort((a, b) => a.x - b.x || compare(a.commit.oid, b.commit.oid));
  const maximumHalfWidth = marks.some((mark) => !!mark.range) ? 152 : 0;
  const hulls = layout.edges.filter((edge) => edge.parent !== edge.child).map(edgeHull)
    .sort((a, b) => a.left - b.left || compare(a.edge.id, b.edge.id));
  let size = 1;
  while (size < hulls.length) size *= 2;
  const maximumRight = new Float64Array(size * 2).fill(-Infinity);
  for (let at = 0; at < hulls.length; at++) maximumRight[size + at] = hulls[at].right;
  for (let node = size - 1; node; node--) maximumRight[node] = Math.max(maximumRight[node * 2], maximumRight[node * 2 + 1]);
  const stats = { markCandidates: 0, edgeCandidates: 0, hitCandidates: 0 };
  const worldViewport = (camera: Camera, viewport: Viewport, margin: number): Extent | null => {
    if (!validCamera(camera) || !Number.isFinite(viewport.width) || !Number.isFinite(viewport.height) || viewport.width <= 0 || viewport.height <= 0) return null;
    const amount = padding(margin);
    return { left: (-amount - camera.x) / camera.k, right: (viewport.width + amount - camera.x) / camera.k,
      top: (-amount - camera.y) / camera.k, bottom: (viewport.height + amount - camera.y) / camera.k };
  };
  return {
    /** Last-query counts expose deterministic candidate budgets, not timings. */
    get queryStats() { return { ...stats }; },
    visibleMarks(camera: Camera, viewport: Viewport, margin = 48): CommitMark[] {
      stats.markCandidates = 0;
      const area = worldViewport(camera, viewport, padding(margin) + 8);
      if (!area) return [];
      const first = lowerBound(marks, area.left - maximumHalfWidth, (mark) => mark.x);
      const end = upperBound(marks, area.right + maximumHalfWidth, (mark) => mark.x);
      const visible: CommitMark[] = [];
      for (let at = first; at < end; at++) {
        const mark = marks[at], halfWidth = mark.range ? 152 : 0, halfHeight = mark.range ? 20 : 0;
        stats.markCandidates++;
        if (mark.x + halfWidth >= area.left && mark.x - halfWidth <= area.right && mark.y + halfHeight >= area.top && mark.y - halfHeight <= area.bottom) visible.push(mark);
      }
      return visible;
    },
    visibleEdges(camera: Camera, viewport: Viewport, margin = 48): GraphEdge[] {
      stats.edgeCandidates = 0;
      const area = worldViewport(camera, viewport, padding(margin) + 2);
      if (!area || !hulls.length) return [];
      const end = upperBound(hulls, area.right, (hull) => hull.left);
      const visible: GraphEdge[] = [];
      const stack = [{ node: 1, left: 0, right: size }];
      while (stack.length) {
        const current = stack.pop()!;
        if (current.left >= end || maximumRight[current.node] < area.left) continue;
        if (current.right - current.left === 1) {
          const hull = hulls[current.left];
          stats.edgeCandidates++;
          if (hull.top <= area.bottom && hull.bottom >= area.top) visible.push(hull.edge);
        } else {
          const middle = (current.left + current.right) >>> 1;
          stack.push({ node: current.node * 2 + 1, left: middle, right: current.right });
          stack.push({ node: current.node * 2, left: current.left, right: middle });
        }
      }
      return visible;
    },
    hitTest(camera: Camera, screen: Point, radius = 24): CommitMark | null {
      stats.hitCandidates = 0;
      if (!validCamera(camera) || !Number.isFinite(screen.x) || !Number.isFinite(screen.y) || !Number.isFinite(radius) || radius < 0) return null;
      const centerX = (screen.x - camera.x) / camera.k, reach = radius / camera.k;
      const first = lowerBound(marks, centerX - reach, (mark) => mark.x), end = upperBound(marks, centerX + reach, (mark) => mark.x);
      let nearest: CommitMark | null = null, distance = radius * radius;
      for (let at = first; at < end; at++) {
        const mark = marks[at];
        stats.hitCandidates++;
        const dx = mark.x * camera.k + camera.x - screen.x, dy = mark.y * camera.k + camera.y - screen.y;
        const next = dx * dx + dy * dy;
        if (next < distance || (next === distance && (!nearest || compare(mark.commit.oid, nearest.commit.oid) < 0))) {
          nearest = mark; distance = next;
        }
      }
      return nearest;
    },
  };
}

const ancestryIndexes = new WeakMap<GitGraphLayout, Map<string, GitCommit>>();

/** Loaded reachability closure, including the requested commit itself. Parent
 * lists come from source commits, never compressed aliases or inferred landings.
 */
export function loadedAncestry(layout: GitGraphLayout, oid: string): ReadonlySet<string> {
  let commits = ancestryIndexes.get(layout);
  if (!commits) {
    commits = new Map(layout.orderedCommits.map((commit) => [commit.oid, commit]));
    ancestryIndexes.set(layout, commits);
  }
  const result = new Set<string>();
  if (!commits.has(oid)) return result;
  const pending = [oid];
  while (pending.length) {
    const next = pending.pop()!;
    if (result.has(next)) continue;
    const commit = commits.get(next);
    if (!commit) continue;
    result.add(next);
    for (const parent of commit.parents) if (!result.has(parent) && commits.has(parent)) pending.push(parent);
  }
  return result;
}
