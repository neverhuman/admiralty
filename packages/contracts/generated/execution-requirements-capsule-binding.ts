/**
 * Generated from schemas/execution-requirements-capsule-binding.schema.json by json-schema-to-typescript@16.0.0.
 * Source SHA-256: ffb8b49ea9da6ce87a8c68120b850a155e34ea0dcf66e30e824b10eec88f59d0
 * Do not edit. Rust domain validation still applies at runtime.
 */

/**
 * This interface was referenced by `ExecutionRequirementsCapsuleBinding`'s JSON-Schema
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
 * This interface was referenced by `ExecutionRequirementsCapsuleBinding`'s JSON-Schema
 * via the `definition` "ResponsibleRole".
 */
export type ResponsibleRole =
  'worker' | 'verifier' | 'auditor' | 'captain' | 'super_captain' | 'owner';
/**
 * This interface was referenced by `ExecutionRequirementsCapsuleBinding`'s JSON-Schema
 * via the `definition` "TaskId".
 */
export type TaskId = string;

export interface ExecutionRequirementsCapsuleBinding {
  base_commit_sha: string;
  contract_digest: string;
  contract_revision: number;
  placement_request_digest: string;
  rendered_summary: string;
  required_delivery_stage: RequiredDeliveryBoundary;
  requirements_digest: string;
  responsible_role: ResponsibleRole;
  schema_version: 1;
  task_id: TaskId;
}
