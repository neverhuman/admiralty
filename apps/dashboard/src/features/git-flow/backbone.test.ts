import { describe, expect, it } from "vitest";
import type { BackboneOptions, GitCommit, GitRepository } from "./model";
import { resolveBackbone } from "./backbone";

const oid = (n: number) => n.toString(16).padStart(40, "0");
const commit = (n: number, parents: number[] = []): GitCommit => ({
  oid: oid(n), parents: parents.map(oid), subject: `Commit ${n}`,
  committed_utc: "2026-10-20T12:00:00Z",
});
const ref = (name: string, tip: number) => ({
  name, oid: oid(tip), commit_oid: oid(tip), kind: "branch",
});
function repository(): GitRepository {
  return {
    instance_id: "observed-repository", node: "test", observed_utc: "2026-10-20T12:00:00Z",
    shallow: false, truncated: false,
    commits: [commit(1), commit(2, [1]), commit(3, [1]), commit(4, [3, 2]), commit(5, [2])],
    refs: [ref("refs/heads/main", 4), ref("refs/remotes/origin/main", 5), ref("refs/heads/topic", 2)],
    checkouts: [{ id: "newer", name: "newer worktree", head: oid(5), kind: "linked", detached: true, locked: false, prunable: false }],
  };
}
const integration = (repo: GitRepository, refName = "refs/heads/topic", tip: number | null = 2): BackboneOptions => ({
  integration: { instanceId: repo.instance_id, refName, oid: tip === null ? null : oid(tip) },
});
function deepFreeze<T>(value: T): T {
  if (value !== null && typeof value === "object") {
    Object.freeze(value);
    for (const child of Object.values(value)) deepFreeze(child);
  }
  return value;
}

