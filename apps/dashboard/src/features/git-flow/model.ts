import type { GitCommit, GitRepository } from "../../../../../packages/contracts/generated/views-snapshot";
export type { GitRepository, GitCommit, GitCheckout } from "../../../../../packages/contracts/generated/views-snapshot";
export type CommitRange = { id: string; commits: GitCommit[]; expanded: boolean; x: number; y: number };
export type CommitMark = { x: number; y: number; commit: GitCommit; lane: number; rank: number; range?: CommitRange };
export type GitLayout = { marks: CommitMark[]; byOid: Map<string, CommitMark>; width: number; height: number; boundaries: number; ranges: CommitRange[]; orderedCommits: GitCommit[] };
export type GraphBounds = { minX: number; minY: number; maxX: number; maxY: number };
export type BackboneSource = "explicit" | "integration" | "display-default";
export type BackbonePolicy = { instanceId: string; refName: string; oid: string | null };
export type BackboneOptions = { explicitRef?: string; integration?: BackbonePolicy };
export type BackboneResolution = {
  kind: "resolved" | "missing" | "unresolved";
  source: BackboneSource | null;
  refName: string | null;
  tipOid: string | null;
  /** Tip first; only loaded, actual first-parent ancestry. */
  spine: string[];
  missingParentOid: string | null;
  reason: string;
};
export type GitPath = {
  id: string;
  oids: string[];
  lane: number;
  startRank: number;
  endRank: number;
};
export type GraphEdge = {
  id: string;
  parentOid: string;
  childOid: string;
  parent: CommitMark | null;
  child: CommitMark;
  parentIndex: number;
  kind: "backbone" | "parent" | "merge-parent" | "missing";
  /** Actual additional-parent edge into a commit on the selected display spine. */
  backboneReturn?: boolean;
};
export type GitGraphLayout = GitLayout & {
  bounds: GraphBounds;
  backbone: BackboneResolution;
  backboneOids: ReadonlySet<string>;
  paths: GitPath[];
  edges: GraphEdge[];
  refsByOid: Map<string, GitRepository["refs"]>;
  checkoutsByOid: Map<string, GitRepository["checkouts"]>;
};
export type GraphLayoutOptions = BackboneOptions & {
  expandedOids?: ReadonlySet<string>;
  collapsedOids?: ReadonlySet<string>;
  /** Reuse a path's lane when its updated interval still fits. */
  previousPaths?: readonly GitPath[];
};
