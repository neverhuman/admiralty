/**
 * Generated from schemas/task-contract.schema.json by json-schema-to-typescript@16.0.0.
 * Source SHA-256: e092c83e2755e01574391921cbe5830d27a38ed8dfe6013eac337c83f40bcb38
 * Do not edit. Rust domain validation still applies at runtime.
 */

/**
 * This interface was referenced by `TaskContract`'s JSON-Schema
 * via the `definition` "TaskId".
 */
export type TaskId = string;
/**
 * This interface was referenced by `TaskContract`'s JSON-Schema
 * via the `definition` "ProjectId".
 */
export type ProjectId = string;
/**
 * This interface was referenced by `TaskContract`'s JSON-Schema
 * via the `definition` "RootId".
 */
export type RootId = string;

export interface TaskContract {
  authority_ref: string;
  checks: CheckSpec[];
  criteria: AcceptanceCriterion[];
  dependencies: TaskId[];
  forbidden_paths: string[];
  max_attempts: number;
  non_goals: string[];
  objective: string;
  owned_paths: string[];
  project_id: ProjectId;
  read_first: string[];
  revision: number;
  root_id: RootId;
  schema_version: number;
  simulated: boolean;
  task_id: TaskId;
  title: string;
}
/**
 * This interface was referenced by `TaskContract`'s JSON-Schema
 * via the `definition` "CheckSpec".
 */
export interface CheckSpec {
  args: string[];
  cwd: string;
  id: string;
  program: string;
  timeout_secs: number;
}
/**
 * This interface was referenced by `TaskContract`'s JSON-Schema
 * via the `definition` "AcceptanceCriterion".
 */
export interface AcceptanceCriterion {
  check_ids: string[];
  id: string;
  text: string;
}
