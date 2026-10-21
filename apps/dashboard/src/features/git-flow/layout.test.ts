import { describe, expect, it } from "vitest";
import type { GitRepository } from "./model";
import { layoutGitGraph } from "./layout";

const oid = (n: number) => n.toString(16).padStart(40, "0");
function fixture(rows: [number, number[]][], main?: number): GitRepository {
  return {
    instance_id: "layout-fixture", node: "test", observed_utc: "2026-10-21T00:00:00Z", shallow: false, truncated: false,
    commits: rows.map(([id, parents]) => ({ oid: oid(id), parents: parents.map(oid), subject: `Commit ${id}`, committed_utc: "2000-01-01" })),
    refs: main === undefined ? [] : [{ name: "refs/heads/main", oid: oid(main), commit_oid: oid(main), kind: "branch" }], checkouts: [],
  };
}

function chain(count: number) {
  return fixture(Array.from({ length: count }, (_, i) => [i, i ? [i - 1] : []] as [number, number[]]), count ? count - 1 : undefined);
}
function assertGraph(layout: ReturnType<typeof layoutGitGraph>, repo: GitRepository) {
  expect(layout.byOid.size).toBe(repo.commits.length);
  expect(layout.orderedCommits).toHaveLength(repo.commits.length);
  expect(new Set(layout.paths.flatMap((path) => path.oids)).size).toBe(repo.commits.length);
  expect(layout.paths.reduce((count, path) => count + path.oids.length, 0)).toBe(repo.commits.length);
  expect(layout.edges).toHaveLength(repo.commits.reduce((count, commit) => count + commit.parents.length, 0));
  for (const commit of repo.commits) {
    const mark = layout.byOid.get(commit.oid)!;
    expect(mark.y).toBe(mark.lane === 0 ? 0 : -mark.lane * 54);
    expect(layout.backboneOids.has(commit.oid) ? mark.lane === 0 : mark.lane > 0).toBe(true);
    commit.parents.forEach((parent, parentIndex) => {
      const edge = layout.edges.find((candidate) => candidate.childOid === commit.oid && candidate.parentIndex === parentIndex)!;
      expect(edge.parentOid).toBe(parent);
      if (edge.parent && edge.parent !== edge.child) {
        expect(edge.parent.x).toBeLessThan(edge.child.x);
        expect(edge.parent.rank).toBeLessThan(edge.child.rank);
      }
    });
  }
  const lanes = new Map<number, typeof layout.paths>();
  for (const path of layout.paths) {
    const previous = lanes.get(path.lane) ?? [];
    previous.push(path); lanes.set(path.lane, previous);
  }
  for (const [lane, paths] of lanes) {
    if (lane === 0) continue;
    paths.sort((a, b) => a.startRank - b.startRank);
    for (let at = 1; at < paths.length; at++) expect(paths[at - 1].endRank + 1).toBeLessThan(paths[at].startRank);
  }
}

