import type { GitRepository } from "./model";

export type GitMergeParent = {
  oid: string;
  loaded: boolean;
  refs: string[];
};

export type GitMergeFacts = {
  commitOid: string;
  parentCount: number;
  firstParent: GitMergeParent;
  additionalParents: GitMergeParent[];
  /** Unknown unless the caller supplies an explicit resolved backbone set. */
  onBackbone: boolean | null;
};

/** Locale-independent ordering, including codepoints outside the BMP. */
function compareCodepoints(first: string, second: string): number {
  let a = 0, b = 0;
  while (a < first.length && b < second.length) {
    const left = first.codePointAt(a)!, right = second.codePointAt(b)!;
    if (left !== right) return left - right;
    a += left > 0xffff ? 2 : 1;
    b += right > 0xffff ? 2 : 1;
  }
  return first.length - second.length;
}

/** Actual multi-parent commits only. No message, branch name, reachability or
 * forge-status inference can turn a one-parent commit into a merge fact.
 * Indexing is O(V + P + R), plus comparison sorting for ref names and merge OIDs.
 */
export function createGitMergeIndex(
  repo: GitRepository,
  backboneOids?: ReadonlySet<string>,
): ReadonlyMap<string, GitMergeFacts> {
  const loaded = new Set(repo.commits.map((commit) => commit.oid));
  const refsByOid = new Map<string, string[]>();
  for (const ref of repo.refs) if (ref.commit_oid) {
    const names = refsByOid.get(ref.commit_oid) ?? [];
    names.push(ref.name); refsByOid.set(ref.commit_oid, names);
  }
  for (const names of refsByOid.values()) names.sort(compareCodepoints);
  const mergeCommits = new Map(repo.commits.filter((commit) => commit.parents.length >= 2).map((commit) => [commit.oid, commit]));
  const result = new Map<string, GitMergeFacts>();
  const parent = (oid: string): GitMergeParent => ({ oid, loaded: loaded.has(oid), refs: refsByOid.get(oid) ?? [] });
  for (const oid of [...mergeCommits.keys()].sort(compareCodepoints)) {
    const commit = mergeCommits.get(oid)!;
    result.set(oid, {
      commitOid: oid,
      parentCount: commit.parents.length,
      firstParent: parent(commit.parents[0]),
      additionalParents: commit.parents.slice(1).map(parent),
      onBackbone: backboneOids === undefined ? null : backboneOids.has(oid),
    });
  }
  return result;
}
