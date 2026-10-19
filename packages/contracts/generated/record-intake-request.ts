/**
 * Generated from schemas/record-intake-request.schema.json by json-schema-to-typescript@16.0.0.
 * Source SHA-256: c9089f4c598d4bdb76a8f6db25accc9e3f646087c6f4545b3e24397408b1facf
 * Do not edit. Rust domain validation still applies at runtime.
 */

/**
 * This interface was referenced by `RecordIntakeRequest`'s JSON-Schema
 * via the `definition` "CommandId".
 */
export type CommandId = string;
/**
 * This interface was referenced by `RecordIntakeRequest`'s JSON-Schema
 * via the `definition` "TaskId".
 */
export type TaskId = string;
/**
 * This interface was referenced by `RecordIntakeRequest`'s JSON-Schema
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
 * This interface was referenced by `RecordIntakeRequest`'s JSON-Schema
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
 * This interface was referenced by `RecordIntakeRequest`'s JSON-Schema
 * via the `definition` "DeliveryTargetKind".
 */
export type DeliveryTargetKind = 'repository' | 'deployment';
/**
 * This interface was referenced by `RecordIntakeRequest`'s JSON-Schema
 * via the `definition` "ProjectId".
 */
export type ProjectId = string;
/**
 * This interface was referenced by `RecordIntakeRequest`'s JSON-Schema
 * via the `definition` "RootId".
 */
export type RootId = string;
/**
 * This interface was referenced by `RecordIntakeRequest`'s JSON-Schema
 * via the `definition` "RequesterAttribution".
 */
export type RequesterAttribution =
  | {
      kind: 'unknown';
    }
  | {
      kind: 'claimed';
      requester: string;
    };
/**
 * This interface was referenced by `RecordIntakeRequest`'s JSON-Schema
 * via the `definition` "PrincipalId".
 */
export type PrincipalId = string;
/**
 * Source references are claims until the Store verifies bytes and an
 * interlocked lifetime pin. Merely decoding an ArtifactClaim grants no custody.
 *
 * This interface was referenced by `RecordIntakeRequest`'s JSON-Schema
 * via the `definition` "SourcePayload".
 */
export type SourcePayload =
  | {
      kind: 'inline_utf8';
      text: string;
    }
  | {
      artifact_ref: string;
      kind: 'artifact_claim';
      pin_receipt_ref: string;
      raw_digest: string;
      size_bytes: number;
    };
/**
 * This interface was referenced by `RecordIntakeRequest`'s JSON-Schema
 * via the `definition` "IntakeFormat".
 */
export type IntakeFormat = 'plain_text' | 'json_batch';

export interface RecordIntakeRequest {
  /**
   * Canonical positive decimal string on the public wire.
   */
  authority_generation: string;
  command_id: CommandId;
  expected_plan_revision: number;
  expected_work_order_revision: number;
  plan_id: string;
  policy_digest: string;
  policy_revision: string;
  preview_manifest_digest: string;
  schema_version: number;
  selected_tasks: SelectedTask[];
  work_order: WorkOrderRevision;
}
/**
 * This interface was referenced by `RecordIntakeRequest`'s JSON-Schema
 * via the `definition` "SelectedTask".
 */
export interface SelectedTask {
  binding: WorkOrderTaskBinding;
  delivery: DeliveryRequirement;
  task: TaskContract;
}
/**
 * This interface was referenced by `RecordIntakeRequest`'s JSON-Schema
 * via the `definition` "WorkOrderTaskBinding".
 */
export interface WorkOrderTaskBinding {
  contract_digest: string;
  contract_revision: number;
  outcome_digest: string;
  outcome_id: string;
  request_index: number;
  source_spans: SourceSpan[];
  task_id: TaskId;
  work_order_id: string;
  work_order_revision: number;
}
/**
 * This interface was referenced by `RecordIntakeRequest`'s JSON-Schema
 * via the `definition` "SourceSpan".
 */
export interface SourceSpan {
  end: number;
  start: number;
}
/**
 * Outside TaskContractv1. A generation is a new immutable requirement, never
 * a rewrite of prior evidence. Unknown is never optional/not-applicable.
 *
 * This interface was referenced by `RecordIntakeRequest`'s JSON-Schema
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
 * This interface was referenced by `RecordIntakeRequest`'s JSON-Schema
 * via the `definition` "DeliveryTarget".
 */
export interface DeliveryTarget {
  id: string;
  requirement: TargetRequirement;
  target_kind: DeliveryTargetKind;
}
/**
 * This interface was referenced by `RecordIntakeRequest`'s JSON-Schema
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
 * This interface was referenced by `RecordIntakeRequest`'s JSON-Schema
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
 * This interface was referenced by `RecordIntakeRequest`'s JSON-Schema
 * via the `definition` "AcceptanceCriterion".
 */
export interface AcceptanceCriterion {
  check_ids: string[];
  id: string;
  text: string;
}
/**
 * This interface was referenced by `RecordIntakeRequest`'s JSON-Schema
 * via the `definition` "WorkOrderRevision".
 */
export interface WorkOrderRevision {
  canonical_digest: string;
  constraints: string[];
  /**
   * Exact chosen parse mode, never inferred from JSON-looking source bytes.
   */
  format: 'plain_text' | 'json_batch';
  id: string;
  outcome?: string | null;
  parser_revision: string;
  provenance: SourceProvenance;
  rationale?: string | null;
  raw_digest: string;
  requested_timing?: string | null;
  revision: number;
  schema_version: number;
  scope: ScopeSnapshot;
  source_payload: SourcePayload;
  title?: string | null;
}
/**
 * This interface was referenced by `RecordIntakeRequest`'s JSON-Schema
 * via the `definition` "SourceProvenance".
 */
export interface SourceProvenance {
  /**
   * Supplied observation time; no wall-clock lookup or freshness claim.
   */
  captured_at: string;
  origin?: OriginReference | null;
  requester: RequesterAttribution;
  revision: string;
  schema_version: number;
  source_id: string;
}
/**
 * Opaque lookup data only: no fetching, path traversal or authorization.
 *
 * This interface was referenced by `RecordIntakeRequest`'s JSON-Schema
 * via the `definition` "OriginReference".
 */
export interface OriginReference {
  record_id: string;
  revision: string;
  system: string;
}
/**
 * This interface was referenced by `RecordIntakeRequest`'s JSON-Schema
 * via the `definition` "ScopeSnapshot".
 */
export interface ScopeSnapshot {
  /**
   * Canonical positive decimal string on the public wire; never a JS number.
   */
  authority_generation: string;
  authority_ref: string;
  principal: PrincipalId;
  project_id?: ProjectId | null;
  root_id?: RootId | null;
}
