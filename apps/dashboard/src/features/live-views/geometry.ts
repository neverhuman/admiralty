import { layoutGitGraph } from "../git-flow/layout";
import type {
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
import type { CommitMark, GitLayout } from "../git-flow/model";
export type { CommitMark, CommitRange, GitLayout } from "../git-flow/model";
const GIT_COMMIT_SPACING = 112;
/** Spend less than the omitted columns, with a usable target at extreme zoom. */
export function gitRangeWidth(count: number, zoom: number) {
  return Math.max(24, Math.min(304, (count - 1) * GIT_COMMIT_SPACING * zoom * 0.75));
}
/** Compatibility bands use the same world layout as project investigation.
 * Only their origin is translated into the old positive scroll coordinate space.
 * Camera scale never changes source topology or world spacing.
 */
export function layoutGit(repo: GitRepository, expanded: ReadonlySet<string> = new Set(), _zoom = 1, collapsed: ReadonlySet<string> = new Set()): GitLayout {
  const graph = layoutGitGraph(repo, { expandedOids: expanded, collapsedOids: collapsed });
  const offset = 44 - graph.marks.reduce((minimum, mark) => Math.min(minimum, mark.y), 0);
  const ranges = graph.ranges.map((range) => ({ ...range, y: range.y + offset }));
  const rangeById = new Map(ranges.map((range) => [range.id, range]));
  const translated = new Map<CommitMark, CommitMark>();
  const marks = graph.marks.map((mark) => {
    const next = { ...mark, y: mark.y + offset, range: mark.range ? rangeById.get(mark.range.id) : undefined };
    translated.set(mark, next); return next;
  });
  return { ...graph, marks, ranges,
    byOid: new Map([...graph.byOid].map(([oid, mark]) => [oid, translated.get(mark)!])),
    edges: graph.edges.map((edge) => ({ ...edge, child: translated.get(edge.child)!, parent: edge.parent ? translated.get(edge.parent)! : null })),
    bounds: { ...graph.bounds, minY: graph.bounds.minY + offset, maxY: graph.bounds.maxY + offset },
    height: Math.max(180, graph.bounds.maxY + offset + 56),
  } as GitLayout;
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
