import type {
  GitCommit,
  GitRepository,
  ViewAgent,
  ViewsSnapshot,
} from "./feed";
export type Point = { x: number; y: number };
export type Camera = Point & { k: number };
export type Box = Point & { width: number; height: number };
export function zoomAt(camera: Camera, point: Point, factor: number): Camera {
  const k = Math.max(0.12, Math.min(2, camera.k * factor));
  return {
    k,
    x: point.x - ((point.x - camera.x) * k) / camera.k,
    y: point.y - ((point.y - camera.y) * k) / camera.k,
  };
}
export function fitCamera(
  world: Box,
  viewport: { width: number; height: number },
  padding = 32,
): Camera {
  const k = Math.max(
    0.12,
    Math.min(
      1,
      (viewport.width - padding * 2) / world.width,
      (viewport.height - padding * 2) / world.height,
    ),
  );
  return {
    k,
    x: (viewport.width - world.width * k) / 2 - world.x * k,
    y: (viewport.height - world.height * k) / 2 - world.y * k,
  };
}
export function visible(
  box: Box,
  camera: Camera,
  width: number,
  height: number,
  margin = 120,
) {
  return (
    (box.x + box.width) * camera.k + camera.x >= -margin &&
    box.x * camera.k + camera.x <= width + margin &&
    (box.y + box.height) * camera.k + camera.y >= -margin &&
    box.y * camera.k + camera.y <= height + margin
  );
}
export type CommitMark = Point & {
  commit: GitCommit;
  lane: number;
  rank: number;
};
export type GitLayout = {
  marks: CommitMark[];
  byOid: Map<string, CommitMark>;
  width: number;
  height: number;
  boundaries: number;
};
/** Kahn's algorithm retains every loaded parent and ignores misleading commit dates. */
export function layoutGit(repo: GitRepository): GitLayout {
  const commits = new Map(repo.commits.map((c) => [c.oid, c]));
  const children = new Map<string, string[]>(),
    indegree = new Map<string, number>();
  let boundaries = 0;
  for (const c of repo.commits) {
    indegree.set(c.oid, c.parents.filter((p) => commits.has(p)).length);
    for (const p of c.parents)
      if (commits.has(p)) children.set(p, [...(children.get(p) ?? []), c.oid]);
      else boundaries++;
  }
  const queue = repo.commits
    .filter((c) => indegree.get(c.oid) === 0)
    .map((c) => c.oid)
    .sort();
  const ranks = new Map<string, number>(),
    marks: CommitMark[] = [],
    lanes = new Map<string, number>();
  const occupied = new Map<number, Set<number>>();
  while (queue.length) {
    const id = queue.shift()!,
      c = commits.get(id)!;
    const rank = Math.max(-1, ...c.parents.map((p) => ranks.get(p) ?? -1)) + 1;
    ranks.set(id, rank);
    const used = occupied.get(rank) ?? new Set<number>();
    let lane = c.parents.length ? (lanes.get(c.parents[0]) ?? 0) : 0;
    while (used.has(lane)) lane++;
    used.add(lane);
    occupied.set(rank, used);
    lanes.set(id, lane);
    marks.push({ commit: c, rank, lane, x: 64 + rank * 72, y: 44 + lane * 40 });
    for (const child of (children.get(id) ?? []).sort()) {
      const degree = indegree.get(child)! - 1;
      indegree.set(child, degree);
      if (!degree) {
        queue.push(child);
        queue.sort();
      }
    }
  }
  if (marks.length !== commits.size)
    throw new Error("Invalid cyclic Git graph");
  const maxRank = Math.max(0, ...marks.map((m) => m.rank));
  return {
    marks,
    byOid: new Map(marks.map((m) => [m.commit.oid, m])),
    width: Math.max(680, 200 + maxRank * 72),
    height: Math.max(180, 100 + Math.max(0, ...marks.map((m) => m.lane)) * 40),
    boundaries,
  };
}
export type AtlasNode = Box & { agent: ViewAgent };
export type AtlasBoard = Box & { id: string; name: string; nodes: AtlasNode[] };
export type SlotRegistry = Map<string, Map<string, number>>;
export function layoutAtlas(
  snapshot: ViewsSnapshot,
  slots: SlotRegistry,
): { nodes: AtlasNode[]; boards: AtlasBoard[]; width: number; height: number } {
  const spine = snapshot.agents
    .filter((a) => ["owner", "watcher", "admiral"].includes(a.role))
    .sort(
      (a, b) =>
        ["owner", "watcher", "admiral"].indexOf(a.role) -
        ["owner", "watcher", "admiral"].indexOf(b.role),
    );
  const nodes: AtlasNode[] = spine.map((agent, i) => ({
    agent,
    x: 40 + i * 320,
    y: 36,
    width: 272,
    height: 116,
  }));
  const projects = [
    ...snapshot.projects.map((p) => ({ id: p.id, name: p.name })),
  ];
  if (
    snapshot.agents.some(
      (a) => !a.project_id && ["worker", "assignment"].includes(a.role),
    )
  )
    projects.push({ id: "__unassigned", name: "Unassigned / unresolved" });
  const boards: AtlasBoard[] = [];
  let y = 252;
  for (let row = 0; row < projects.length; row += 3) {
    const rowBoards = projects.slice(row, row + 3).map((p, column) => {
      const agents = snapshot.agents.filter(
        (a) =>
          a.project_id === p.id ||
          (p.id === "__unassigned" &&
            !a.project_id &&
            ["worker", "assignment"].includes(a.role)),
      );
      let registry = slots.get(p.id);
      if (!registry) {
        registry = new Map();
        slots.set(p.id, registry);
      }
      const workers = agents
        .filter((a) => a.role !== "commodore")
        .sort((a, b) => a.id.localeCompare(b.id));
      const currentIds = new Set(workers.map((a) => a.id));
      for (const id of registry.keys())
        if (!currentIds.has(id)) registry.delete(id);
      const occupied = new Set(registry.values());
      let nextSlot = 0;
      for (const a of workers)
        if (!registry.has(a.id)) {
          while (occupied.has(nextSlot)) nextSlot++;
          registry.set(a.id, nextSlot);
          occupied.add(nextSlot++);
        }
      const x = 40 + column * 632,
        boardNodes: AtlasNode[] = [];
      const manager = agents.find((a) => a.role === "commodore");
      if (manager)
        boardNodes.push({
          agent: manager,
          x: x + 24,
          y: y + 64,
          width: 552,
          height: 100,
        });
      for (const agent of workers) {
        const slot = registry.get(agent.id)!;
        boardNodes.push({
          agent,
          x: x + 24 + (slot % 2) * 284,
          y: y + 196 + Math.floor(slot / 2) * 132,
          width: 268,
          height: 112,
        });
      }
      const height = Math.max(
        360,
        220 + Math.ceil((Math.max(-1, ...registry.values()) + 1) / 2) * 132,
      );
      return {
        id: p.id,
        name: p.name,
        x,
        y,
        width: 600,
        height,
        nodes: boardNodes,
      };
    });
    boards.push(...rowBoards);
    for (const b of rowBoards) nodes.push(...b.nodes);
    y += Math.max(...rowBoards.map((b) => b.height)) + 64;
  }
  return {
    nodes,
    boards,
    width: Math.max(1000, Math.min(projects.length, 3) * 632 + 40),
    height: Math.max(500, y),
  };
}
