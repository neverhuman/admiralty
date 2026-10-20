import { describe, expect, it } from "vitest";
import type { GitRepository } from "./feed";
import { layoutGit } from "./geometry";

const oid = (n: number) => "a" + n.toString(16).padStart(39, "0");
function chain(count: number): GitRepository {
  return {
    instance_id: "synthetic", node: "test", observed_utc: "2026-10-20T12:00:00Z",
    shallow: false, truncated: false, refs: [], checkouts: [],
    commits: Array.from({ length: count }, (_, i) => ({
      oid: oid(i), parents: i ? [oid(i - 1)] : [],
      subject: `Commit ${i}`, committed_utc: "2000-01-01",
    })),
  };
}
describe("maximal linear Git compression", () => {
  it("reduces 1,000 linear commits to two anchors and one exact range", () => {
    const repo = chain(1000), before = structuredClone(repo), layout = layoutGit(repo);
    expect(layout.marks).toHaveLength(3);
    expect(layout.ranges).toHaveLength(1);
    expect(layout.ranges[0].commits).toEqual(repo.commits.slice(1, -1));
    expect(layout.byOid.size).toBe(1000);
    expect(layout.orderedCommits).toHaveLength(1000);
    expect(layout.width).toBeLessThan(1000);
    expect(repo).toEqual(before);
    expect(layoutGit({ ...repo, commits: [...repo.commits].reverse() })).toEqual(layout);
  });
  it("expands and contracts every member without changing ancestry or count", () => {
    const repo = chain(100), compact = layoutGit(repo);
    const expanded = layoutGit(repo, new Set(compact.ranges[0].commits.map((c) => c.oid)));
    expect(expanded.marks).toHaveLength(100);
    expect(expanded.ranges[0].expanded).toBe(true);
    for (let i = 1; i < 100; i++)
      expect(expanded.byOid.get(oid(i))!.x).toBeGreaterThan(expanded.byOid.get(oid(i - 1))!.x);
    expect(layoutGit(repo)).toEqual(compact);
  });
  it("keeps expansion when a live tip extends the same run", () => {
    const expanded = new Set(layoutGit(chain(20)).ranges[0].commits.map((c) => c.oid));
    const updated = layoutGit(chain(21), expanded);
    expect(updated.ranges[0].expanded).toBe(true);
    expect(updated.marks).toHaveLength(21);
  });
  it("keeps a manually collapsed growing range closed despite new automatic matches", () => {
    const initial = layoutGit(chain(20));
    const collapsed = new Set(initial.ranges[0].commits.map((commit) => commit.oid));
    const repo = chain(21), matching = new Set(repo.commits.map((commit) => commit.oid));
    const updated = layoutGit(repo, matching, 1, collapsed);
    expect(updated.ranges[0].expanded).toBe(false);
    expect(updated.ranges[0].commits).toHaveLength(19);
    expect(updated.marks).toHaveLength(3);
    expect(updated.orderedCommits).toHaveLength(21);
    expect(updated.byOid.size).toBe(21);
  });
  it("keeps manual collapse local to its range when refs split a history", () => {
    const repo = chain(40);
    repo.refs = [{ name: "refs/tags/split", oid: oid(20), commit_oid: oid(20), kind: "tag" }];
    const collapsed = new Set(layoutGit(repo).ranges[0].commits.map((commit) => commit.oid));
    const matching = new Set(repo.commits.map((commit) => commit.oid));
    const layout = layoutGit(repo, matching, 1, collapsed);
    expect(layout.ranges.map((range) => range.expanded)).toEqual([false, true]);
    expect(layout.byOid.get(oid(20))?.range).toBeUndefined();
    expect(layout.orderedCommits).toHaveLength(40);
  });
  it("preserves refs, tags and detached checkout heads as visible anchors", () => {
    const repo = chain(40);
    repo.refs = [10, 20].map((i) => ({ name: `refs/tags/v${i}`, oid: oid(i), commit_oid: oid(i), kind: "tag" }));
    repo.checkouts = [{ id: "detached", name: "tree", head: oid(30), kind: "linked", detached: true, locked: false, prunable: false }];
    const layout = layoutGit(repo);
    expect(layout.ranges).toHaveLength(4);
    for (const i of [0, 10, 20, 30, 39]) {
      expect(layout.byOid.get(oid(i))!.commit.oid).toBe(oid(i));
      expect(layout.byOid.get(oid(i))!.range).toBeUndefined();
    }
  });
  it("retains fork and octopus merge points and every ordered merge parent", () => {
    const repo = chain(36);
    repo.commits[12].parents = [oid(2)];
    repo.commits[22].parents = [oid(2)];
    repo.commits[32].parents = [oid(11), oid(21), oid(31)];
    const layout = layoutGit(repo);
    expect(layout.ranges.length).toBeGreaterThan(1);
    expect(layout.byOid.get(oid(2))!.range).toBeUndefined();
    expect(layout.byOid.get(oid(32))!.range).toBeUndefined();
    for (const c of repo.commits) for (const p of c.parents) {
      const a = layout.byOid.get(p)!, b = layout.byOid.get(c.oid)!;
      if (a !== b) expect(a.x).toBeLessThan(b.x);
    }
  });
  it("leaves shallow and bounded-window parent stubs explicit", () => {
    const repo = chain(20);
    repo.shallow = repo.truncated = true;
    repo.commits[0].parents = [oid(100), oid(101)];
    const layout = layoutGit(repo);
    expect(layout.boundaries).toBe(2);
    expect(layout.marks[0].commit).toEqual(repo.commits[0]);
    expect(layout.marks[0].range).toBeUndefined();
  });
  it.each([0, 1, 2, 3, 4, 5])("keeps tiny histories of %i commits explicit", (n) => {
    expect(layoutGit(chain(n)).marks).toHaveLength(n);
    expect(layoutGit(chain(n)).ranges).toHaveLength(0);
  });
  it("reserves readable bubble space at every supported zoom", () => {
    for (const zoom of [0.2, 0.4, 0.9, 1, 2]) {
      const layout = layoutGit(chain(100), new Set(), zoom);
      const [root, bubble, tip] = layout.marks;
      expect((bubble.x - root.x) * zoom).toBeGreaterThan(152);
      expect((tip.x - bubble.x) * zoom).toBeGreaterThan(152);
    }
  });
  it("never folds disconnected histories together", () => {
    const repo = chain(20);
    repo.commits[10].parents = [];
    const layout = layoutGit(repo);
    expect(layout.ranges).toHaveLength(2);
    expect(layout.marks).toHaveLength(6);
    expect(layout.ranges[0].commits).toHaveLength(8);
    expect(layout.ranges[1].commits).toHaveLength(8);
  });
  it("preserves all external parent edges across many seeded DAGs", () => {
    let seed = 7;
    const random = () => { seed = (seed * 1664525 + 1013904223) >>> 0; return seed; };
    for (let cut = 0; cut < 100; cut++) {
      const repo = chain(100);
      for (let i = 3; i < 100; i++) if (random() % 8 === 0)
        repo.commits[i].parents.push(oid(random() % (i - 1)));
      const layout = layoutGit(repo);
      for (const c of repo.commits) for (const p of c.parents) {
        const a = layout.byOid.get(p)!, b = layout.byOid.get(c.oid)!;
        if (a !== b) expect(a.x).toBeLessThan(b.x);
        else expect(b.range?.commits.some((member) => member.oid === p)).toBe(true);
      }
      expect(new Set(layout.orderedCommits.map((c) => c.oid)).size).toBe(100);
    }
  });
});
