/**
 * Generated from schemas/views-snapshot.schema.json by json-schema-to-typescript@16.0.0.
 * Source SHA-256: 56e65a94e54997604f91dfdac0344f3eb25a77e377e94c500395fecd4eaa27a7
 * Do not edit. Rust domain validation still applies at runtime.
 */

/**
 * This interface was referenced by `ViewsSnapshot`'s JSON-Schema
 * via the `definition` "ViewPresence".
 */
export type ViewPresence = 'observed' | 'unobserved' | 'ended';
/**
 * This interface was referenced by `ViewsSnapshot`'s JSON-Schema
 * via the `definition` "ViewRole".
 */
export type ViewRole = 'owner' | 'watcher' | 'admiral' | 'commodore' | 'worker' | 'assignment';

export interface ViewsSnapshot {
  agents: ViewAgent[];
  observed_utc: string;
  projects: ViewProject[];
  revision: string;
  schema_version: number;
  simulated: boolean;
  sources: ViewSource[];
}
/**
 * This interface was referenced by `ViewsSnapshot`'s JSON-Schema
 * via the `definition` "ViewAgent".
 */
export interface ViewAgent {
  branch?: string | null;
  effort?: string | null;
  id: string;
  model?: string | null;
  model_source: string;
  name: string;
  node?: string | null;
  observed_utc?: string | null;
  parent_id?: string | null;
  presence: ViewPresence;
  project_id?: string | null;
  requested_model?: string | null;
  role: ViewRole;
  source: string;
  state: string;
  task?: string | null;
}
/**
 * This interface was referenced by `ViewsSnapshot`'s JSON-Schema
 * via the `definition` "ViewProject".
 */
export interface ViewProject {
  git?: GitRepository | null;
  id: string;
  name: string;
  task_count?: number | null;
}
/**
 * This interface was referenced by `ViewsSnapshot`'s JSON-Schema
 * via the `definition` "GitRepository".
 */
export interface GitRepository {
  checkouts: GitCheckout[];
  commits: GitCommit[];
  error?: string | null;
  instance_id: string;
  node: string;
  observed_utc: string;
  refs: GitRef[];
  shallow: boolean;
  truncated: boolean;
}
/**
 * This interface was referenced by `ViewsSnapshot`'s JSON-Schema
 * via the `definition` "GitCheckout".
 */
export interface GitCheckout {
  branch?: string | null;
  detached: boolean;
  head?: string | null;
  id: string;
  kind: string;
  locked: boolean;
  name: string;
  prunable: boolean;
  status?: GitWorkingState | null;
}
/**
 * This interface was referenced by `ViewsSnapshot`'s JSON-Schema
 * via the `definition` "GitWorkingState".
 */
export interface GitWorkingState {
  conflicts: number;
  staged: number;
  unstaged: number;
  untracked: number;
}
/**
 * This interface was referenced by `ViewsSnapshot`'s JSON-Schema
 * via the `definition` "GitCommit".
 */
export interface GitCommit {
  committed_utc: string;
  oid: string;
  parents: string[];
  subject: string;
}
/**
 * This interface was referenced by `ViewsSnapshot`'s JSON-Schema
 * via the `definition` "GitRef".
 */
export interface GitRef {
  commit_oid?: string | null;
  kind: string;
  name: string;
  oid: string;
}
/**
 * This interface was referenced by `ViewsSnapshot`'s JSON-Schema
 * via the `definition` "ViewSource".
 */
export interface ViewSource {
  coverage: string;
  id: string;
  label: string;
  observed_utc?: string | null;
  state: string;
}
