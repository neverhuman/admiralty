import { describe, expect, it } from "vitest";
import type { GitRepository } from "./model";
import { createGitMergeIndex } from "./merges";

const oid = (n: number) => n.toString(16).padStart(40, "0");
function repo(rows: [number, number[]][]): GitRepository {
  return { instance_id: "actual-merges", node: "test", observed_utc: "2026-10-22T00:00:00Z", shallow: false, truncated: false, refs: [], checkouts: [],
    commits: rows.map(([id, parents]) => ({ oid: oid(id), parents: parents.map(oid), subject: `Commit ${id}`, committed_utc: "2000-01-01" })) };
}
function ref(name: string, commit: number): GitRepository["refs"][number] {
  return { name, oid: oid(commit), commit_oid: oid(commit), kind: "branch" };
}

describe("actual Git merge facts", () => {
  it("retains adversarial first-parent order and every octopus parent", () => {
    const source = repo([[1, []], [2, [1]], [3, [1]], [4, [1]], [10, [3, 2]], [11, [10, 4, 2, 1]]]);
    source.refs = [ref("refs/heads/main", 2), ref("refs/heads/topic", 3), ref("refs/remotes/origin/main", 2)];
    const index = createGitMergeIndex(source, new Set([oid(11)]));
    expect(index.size).toBe(2);
    expect(index.get(oid(10))).toEqual({ commitOid: oid(10), parentCount: 2,
      firstParent: { oid: oid(3), loaded: true, refs: ["refs/heads/topic"] },
      additionalParents: [{ oid: oid(2), loaded: true, refs: ["refs/heads/main", "refs/remotes/origin/main"] }], onBackbone: false });
    expect(index.get(oid(11))?.firstParent.oid).toBe(oid(10));
    expect(index.get(oid(11))?.additionalParents.map((parent) => parent.oid)).toEqual([4, 2, 1].map(oid));
    expect(index.get(oid(11))?.parentCount).toBe(4);
    expect(index.get(oid(11))?.onBackbone).toBe(true);
  });

  it("is deterministic under commit/ref permutation without sorting actual parents", () => {
    const source = repo([[10, [3, 2, 1]], [1, []], [2, [1]], [11, [2, 3]], [3, [1]]]);
    source.refs = [ref("refs/heads/z", 2), ref("refs/heads/A", 2), ref("refs/heads/a", 2), ref("refs/tags/v1", 3)];
    const first = createGitMergeIndex(source), second = createGitMergeIndex({ ...source, commits: [...source.commits].reverse(), refs: [...source.refs].reverse() });
    expect([...second]).toEqual([...first]);
    expect([...first.keys()]).toEqual([oid(10), oid(11)]);
    expect(first.get(oid(10))?.firstParent.oid).toBe(oid(3));
    expect(first.get(oid(10))?.additionalParents[0].refs).toEqual(["refs/heads/A", "refs/heads/a", "refs/heads/z"]);
  });

  it("keeps missing parent OIDs and reports only their actual matching refs", () => {
    const source = repo([[1, []], [2, [99, 1, 98]]]);
    source.shallow = true;
    source.refs = [ref("refs/remotes/origin/unloaded", 99)];
    const facts = createGitMergeIndex(source).get(oid(2))!;
    expect(facts.firstParent).toEqual({ oid: oid(99), loaded: false, refs: ["refs/remotes/origin/unloaded"] });
    expect(facts.additionalParents).toEqual([{ oid: oid(1), loaded: true, refs: [] }, { oid: oid(98), loaded: false, refs: [] }]);
    expect(facts.parentCount).toBe(3);
  });

  it("never attributes raw ref object OIDs when commit_oid is absent or different", () => {
    const source = repo([[1, []], [2, []], [3, [1, 2]]]);
    source.refs = [
      { name: "refs/tags/unpeeled", oid: oid(1), kind: "tag" },
      { name: "refs/tags/peeled-elsewhere", oid: oid(1), commit_oid: oid(2), kind: "tag" },
      ref("refs/heads/actual-parent", 1),
    ];
    const facts = createGitMergeIndex(source).get(oid(3))!;
    expect(facts.firstParent.refs).toEqual(["refs/heads/actual-parent"]);
    expect(facts.additionalParents[0].refs).toEqual(["refs/tags/peeled-elsewhere"]);
  });

  it("does not invent merges from squash/rebase/fast-forward-looking messages or refs", () => {
    const source = repo([[1, []], [2, [1]], [3, [2]], [4, [3]], [5, [4]]]);
    ["Merge branch topic", "Squash merge PR #42", "Rebased onto main", "Fast-forward integration", "Landed safely"].forEach((subject, at) => { source.commits[at].subject = subject; });
    source.refs = [ref("refs/heads/main", 5), ref("refs/heads/squashed", 2), ref("refs/heads/merged-topic", 4)];
    expect([...createGitMergeIndex(source)]).toEqual([]);
  });

  it("keeps backbone membership unknown unless explicitly supplied", () => {
    const source = repo([[1, []], [2, []], [3, [1, 2]]]);
    source.refs = [ref("refs/heads/main", 3)];
    expect(createGitMergeIndex(source).get(oid(3))?.onBackbone).toBeNull();
    expect(createGitMergeIndex(source, new Set()).get(oid(3))?.onBackbone).toBe(false);
    expect(createGitMergeIndex(source, new Set([oid(3)])).get(oid(3))?.onBackbone).toBe(true);
  });

  it("sorts names by Unicode codepoint, including astral characters and prefixes", () => {
    const source = repo([[1, []], [2, []], [3, [1, 2]]]);
    source.refs = ["refs/heads/😀", "refs/heads/\uE000", "refs/heads/a-long", "refs/heads/a", "refs/heads/A"].map((name) => ref(name, 1));
    expect(createGitMergeIndex(source).get(oid(3))?.firstParent.refs)
      .toEqual(["refs/heads/A", "refs/heads/a", "refs/heads/a-long", "refs/heads/\uE000", "refs/heads/😀"]);
  });

  it("preserves frozen source parents/refs/timestamps and exact raw SHA256 OIDs", () => {
    const source = repo([[1, []], [2, []], [3, [2, 1]]]), wide = (value: string) => value.padStart(64, "b");
    source.commits = source.commits.map((commit) => ({ ...commit, oid: wide(commit.oid), parents: commit.parents.map(wide) }));
    source.refs = [{ name: "refs/heads/topic", oid: wide(oid(2)), commit_oid: wide(oid(2)), kind: "branch" }];
    const before = structuredClone(source);
    for (const commit of source.commits) { Object.freeze(commit.parents); Object.freeze(commit); }
    for (const observedRef of source.refs) Object.freeze(observedRef);
    Object.freeze(source.commits); Object.freeze(source.refs); Object.freeze(source);
    const facts = createGitMergeIndex(source).get(wide(oid(3)))!;
    expect(facts.firstParent.oid).toBe(wide(oid(2)));
    expect(facts.additionalParents[0].oid).toBe(wide(oid(1)));
    expect(source).toEqual(before);
  });

  it("handles an empty repository without invented merge facts", () => {
    expect(createGitMergeIndex(repo([])).size).toBe(0);
  });
});
