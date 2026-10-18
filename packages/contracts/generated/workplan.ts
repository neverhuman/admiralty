/**
 * Generated from schemas/workplan.schema.json by json-schema-to-typescript@16.0.0.
 * Source SHA-256: f00c1f4a6a6a5603e5c48ddce10b38dbd7390e27057a3294a843bc0054e0eaf5
 * Do not edit. Rust domain validation still applies at runtime.
 */

/**
 * This interface was referenced by `Workplan`'s JSON-Schema
 * via the `definition` "ProjectId".
 */
export type ProjectId = string;
/**
 * This interface was referenced by `Workplan`'s JSON-Schema
 * via the `definition` "TaskId".
 */
export type TaskId = string;
/**
 * This interface was referenced by `Workplan`'s JSON-Schema
 * via the `definition` "RootId".
 */
export type RootId = string;

export interface Workplan {
  plan_id: string;
  project_id: ProjectId;
  revision: number;
  schema_version: number;
  /**
   * @maxItems 1000
   */
  tasks: TaskContract[];
}
/**
 * This interface was referenced by `Workplan`'s JSON-Schema
 * via the `definition` "TaskContract".
 */
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
 * This interface was referenced by `Workplan`'s JSON-Schema
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
 * This interface was referenced by `Workplan`'s JSON-Schema
 * via the `definition` "AcceptanceCriterion".
 */
export interface AcceptanceCriterion {
  check_ids: string[];
  id: string;
  text: string;
}
