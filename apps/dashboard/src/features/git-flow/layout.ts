import type { CommitMark, CommitRange, GitCommit, GitGraphLayout, GitPath, GitRepository, GraphBounds, GraphLayoutOptions } from "./model";
import { resolveBackbone } from "./backbone";
import { curveBounds, parentCurve } from "./curves";

const COLUMN = 112;
const LANE = 54;
const CLEARANCE = 1;
const oidPattern = /^(?:[a-f\d]{40}|[a-f\d]{64})$/i;
const compare = (a: string, b: string) => a < b ? -1 : a > b ? 1 : 0;
const rangeWidth = (count: number) => Math.max(24, Math.min(304, (count - 1) * COLUMN * 0.75));

class Heap<T> {
  private values: T[] = [];
  constructor(private readonly less: (a: T, b: T) => boolean) {}
  get size() { return this.values.length; }
  peek() { return this.values[0]; }
  push(value: T) {
    let at = this.values.length;
    this.values.push(value);
    while (at) {
      const parent = (at - 1) >> 1;
      if (!this.less(value, this.values[parent])) break;
      this.values[at] = this.values[parent];
      at = parent;
    }
    this.values[at] = value;
  }
  pop(): T | undefined {
    const first = this.values[0], last = this.values.pop();
    if (!this.values.length || last === undefined) return first;
    let at = 0;
    while (at * 2 + 1 < this.values.length) {
      let child = at * 2 + 1;
      if (child + 1 < this.values.length && this.less(this.values[child + 1], this.values[child])) child++;
      if (!this.less(this.values[child], last)) break;
      this.values[at] = this.values[child];
      at = child;
    }
    this.values[at] = last;
    return first;
  }
}

/** Each free lane holds its next reserved interval start (Infinity if none).
 * A maximum tree finds the first lane that can contain an entire path interval,
 * including future reservations, without scanning every concurrent lane.
 */
class AvailableLanes {
  private readonly size: number;
  private readonly tree: Float64Array;
  constructor(count: number) {
    let size = 1;
    while (size < count) size *= 2;
    this.size = size;
    this.tree = new Float64Array(size * 2).fill(-Infinity);
    for (let lane = 1; lane < count; lane++) this.tree[size + lane] = Infinity;
    for (let node = size - 1; node; node--) this.tree[node] = Math.max(this.tree[node * 2], this.tree[node * 2 + 1]);
  }
  set(lane: number, next: number) {
    let node = this.size + lane;
    this.tree[node] = next;
    while ((node >>= 1)) this.tree[node] = Math.max(this.tree[node * 2], this.tree[node * 2 + 1]);
  }
  get(lane: number) { return this.tree[this.size + lane]; }
  first(minimum: number, end: number): number {
    const find = (node: number, left: number, right: number): number => {
      if (right <= minimum || this.tree[node] <= end) return -1;
      if (right - left === 1) return left;
      const middle = (left + right) >> 1;
      const first = find(node * 2, left, middle);
      return first >= 0 ? first : find(node * 2 + 1, middle, right);
    };
    return find(1, 0, this.size);
  }
}

function index(repo: GitRepository) {
  const commits = new Map<string, GitCommit>();
  for (const commit of repo.commits) {
    if (!oidPattern.test(commit.oid) || commits.has(commit.oid)) throw new Error("Invalid or duplicate Git commit identity");
    if (!Array.isArray(commit.parents) || commit.parents.some((oid) => !oidPattern.test(oid)) || new Set(commit.parents).size !== commit.parents.length)
      throw new Error("Invalid Git parent list");
    commits.set(commit.oid, commit);
  }
  const children = new Map<string, string[]>(), firstChildren = new Map<string, string[]>();
  const indegree = new Map<string, number>();
  let boundaries = 0;
  for (const commit of commits.values()) {
    let loaded = 0;
    for (const [parentIndex, parent] of commit.parents.entries()) {
      if (!commits.has(parent)) { boundaries++; continue; }
      loaded++;
      const list = children.get(parent) ?? [];
      list.push(commit.oid); children.set(parent, list);
      if (parentIndex === 0) {
        const first = firstChildren.get(parent) ?? [];
        first.push(commit.oid); firstChildren.set(parent, first);
      }
    }
    indegree.set(commit.oid, loaded);
  }
  const refsByOid: GitGraphLayout["refsByOid"] = new Map();
  for (const ref of repo.refs) if (ref.commit_oid) {
    const refs = refsByOid.get(ref.commit_oid) ?? [];
    refs.push(ref); refsByOid.set(ref.commit_oid, refs);
  }
  const checkoutsByOid: GitGraphLayout["checkoutsByOid"] = new Map();
  for (const checkout of repo.checkouts) if (checkout.head) {
    const checkouts = checkoutsByOid.get(checkout.head) ?? [];
    checkouts.push(checkout); checkoutsByOid.set(checkout.head, checkouts);
  }
  return { commits, children, firstChildren, indegree, boundaries, refsByOid, checkoutsByOid };
}

