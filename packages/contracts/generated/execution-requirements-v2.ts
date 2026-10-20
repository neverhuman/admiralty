/**
 * Generated from schemas/execution-requirements-v2.schema.json by json-schema-to-typescript@16.0.0.
 * Source SHA-256: db61cb5822dae555de32ebf1eb8c26f5cd4bdbcee74e7c8415b4578a0f1e42f6
 * Do not edit. Rust domain validation still applies at runtime.
 */

/**
 * This interface was referenced by `ExecutionRequirementsV2`'s JSON-Schema
 * via the `definition` "ProjectId".
 */
export type ProjectId = string;
/**
 * This interface was referenced by `ExecutionRequirementsV2`'s JSON-Schema
 * via the `definition` "RequiredDeliveryBoundary".
 */
export type RequiredDeliveryBoundary =
  | 'source_only'
  | 'verified_candidate'
  | 'merged_on_main'
  | 'packaged'
  | 'staged'
  | 'canary_qualified'
  | 'released_and_running';
/**
 * This interface was referenced by `ExecutionRequirementsV2`'s JSON-Schema
 * via the `definition` "ResponsibleRole".
 */
export type ResponsibleRole =
  'worker' | 'verifier' | 'auditor' | 'captain' | 'super_captain' | 'owner';
/**
 * This interface was referenced by `ExecutionRequirementsV2`'s JSON-Schema
 * via the `definition` "RootId".
 */
export type RootId = string;
/**
 * This interface was referenced by `ExecutionRequirementsV2`'s JSON-Schema
 * via the `definition` "DecimalU64".
 */
export type DecimalU64 = string;
/**
 * This interface was referenced by `ExecutionRequirementsV2`'s JSON-Schema
 * via the `definition` "ExecutionRunnerMode".
 */
export type ExecutionRunnerMode = 'trusted_local_worktree' | 'private_clone_rootless_oci';
/**
 * This interface was referenced by `ExecutionRequirementsV2`'s JSON-Schema
 * via the `definition` "ShellBoundaryPolicy".
 */
export type ShellBoundaryPolicy =
  | {
      mode: 'forbid_shell';
    }
  | {
      interpreter_program: string;
      justification: string;
      mode: 'declared_interpreter';
      trust_boundary_ref: string;
    };
/**
 * This interface was referenced by `ExecutionRequirementsV2`'s JSON-Schema
 * via the `definition` "TaskId".
 */
export type TaskId = string;

export interface ExecutionRequirementsV2 {
  base_commit_sha: string;
  contract_digest: string;
  contract_revision: number;
  deadline_utc: string;
  environment_allowlist: string[];
  max_wall_clock_secs: number;
  project_id: ProjectId;
  required_delivery_stage: RequiredDeliveryBoundary;
  responsible_role: ResponsibleRole;
  root_id: RootId;
  runner: ExecutionRunnerRequirements;
  schema_version: 1;
  shell_boundary: ShellBoundaryPolicy;
  task_id: TaskId;
}
/**
 * This interface was referenced by `ExecutionRequirementsV2`'s JSON-Schema
 * via the `definition` "ExecutionRunnerRequirements".
 */
export interface ExecutionRunnerRequirements {
  budget_requirement: BudgetRequirement;
  locality_refs: string[];
  required_capabilities: string[];
  runner_mode: ExecutionRunnerMode;
}
/**
 * This interface was referenced by `ExecutionRequirementsV2`'s JSON-Schema
 * via the `definition` "BudgetRequirement".
 */
export interface BudgetRequirement {
  account_pool: string;
  amount: DecimalU64;
}
