/**
 * Generated from schemas/intake-batch.schema.json by json-schema-to-typescript@16.0.0.
 * Source SHA-256: a2142b402d6ab14748a86a495eea700a3e2d1aec1170798b54a443c8538889ee
 * Do not edit. Rust domain validation still applies at runtime.
 */

/**
 * This interface was referenced by `IntakeBatch`'s JSON-Schema
 * via the `definition` "ProjectId".
 */
export type ProjectId = string;
/**
 * This interface was referenced by `IntakeBatch`'s JSON-Schema
 * via the `definition` "TargetRequirement".
 */
export type TargetRequirement =
  | {
      kind: 'unknown';
    }
  | {
      kind: 'required';
      stage: DeliveryStage;
    }
  | {
      kind: 'optional';
      stage: DeliveryStage;
    }
  | {
      kind: 'not_applicable';
      reason: string;
    };
/**
 * This interface was referenced by `IntakeBatch`'s JSON-Schema
 * via the `definition` "DeliveryStage".
 */
export type DeliveryStage =
  | 'candidate'
  | 'verified'
  | 'merged'
  | 'packaged'
  | 'staged'
  | 'installed'
  | 'running'
  | 'released';
/**
 * This interface was referenced by `IntakeBatch`'s JSON-Schema
 * via the `definition` "DeliveryTargetKind".
 */
export type DeliveryTargetKind = 'repository' | 'deployment';
/**
 * This interface was referenced by `IntakeBatch`'s JSON-Schema
 * via the `definition` "TaskId".
 */
export type TaskId = string;
/**
 * This interface was referenced by `IntakeBatch`'s JSON-Schema
 * via the `definition` "RootId".
 */
export type RootId = string;

export interface IntakeBatch {
  items: IntakeItem[];
  schema_version: number;
}
/**
 * This interface was referenced by `IntakeBatch`'s JSON-Schema
 * via the `definition` "IntakeItem".
 */
export interface IntakeItem {
  claimed_project_id?: ProjectId | null;
  constraints?: string[];
  outcomes?: OutcomeInput[];
  rationale?: string | null;
  requested_timing?: string | null;
  text: string;
  title?: string | null;
}
/**
 * This interface was referenced by `IntakeBatch`'s JSON-Schema
 * via the `definition` "OutcomeInput".
 */
export interface OutcomeInput {
  constraints?: string[];
  delivery?: DeliveryRequirement | null;
  description: string;
  id: string;
  /**
   * Raw source byte offsets, NOT decoded `text` offsets. Empty is unresolved.
   */
  source_spans?: SourceSpan[];
  task?: TaskContract | null;
}
/**
 * Outside TaskContractv1. A generation is a new immutable requirement, never
 * a rewrite of prior evidence. Unknown is never optional/not-applicable.
 *
 * This interface was referenced by `IntakeBatch`'s JSON-Schema
 * via the `definition` "DeliveryRequirement".
 */
export interface DeliveryRequirement {
  contract_digest: string;
  contract_revision: number;
  generation: number;
  schema_version: number;
  targets: DeliveryTarget[];
  task_id: TaskId;
}
/**
 * This interface was referenced by `IntakeBatch`'s JSON-Schema
 * via the `definition` "DeliveryTarget".
 */
export interface DeliveryTarget {
  id: string;
  requirement: TargetRequirement;
  target_kind: DeliveryTargetKind;
}
/**
 * This interface was referenced by `IntakeBatch`'s JSON-Schema
 * via the `definition` "SourceSpan".
 */
export interface SourceSpan {
  end: number;
  start: number;
}
/**
 * This interface was referenced by `IntakeBatch`'s JSON-Schema
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
 * This interface was referenced by `IntakeBatch`'s JSON-Schema
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
 * This interface was referenced by `IntakeBatch`'s JSON-Schema
 * via the `definition` "AcceptanceCriterion".
 */
export interface AcceptanceCriterion {
  check_ids: string[];
  id: string;
  text: string;
}