describe("loaded-history trunk-first layout", () => {
  it("reserves actual main first-parent ancestry and stacks every topic above it", () => {
    const repo = fixture([[1, []], [2, [1]], [3, [1]], [4, [2]], [5, [3]], [9, [1]], [10, [9]], [11, [10, 4, 5]]], 11);
    const layout = layoutGitGraph(repo);
    expect([...layout.backboneOids]).toEqual([11, 10, 9, 1].map(oid));
    for (const id of [1, 9, 10, 11]) expect([layout.byOid.get(oid(id))!.lane, layout.byOid.get(oid(id))!.y]).toEqual([0, 0]);
    expect(layout.edges.filter((edge) => edge.childOid === oid(11)).map((edge) => [edge.parentOid, edge.parentIndex, edge.kind]))
      .toEqual([[oid(10), 0, "backbone"], [oid(4), 1, "merge-parent"], [oid(5), 2, "merge-parent"]]);
    expect(layout.edges.filter((edge) => edge.backboneReturn).map((edge) => [edge.childOid, edge.parentOid, edge.parentIndex]))
      .toEqual([[oid(11), oid(4), 1], [oid(11), oid(5), 2]]);
    for (const id of [1, 11]) expect(layout.byOid.get(oid(id))!.range).toBeUndefined();
    assertGraph(layout, repo);
  });

  it("is deterministic under shuffled commits and attachment order, without a camera input", () => {
    const repo = fixture([[1, []], [2, [1]], [3, [1]], [4, [2]], [5, [3]], [9, [1]], [10, [9, 4, 5]]], 10);
    repo.refs.push({ name: "refs/heads/topic", oid: oid(4), commit_oid: oid(4), kind: "branch" });
    const options = { expandedOids: new Set(repo.commits.map((commit) => commit.oid)) };
    const layout = layoutGitGraph(repo, options), shuffled = layoutGitGraph({ ...repo, commits: [...repo.commits].reverse(), refs: [...repo.refs].reverse() }, options);
    expect(shuffled.marks).toEqual(layout.marks);
    expect(shuffled.paths).toEqual(layout.paths);
    expect(shuffled.edges).toEqual(layout.edges);
    expect(shuffled.bounds).toEqual(layout.bounds);
    expect(layoutGitGraph(repo, options).marks).toEqual(layout.marks);
  });

  it("keeps stacked ancestry shared once and routes main syncs and topic merges truthfully", () => {
    const repo = fixture([[1, []], [10, [1]], [11, [10]], [2, [1]], [3, [2]], [4, [3, 11]], [5, [3]], [6, [5]], [7, [4, 6]]], 11);
    repo.refs.push(...[ ["alpha", 7], ["beta", 6] ].map(([name, n]) => ({ name: `refs/heads/${name}`, oid: oid(Number(n)), commit_oid: oid(Number(n)), kind: "branch" })));
    const layout = layoutGitGraph(repo), owner = new Map(layout.paths.flatMap((path) => path.oids.map((id) => [id, path] as const)));
    expect(owner.get(oid(6))!.lane).toBeGreaterThan(owner.get(oid(3))!.lane);
    expect(layout.byOid.get(oid(4))!.lane).toBeGreaterThan(0);
    expect(layout.edges.find((edge) => edge.childOid === oid(4) && edge.parentIndex === 1)?.kind).toBe("merge-parent");
    expect(layout.edges.find((edge) => edge.childOid === oid(7) && edge.parentIndex === 1)?.parentOid).toBe(oid(6));
    expect(layout.edges.filter((edge) => edge.backboneReturn)).toEqual([]);
    assertGraph(layout, repo);
  });

  it("packs sequential topic intervals without overlapping or placing topics on main", () => {
    const repo = fixture(Array.from({ length: 160 }, (_, i) => [10000 + i, i ? [9999 + i] : []] as [number, number[]]), 10159);
    for (let at = 4; at < 150; at += 5) {
      repo.commits.push(...fixture([[at * 10, [10000 + at]], [at * 10 + 1, [at * 10]], [at * 10 + 2, [at * 10 + 1]]]).commits);
      repo.refs.push({ name: `refs/heads/topic-${at}`, oid: oid(at * 10 + 2), commit_oid: oid(at * 10 + 2), kind: "branch" });
    }
    const layout = layoutGitGraph(repo);
    expect(new Set(layout.paths.filter((path) => path.lane).map((path) => path.lane)).size).toBeLessThan(10);
    assertGraph(layout, repo);
  });

  it("retains disconnected components, criss-cross edges and all ordered octopus parents", () => {
    const repo = fixture([[1, []], [2, [1]], [3, [1]], [4, [2, 3]], [5, [3, 2]], [6, [4, 5]], [20, []], [21, [20]], [22, [21]]], 6);
    const layout = layoutGitGraph(repo);
    expect(layout.edges.some((edge) => Number.parseInt(edge.parentOid, 16) < 20 && Number.parseInt(edge.childOid, 16) >= 20)).toBe(false);
    expect(layout.byOid.get(oid(20))!.y).toBeLessThan(0);
    assertGraph(layout, repo);
  });

  it("keeps missing ancestry, source objects, timestamps, refs and checkout uncertainty intact", () => {
    const repo = fixture([[1, [98, 99]], [2, [1]], [3, [2]]], 3);
    repo.shallow = repo.truncated = true;
    repo.checkouts = [{ id: "tree", name: "detached", head: oid(2), kind: "linked", detached: true, locked: true, prunable: false,
      status: { staged: 1, unstaged: 2, untracked: 3, conflicts: 0 } }];
    const before = structuredClone(repo), layout = layoutGitGraph(repo);
    expect(layout.boundaries).toBe(2);
    expect(layout.backbone.missingParentOid).toBe(oid(98));
    expect(layout.edges.filter((edge) => edge.kind === "missing").map((edge) => edge.parentOid)).toEqual([oid(98), oid(99)]);
    expect(layout.refsByOid.get(oid(3))![0]).toBe(repo.refs[0]);
    expect(layout.checkoutsByOid.get(oid(2))![0]).toBe(repo.checkouts[0]);
    expect(layout.byOid.get(oid(1))!.commit).toBe(repo.commits[0]);
    expect(repo).toEqual(before);
    assertGraph(layout, repo);
  });

  it("uses neutral off-main paths when no backbone is resolved", () => {
    const repo = fixture([[1, []], [2, [1]], [3, [2]]]), layout = layoutGitGraph(repo);
    expect(layout.backbone.kind).toBe("unresolved");
    expect(layout.backboneOids.size).toBe(0);
    expect(layout.marks.every((mark) => mark.y < 0)).toBe(true);
    assertGraph(layout, repo);
  });

  it.each([0, 1, 2, 3])("handles %i commits without fake roots or collapse", (count) => {
    const repo = chain(count), layout = layoutGitGraph(repo);
    expect(layout.marks).toHaveLength(count);
    expect(layout.ranges).toHaveLength(0);
    expect(Object.values(layout.bounds).every(Number.isFinite)).toBe(true);
    assertGraph(layout, repo);
  });

  it("compresses 1000 exact interiors reversibly while retaining all source edges", () => {
    const repo = chain(1000), compact = layoutGitGraph(repo), range = compact.ranges[0];
    expect(compact.marks).toHaveLength(3);
    expect(range.commits).toEqual(repo.commits.slice(1, -1));
    expect(compact.width).toBeLessThan(1000);
    expect(compact.edges).toHaveLength(999);
    const expanded = layoutGitGraph(repo, { expandedOids: new Set([oid(500)]) });
    expect(expanded.marks).toHaveLength(1000);
    expect(expanded.ranges[0].expanded).toBe(true);
    expect(layoutGitGraph(repo)).toEqual(compact);
    for (const commit of range.commits) expect(compact.byOid.get(commit.oid)).toBe(compact.marks[1]);
  });

  it("preserves manual collapse through growth, and does not collapse anchors", () => {
    const previous = layoutGitGraph(chain(20)), collapsed = new Set(previous.ranges[0].commits.map((commit) => commit.oid));
    const updated = layoutGitGraph(chain(21), { collapsedOids: collapsed, expandedOids: new Set(chain(21).commits.map((commit) => commit.oid)) });
    expect(updated.ranges[0].expanded).toBe(false);
    expect(updated.ranges[0].commits).toHaveLength(19);
    const repo = chain(40);
    repo.refs.push(...[10, 20].map((i) => ({ name: `refs/tags/v${i}`, oid: oid(i), commit_oid: oid(i), kind: "tag" })));
    repo.checkouts = [{ id: "detached", name: "tree", head: oid(30), kind: "linked", detached: true, locked: false, prunable: false }];
    const layout = layoutGitGraph(repo);
    expect(layout.ranges).toHaveLength(4);
    for (const id of [0, 10, 20, 30, 39]) expect(layout.byOid.get(oid(id))!.range).toBeUndefined();
  });

  it("preserves path ids and feasible prior lanes while a tip grows or a sibling arrives", () => {
    const repo = fixture([[1, []], [10, [1]], [11, [10]], [2, [1]], [3, [2]]], 11);
    repo.refs.push({ name: "refs/heads/z-topic", oid: oid(3), commit_oid: oid(3), kind: "branch" });
    const initial = layoutGitGraph(repo), topic = initial.paths.find((path) => path.oids.includes(oid(3)))!;
    const grown = structuredClone(repo);
    grown.commits.push(...fixture([[4, [3]], [20, [1]], [21, [20]]]).commits);
    grown.refs[1] = { ...grown.refs[1], oid: oid(4), commit_oid: oid(4) };
    grown.refs.push({ name: "refs/heads/a-new-topic", oid: oid(21), commit_oid: oid(21), kind: "branch" });
    const layout = layoutGitGraph(grown, { previousPaths: initial.paths });
    const continued = layout.paths.find((path) => path.oids.includes(oid(4)))!;
    expect(continued.id).toBe(topic.id);
    expect(continued.lane).toBe(topic.lane);
    expect(continued.oids).toEqual([2, 3, 4].map(oid));
    expect(layoutGitGraph({ ...grown, commits: [...grown.commits].reverse() }, { previousPaths: [...initial.paths].reverse() })).toEqual(layout);
    assertGraph(layout, grown);
  });

  it("reassigns infeasible old lanes after overlapping extensions without collisions", () => {
    const repo = fixture([[1, []], [10, [1]], [11, [10]], [2, [1]], [3, [2]], [20, [11]], [21, [20]]], 11);
    repo.refs.push(...[["a", 3], ["b", 21]].map(([name, id]) => ({ name: `refs/heads/${name}`, oid: oid(Number(id)), commit_oid: oid(Number(id)), kind: "branch" })));
    const initial = layoutGitGraph(repo), fakePrior = initial.paths.map((path) => path.lane ? { ...path, lane: 1 } : path);
    const layout = layoutGitGraph(repo, { previousPaths: fakePrior });
    assertGraph(layout, repo);
  });

  it("bounds contain dots, missing stubs, ranges and conservative curve control hulls", () => {
    const repo = fixture([[1, [90]], [2, [1]], [3, [2]], [4, [3]], [10, [1]], [11, [10, 4]]], 11);
    const layout = layoutGitGraph(repo), b = layout.bounds;
    for (const mark of layout.marks) {
      expect(mark.x - 6).toBeGreaterThanOrEqual(b.minX); expect(mark.x + 6).toBeLessThanOrEqual(b.maxX);
      expect(mark.y - 6).toBeGreaterThanOrEqual(b.minY); expect(mark.y + 6).toBeLessThanOrEqual(b.maxY);
    }
    for (const edge of layout.edges) if (!edge.parent) expect(edge.child.x - 40).toBeGreaterThanOrEqual(b.minX);
    expect(b.minY).toBeLessThan(0);
  });

  it("rejects duplicate identities, malformed parents and cycles", () => {
    const duplicate = chain(2); duplicate.commits.push(duplicate.commits[0]);
    expect(() => layoutGitGraph(duplicate)).toThrow(/identity/);
    const invalid = chain(2); invalid.commits[1].parents = ["bad"];
    expect(() => layoutGitGraph(invalid)).toThrow(/parent/);
    const repeated = chain(2); repeated.commits[1].parents.push(oid(0));
    expect(() => layoutGitGraph(repeated)).toThrow(/parent/);
    expect(() => layoutGitGraph(fixture([[1, [2]], [2, [1]]], 2))).toThrow(/cyclic/);
  });

  it("supports SHA256 identities without shortening source ids", () => {
    const repo = chain(8), map = (id: string) => id.padStart(64, "b");
    repo.commits = repo.commits.map((commit) => ({ ...commit, oid: map(commit.oid), parents: commit.parents.map(map) }));
    repo.refs = repo.refs.map((ref) => ({ ...ref, oid: map(ref.oid), commit_oid: map(ref.commit_oid!) }));
    const layout = layoutGitGraph(repo);
    expect(layout.byOid.size).toBe(8);
    expect(layout.orderedCommits.every((commit) => commit.oid.length === 64)).toBe(true);
  });

  it("retains exact ancestry and interval separation across seeded DAGs and adoption", () => {
    let seed = 83;
    const random = () => { seed = (seed * 1664525 + 1013904223) >>> 0; return seed; };
    for (let round = 0; round < 20; round++) {
      const rows: [number, number[]][] = [[0, []]];
      for (let id = 1; id < 100; id++) {
        const first = random() % id;
        const parents = [first];
        if (id > 3 && random() % 3 === 0) {
          const second = random() % id;
          if (second !== first) parents.push(second);
        }
        rows.push([id, parents]);
      }
      const repo = fixture(rows, 99);
      for (let id = 5; id < 100; id += 13) repo.refs.push({ name: `refs/heads/topic-${id}`, oid: oid(id), commit_oid: oid(id), kind: "branch" });
      const first = layoutGitGraph(repo), adopted = layoutGitGraph({ ...repo, commits: [...repo.commits].reverse() }, { previousPaths: first.paths });
      assertGraph(first, repo); assertGraph(adopted, repo);
      expect(adopted.paths).toEqual(first.paths);
      expect(adopted.marks).toEqual(first.marks);
      const ownership = new Map(adopted.paths.flatMap((path) => path.oids.map((id) => [id, path] as const)));
      for (const path of adopted.paths) if (path.lane) {
        const parent = repo.commits.find((commit) => commit.oid === path.oids[0])!.parents[0];
        const parentPath = ownership.get(parent);
        if (parentPath) expect(path.lane).toBeGreaterThan(parentPath.lane);
      }
    }
  });

  it("handles more than100000 linear commits without recursion or spread-argument limits", () => {
    const layout = layoutGitGraph(chain(100001));
    expect(layout.byOid.size).toBe(100001);
    expect(layout.edges).toHaveLength(100000);
    expect(layout.marks).toHaveLength(3);
  }, 10000);

  it("packs10000 concurrent disconnected roots without quadratic lane scanning", () => {
    const repo = fixture(Array.from({ length: 10000 }, (_, i) => [i, []] as [number, number[]]));
    const layout = layoutGitGraph(repo);
    expect(layout.paths).toHaveLength(10000);
    expect(new Set(layout.paths.map((path) => path.lane)).size).toBe(10000);
    expect(layout.edges).toHaveLength(0);
  }, 10000);
});
