import type {
  BackboneOptions,
  BackboneResolution,
  BackboneSource,
  GitRepository,
} from "./model";

const LOCAL_MAIN = "refs/heads/main";

/** Choose a display spine from observed refs without changing integration policy. */
export function resolveBackbone(
  repo: GitRepository,
  options: BackboneOptions = {},
): BackboneResolution {
  const explicitRef = options.explicitRef?.trim();
  const policy = options.integration;
  const policyMatches = policy?.instanceId === repo.instance_id;
  const ignoredPolicy = !explicitRef && policy && !policyMatches
    ? `Integration policy belongs to repository instance ${policy.instanceId}, not ${repo.instance_id}; ignored for this display. `
    : "";
  const source: BackboneSource = explicitRef
    ? "explicit"
    : policyMatches ? "integration" : "display-default";
  const refName = explicitRef || (policyMatches ? policy!.refName : LOCAL_MAIN);
  const refs = repo.refs.filter((ref) => ref.name === refName);
  const result = (
    kind: BackboneResolution["kind"],
    tipOid: string | null,
    reason: string,
    spine: string[] = [],
    missingParentOid: string | null = null,
  ): BackboneResolution => ({
    kind,
    source,
    refName,
    tipOid,
    spine,
    missingParentOid,
    reason: ignoredPolicy + (source === "display-default" ? "Display default: local main. " : "") + reason,
  });

  if (!refs.length) {
    if (source === "display-default") {
      return {
        kind: "unresolved",
        source: null,
        refName: null,
        tipOid: null,
        spine: [],
        missingParentOid: null,
        reason: ignoredPolicy + `No observed local ${LOCAL_MAIN}; choose a display backbone.`,
      };
    }
    return result("missing", null, `Selected display ref ${refName} is missing from this snapshot.`);
  }

  const ref = refs[0];
  if (refs.some((candidate) => candidate.oid !== ref.oid || candidate.commit_oid !== ref.commit_oid)) {
    return result("missing", null, `Conflicting observations for display ref ${refName}; no tip selected.`);
  }
  // An annotated tag's object OID is not its peeled commit. Never substitute it.
  const tipOid = ref.commit_oid;
  if (!tipOid) {
    return result("missing", null, `Selected display ref ${refName} does not resolve to an observed commit.`);
  }
  if (source === "integration") {
    if (!policy!.oid) {
      return result("missing", tipOid, `Integration ref ${refName} has no snapshot-bound OID; display backbone is unknown.`);
    }
    if (policy!.oid !== tipOid) {
      return result("missing", tipOid, `Integration ref ${refName} is stale: policy OID ${policy!.oid} differs from observed commit ${tipOid}.`);
    }
  }

  const commits = new Map(repo.commits.map((commit) => [commit.oid, commit]));
  if (!commits.has(tipOid)) {
    return result("missing", tipOid, `Display ref ${refName} points to commit ${tipOid}, which is not loaded.`);
  }

  const spine: string[] = [];
  const seen = new Set<string>();
  let current: string | undefined = tipOid;
  while (current !== undefined) {
    if (seen.has(current)) {
      return result("missing", tipOid, `Invalid first-parent cycle at commit ${current}; no display spine selected.`);
    }
    const commit = commits.get(current);
    if (!commit) {
      return result("resolved", tipOid, `Loaded first-parent backbone stops at missing parent ${current}; earlier ancestry is unknown.`, spine, current);
    }
    seen.add(current);
    spine.push(current);
    current = commit.parents[0];
  }
  return result("resolved", tipOid, `Loaded first-parent backbone resolved from ${refName}.`, spine);
}