describe("observed Git display backbone", () => {
  it("uses exact local main rather than newer origin, checkout, or sibling tips", () => {
    const result = resolveBackbone(repository());
    expect(result).toMatchObject({ kind: "resolved", source: "display-default", refName: "refs/heads/main", tipOid: oid(4), spine: [oid(4), oid(3), oid(1)], missingParentOid: null });
    expect(result.reason).toContain("Display default");
  });

  it("does not guess main from remote main, master, a similarly named ref, or a checkout", () => {
    const repo = repository();
    repo.refs = [ref("refs/remotes/origin/main", 5), ref("refs/heads/master", 4), ref("refs/heads/team/main", 3)];
    expect(resolveBackbone(repo)).toMatchObject({ kind: "unresolved", source: null, refName: null, tipOid: null, spine: [] });
  });

  it("keeps an empty or unborn repository unresolved", () => {
    const repo = repository();
    repo.commits = []; repo.refs = []; repo.checkouts = [];
    expect(resolveBackbone(repo)).toMatchObject({ kind: "unresolved", tipOid: null, spine: [], missingParentOid: null });
  });

  it("lets an explicit UI choice override integration display selection", () => {
    const repo = repository();
    const options = { ...integration(repo), explicitRef: "refs/remotes/origin/main" };
    const result = resolveBackbone(repo, options);
    expect(result).toMatchObject({ kind: "resolved", source: "explicit", refName: "refs/remotes/origin/main", tipOid: oid(5), spine: [oid(5), oid(2), oid(1)] });
    expect(options.integration?.refName).toBe("refs/heads/topic");
  });

  it("does not let a foreign or stale integration policy invalidate an explicit display choice", () => {
    const repo = repository();
    const result = resolveBackbone(repo, { explicitRef: "refs/heads/main", integration: { instanceId: "foreign-repository", refName: "refs/heads/topic", oid: oid(99) } });
    expect(result).toMatchObject({ kind: "resolved", source: "explicit", refName: "refs/heads/main", tipOid: oid(4), spine: [oid(4), oid(3), oid(1)] });
  });

  it("does not fall back when an explicit ref is missing", () => {
    const repo = repository();
    expect(resolveBackbone(repo, { ...integration(repo), explicitRef: "refs/heads/missing" })).toMatchObject({ kind: "missing", source: "explicit", refName: "refs/heads/missing", tipOid: null, spine: [] });
  });

  it("requires exact full ref matching rather than expanding aliases", () => {
    expect(resolveBackbone(repository(), { explicitRef: "main" })).toMatchObject({ kind: "missing", source: "explicit", refName: "main", spine: [] });
  });

  it("treats an empty UI choice as absent", () => {
    expect(resolveBackbone(repository(), { explicitRef: "  " })).toMatchObject({ kind: "resolved", source: "display-default", refName: "refs/heads/main" });
  });

  it("uses a matching-instance integration ref only at its snapshot-bound OID", () => {
    const repo = repository();
    expect(resolveBackbone(repo, integration(repo))).toMatchObject({ kind: "resolved", source: "integration", refName: "refs/heads/topic", tipOid: oid(2), spine: [oid(2), oid(1)] });
  });

  it("keeps a missing configured integration ref missing instead of choosing main", () => {
    const repo = repository();
    expect(resolveBackbone(repo, integration(repo, "refs/heads/missing"))).toMatchObject({ kind: "missing", source: "integration", refName: "refs/heads/missing", tipOid: null, spine: [] });
  });

  it("reports unknown integration binding when the policy has no OID", () => {
    const repo = repository();
    const result = resolveBackbone(repo, integration(repo, "refs/heads/topic", null));
    expect(result).toMatchObject({ kind: "missing", source: "integration", tipOid: oid(2), spine: [] });
    expect(result.reason).toContain("no snapshot-bound OID");
  });

  it("reports stale integration binding without substituting its expected or main tip", () => {
    const repo = repository();
    const result = resolveBackbone(repo, integration(repo, "refs/heads/topic", 5));
    expect(result).toMatchObject({ kind: "missing", source: "integration", tipOid: oid(2), spine: [] });
    expect(result.reason).toContain("stale");
    expect(result.reason).toContain(oid(5));
    expect(result.reason).toContain(oid(2));
  });

  it("ignores a policy for another repository instance with an explicit diagnostic", () => {
    const repo = repository();
    const result = resolveBackbone(repo, { integration: { instanceId: "other-repository", refName: "refs/heads/topic", oid: oid(2) } });
    expect(result).toMatchObject({ kind: "resolved", source: "display-default", tipOid: oid(4) });
    expect(result.reason).toContain("other-repository");
    expect(result.reason).toContain("ignored");
  });

  it("remains unresolved when the foreign policy and a missing local main are all that exist", () => {
    const repo = repository(); repo.refs = [ref("refs/heads/topic", 2)];
    const result = resolveBackbone(repo, { integration: { instanceId: "other-repository", refName: "refs/heads/topic", oid: oid(2) } });
    expect(result).toMatchObject({ kind: "unresolved", source: null, spine: [] });
    expect(result.reason).toContain("ignored");
  });

  it("preserves an observed but unloaded tip as a missing display location", () => {
    const repo = repository(); repo.refs.push(ref("refs/heads/not-loaded", 100));
    const result = resolveBackbone(repo, { explicitRef: "refs/heads/not-loaded" });
    expect(result).toMatchObject({ kind: "missing", source: "explicit", tipOid: oid(100), spine: [], missingParentOid: null });
    expect(result.reason).toContain("not loaded");
  });

  it("never uses a noncommit ref object's OID as its commit", () => {
    const repo = repository();
    repo.refs.push({ name: "refs/tags/blob", oid: oid(4), commit_oid: null, kind: "tag" });
    expect(resolveBackbone(repo, { explicitRef: "refs/tags/blob" })).toMatchObject({ kind: "missing", tipOid: null, spine: [] });
  });

  it("resolves an explicitly chosen annotated tag through its loaded peeled commit", () => {
    const repo = repository();
    repo.refs.push({ name: "refs/tags/release", oid: oid(99), commit_oid: oid(3), kind: "tag" });
    expect(resolveBackbone(repo, { explicitRef: "refs/tags/release" })).toMatchObject({ kind: "resolved", source: "explicit", tipOid: oid(3), spine: [oid(3), oid(1)] });
  });

  it("walks only ordered first parents and preserves the missing loaded-history boundary", () => {
    const repo = repository(); repo.commits = repo.commits.filter((item) => item.oid !== oid(1)); repo.shallow = true; repo.truncated = true;
    const result = resolveBackbone(repo);
    expect(result).toMatchObject({ kind: "resolved", spine: [oid(4), oid(3)], missingParentOid: oid(1) });
    expect(result.spine).not.toContain(oid(2));
    expect(result.reason).toContain("Display default");
    expect(result.reason).toContain("earlier ancestry is unknown");
  });

  it.each([
    { name: "self cycle", parents: [[4]] },
    { name: "two-commit cycle", parents: [[3], [4]] },
  ])("detects malformed first-parent cycles safely: $name", ({ parents }) => {
    const repo = repository();
    repo.commits = parents.length === 1 ? [commit(4, parents[0])] : [commit(4, parents[0]), commit(3, parents[1])];
    const result = resolveBackbone(repo);
    expect(result).toMatchObject({ kind: "missing", tipOid: oid(4), spine: [], missingParentOid: null });
    expect(result.reason).toContain("cycle");
  });

  it("is deterministic when observations arrive in different input orders", () => {
    const repo = repository();
    const reordered = { ...repo, commits: [...repo.commits].reverse(), refs: [...repo.refs].reverse(), checkouts: [...repo.checkouts].reverse() };
    expect(resolveBackbone(reordered)).toEqual(resolveBackbone(repo));
    expect(resolveBackbone(reordered, integration(repo))).toEqual(resolveBackbone(repo, integration(repo)));
  });

  it("fails closed on conflicting same-name ref observations regardless of their order", () => {
    const repo = repository(); repo.refs.push(ref("refs/heads/main", 5));
    const result = resolveBackbone(repo);
    expect(result).toMatchObject({ kind: "missing", tipOid: null, spine: [] });
    expect(result.reason).toContain("Conflicting observations");
    expect(resolveBackbone({ ...repo, refs: [...repo.refs].reverse() })).toEqual(result);
  });

  it("does not mutate source facts or the separate authoritative policy", () => {
    const repo = deepFreeze(repository()), options = deepFreeze({ ...integration(repo), explicitRef: "refs/heads/main" });
    const beforeRepo = structuredClone(repo), beforeOptions = structuredClone(options);
    const result = resolveBackbone(repo, options);
    result.spine.reverse();
    expect(repo).toEqual(beforeRepo); expect(options).toEqual(beforeOptions);
    expect(resolveBackbone(repo, options).spine).toEqual([oid(4), oid(3), oid(1)]);
  });
});