/** Loaded ancestry only. Refs label observations; they never invent integrations. */
export function layoutGitGraph(repo: GitRepository, options: GraphLayoutOptions = {}): GitGraphLayout {
  const graph = index(repo), backbone = resolveBackbone(repo, options);
  const backboneOids = new Set(backbone.spine);
  const queue = new Heap<string>((a, b) => {
    const first = Number(backboneOids.has(a)), second = Number(backboneOids.has(b));
    return first !== second ? first > second : compare(a, b) < 0;
  });
  for (const [oid, degree] of graph.indegree) if (!degree) queue.push(oid);
  const orderedCommits: GitCommit[] = [], ranks = new Map<string, number>();
  while (queue.size) {
    const oid = queue.pop()!, commit = graph.commits.get(oid)!;
    let rank = 0;
    for (const parent of commit.parents) if (ranks.has(parent)) rank = Math.max(rank, ranks.get(parent)! + 1);
    ranks.set(oid, rank); orderedCommits.push(commit);
    for (const child of graph.children.get(oid) ?? []) {
      const degree = graph.indegree.get(child)! - 1;
      graph.indegree.set(child, degree);
      if (!degree) queue.push(child);
    }
  }
  if (orderedCommits.length !== graph.commits.size) throw new Error("Invalid cyclic Git graph");

  const owner = new Map<string, GitPath>(), paths: GitPath[] = [];
  if (backboneOids.size) {
    const oids = [...backbone.spine].reverse();
    const spine: GitPath = { id: `${repo.instance_id}:backbone:${backbone.refName}`, oids, lane: 0, startRank: ranks.get(oids[0])!, endRank: ranks.get(oids.at(-1)!)! };
    paths.push(spine);
    for (const oid of oids) owner.set(oid, spine);
  }
  const prefix = `${repo.instance_id}:path:`;
  const previous = new Map<string, GitPath>();
  for (const path of [...(options.previousPaths ?? [])].sort((a, b) => compare(a.id, b.id))) {
    if (!path.id.startsWith(prefix) || previous.has(path.id)) continue;
    previous.set(path.id, path);
    const oids = path.oids.filter((oid) => graph.commits.has(oid) && !backboneOids.has(oid));
    if (!oids.length || oids.some((oid, at) => owner.has(oid) || (at > 0 && graph.commits.get(oid)!.parents[0] !== oids[at - 1]))) continue;
    const surviving: GitPath = { id: path.id, oids: [...oids], lane: 0, startRank: 0, endRank: 0 };
    paths.push(surviving);
    for (const oid of oids) owner.set(oid, surviving);
  }
  const addTip = (tip: string) => {
    const chain: string[] = [];
    let next: string | undefined = tip;
    while (next && graph.commits.has(next) && !owner.has(next)) {
      chain.push(next);
      next = graph.commits.get(next)!.parents[0];
    }
    if (!chain.length) return;
    chain.reverse();
    const fork = next ? owner.get(next) : undefined;
    // Extend an existing segment on ordinary tip growth without changing its id.
    const extension = fork && !backboneOids.has(next!) && fork.oids.at(-1) === next && graph.firstChildren.get(next!)?.length === 1;
    let path: GitPath;
    if (extension) { path = fork; for (const oid of chain) path.oids.push(oid); }
    else {
      path = { id: `${prefix}${chain[0]}`, oids: chain, lane: -1, startRank: 0, endRank: 0 };
      paths.push(path);
    }
    for (const oid of chain) owner.set(oid, path);
  };
  // Ref identity and checkout identity, not commit time, choose shared ancestry
  // ownership. Previous ownership takes precedence when adopting a new snapshot.
  const tips: { oid: string; priority: number; name: string }[] = [];
  for (const checkout of repo.checkouts) if (checkout.head) tips.push({ oid: checkout.head, priority: 0, name: checkout.id });
  for (const ref of repo.refs) if (ref.commit_oid) tips.push({ oid: ref.commit_oid, priority: ref.name.startsWith("refs/heads/") ? 1 : ref.name.startsWith("refs/remotes/") ? 2 : 3, name: ref.name });
  tips.sort((a, b) => a.priority - b.priority || compare(a.name, b.name) || compare(a.oid, b.oid));
  for (const tip of tips) addTip(tip.oid);
  for (let at = orderedCommits.length - 1; at >= 0; at--) addTip(orderedCommits[at].oid);

  const topics = paths.filter((path) => path.lane !== 0 || !path.oids.some((oid) => backboneOids.has(oid)));
  for (const path of topics) {
    const first = graph.commits.get(path.oids[0])!;
    path.startRank = ranks.get(first.parents[0]) ?? ranks.get(first.oid)!;
    path.endRank = path.startRank;
    for (const oid of path.oids) {
      path.endRank = Math.max(path.endRank, ranks.get(oid)!);
      for (const child of graph.children.get(oid) ?? []) if (owner.get(child) !== path) path.endRank = Math.max(path.endRank, ranks.get(child)!);
    }
  }
  topics.sort((a, b) => a.startRank - b.startRank || ranks.get(a.oids[0])! - ranks.get(b.oids[0])! || compare(a.id, b.id));
  const reservation = new Map<string, number>(), reservations = new Map<number, GitPath[]>();
  const priorLimit = Math.max(options.previousPaths?.length ?? 0, paths.length) + 1;
  let maximumPriorLane = 0;
  for (const path of topics) {
    const lane = previous.get(path.id)?.lane;
    if (lane === undefined || !Number.isInteger(lane) || lane <= 0 || lane > priorLimit) continue;
    const list = reservations.get(lane) ?? [];
    if (list.length && list.at(-1)!.endRank + CLEARANCE >= path.startRank) continue;
    list.push(path); reservations.set(lane, list); reservation.set(path.id, lane);
    maximumPriorLane = Math.max(maximumPriorLane, lane);
  }
  const available = new AvailableLanes(maximumPriorLane + topics.length + 2);
  const reservationIndex = new Map<number, number>(), activeEnd = new Map<number, number>();
  for (const [lane, list] of reservations) available.set(lane, list[0].startRank);
  const active = new Heap<{ end: number; lane: number }>((a, b) => a.end < b.end || (a.end === b.end && a.lane < b.lane));
  const nextReserved = (lane: number) => reservations.get(lane)?.[reservationIndex.get(lane) ?? 0]?.startRank ?? Infinity;
  for (const path of topics) {
    while (active.size && active.peek().end < path.startRank) {
      const ended = active.pop()!;
      activeEnd.delete(ended.lane); available.set(ended.lane, nextReserved(ended.lane));
    }
    const desired = reservation.get(path.id);
    if (desired !== undefined) {
      reservationIndex.set(desired, (reservationIndex.get(desired) ?? 0) + 1);
      available.set(desired, activeEnd.has(desired) ? -Infinity : nextReserved(desired));
    }
    const parent = graph.commits.get(path.oids[0])!.parents[0];
    const minimum = Math.max(1, (owner.get(parent)?.lane ?? 0) + 1);
    const end = path.endRank + CLEARANCE;
    const lane = desired !== undefined && desired >= minimum && available.get(desired) > end ? desired : available.first(minimum, end);
    if (lane < 1) throw new Error("Git path lane allocation exhausted");
    path.lane = lane;
    activeEnd.set(lane, end); available.set(lane, -Infinity); active.push({ end, lane });
  }
  paths.sort((a, b) => a.startRank - b.startRank || a.lane - b.lane || compare(a.id, b.id));

  const eligible = (commit: GitCommit) => !graph.refsByOid.has(commit.oid) && !graph.checkoutsByOid.has(commit.oid) &&
    commit.parents.length === 1 && graph.commits.has(commit.parents[0]) && graph.children.get(commit.oid)?.length === 1;
  const membership = new Map<string, CommitRange>(), ranges: CommitRange[] = [];
  for (const commit of orderedCommits) {
    if (!eligible(commit) || membership.has(commit.oid)) continue;
    const chain = [commit];
    let next = graph.commits.get(graph.children.get(commit.oid)![0]);
    while (next && eligible(next) && owner.get(next.oid) === owner.get(commit.oid)) {
      chain.push(next); next = graph.commits.get(graph.children.get(next.oid)![0]);
    }
    if (chain.length < 2) continue;
    const range: CommitRange = { id: `${repo.instance_id}:${chain[0].oid}:${chain.at(-1)!.oid}`, commits: chain,
      expanded: chain.some((member) => options.expandedOids?.has(member.oid)) && !chain.some((member) => options.collapsedOids?.has(member.oid)), x: 0, y: 0 };
    ranges.push(range);
    for (const member of chain) membership.set(member.oid, range);
  }
  const marks: CommitMark[] = [], byOid = new Map<string, CommitMark>(), laneEnds = new Map<number, number>();
  const radius = (mark: CommitMark) => mark.range ? rangeWidth(mark.range.commits.length) / 2 : 0;
  for (const commit of orderedCommits) {
    const range = membership.get(commit.oid);
    if (range && !range.expanded && range.commits[0].oid !== commit.oid) { byOid.set(commit.oid, byOid.get(range.commits[0].oid)!); continue; }
    const lane = owner.get(commit.oid)!.lane;
    const mark: CommitMark = { commit, rank: ranks.get(commit.oid)!, lane, x: 0, y: lane === 0 ? 0 : -lane * LANE, range: range && !range.expanded ? range : undefined };
    const size = radius(mark);
    mark.x = Math.max(64 + size, (laneEnds.get(lane) ?? -48) + COLUMN + size);
    for (const oid of commit.parents) {
      const parent = byOid.get(oid);
      if (parent) mark.x = Math.max(mark.x, parent.x + radius(parent) + COLUMN + size);
    }
    if (range && range.commits[0].oid === commit.oid) { range.x = mark.x; range.y = mark.y; }
    laneEnds.set(lane, mark.x + size); marks.push(mark); byOid.set(commit.oid, mark);
  }
  const edges: GitGraphLayout["edges"] = [];
  for (const commit of orderedCommits) for (const [parentIndex, parentOid] of commit.parents.entries()) {
    const parent = byOid.get(parentOid) ?? null;
    edges.push({ id: `${repo.instance_id}:edge:${commit.oid}:${parentIndex}`, parentOid, childOid: commit.oid,
      parent, child: byOid.get(commit.oid)!, parentIndex,
      backboneReturn: !!parent && parentIndex > 0 && backboneOids.has(commit.oid),
      kind: !parent ? "missing" : parentIndex === 0 && backboneOids.has(commit.oid) && backboneOids.has(parentOid) ? "backbone" : parentIndex > 0 ? "merge-parent" : "parent" });
  }
  const bounds: GraphBounds = { minX: Infinity, minY: Infinity, maxX: -Infinity, maxY: -Infinity };
  const include = (x: number, y: number) => {
    bounds.minX = Math.min(bounds.minX, x); bounds.minY = Math.min(bounds.minY, y);
    bounds.maxX = Math.max(bounds.maxX, x); bounds.maxY = Math.max(bounds.maxY, y);
  };
  for (const mark of marks) {
    const size = Math.max(6, radius(mark));
    include(mark.x - size, mark.y - (mark.range ? 20 : 6)); include(mark.x + size, mark.y + (mark.range ? 20 : 6));
    if (graph.checkoutsByOid.has(mark.commit.oid)) { include(mark.x - 6, mark.y + 9); include(mark.x + 34, mark.y + 25); }
  }
  for (const range of ranges) if (range.expanded) { include(range.x - 78, range.y - 40); include(range.x + 78, range.y + 6); }
  for (const edge of edges) {
    if (!edge.parent) { include(edge.child.x - 40, edge.child.y - 4); include(edge.child.x - 8, edge.child.y + 4); }
    else {
      const curve = parentCurve(edge);
      if (curve) { const hull = curveBounds(curve); include(hull.minX - 2, hull.minY - 2); include(hull.maxX + 2, hull.maxY + 2); }
    }
  }
  if (!marks.length) Object.assign(bounds, { minX: 0, minY: 0, maxX: 0, maxY: 0 });
  return { marks, byOid, ranges, orderedCommits, boundaries: graph.boundaries,
    width: Math.max(680, bounds.maxX + 140), height: Math.max(180, bounds.maxY - bounds.minY + 100),
    bounds, backbone, backboneOids, paths, edges, refsByOid: graph.refsByOid, checkoutsByOid: graph.checkoutsByOid };
}
