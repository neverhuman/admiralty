/**
 * Generated from schemas/intake-preview.schema.json by json-schema-to-typescript@16.0.0.
 * Source SHA-256: 48a4c8b3a1c7c61a136f62d5ba66517e3fcc1566a694dc915bd5aef176482a9e
 * Do not edit. Rust domain validation still applies at runtime.
 */

/**
 * This interface was referenced by `IntakePreview`'s JSON-Schema
 * via the `definition` "PrincipalId".
 */
export type PrincipalId = string;
/**
 * This interface was referenced by `IntakePreview`'s JSON-Schema
 * via the `definition` "ProjectId".
 */
export type ProjectId = string;
/**
 * This interface was referenced by `IntakePreview`'s JSON-Schema
 * via the `definition` "RootId".
 */
export type RootId = string;
/**
 * This interface was referenced by `IntakePreview`'s JSON-Schema
 * via the `definition` "IntakeFormat".
 */
export type IntakeFormat = 'plain_text' | 'json_batch';
/**
 * This interface was referenced by `IntakePreview`'s JSON-Schema
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
 * This interface was referenced by `IntakePreview`'s JSON-Schema
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
 * This interface was referenced by `IntakePreview`'s JSON-Schema
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
 * This interface was referenced by `IntakePreview`'s JSON-Schema
 * via the `definition` "DeliveryTargetKind".
 */
export type DeliveryTargetKind = 'repository' | 'deployment';
/**
 * This interface was referenced by `IntakePreview`'s JSON-Schema
 * via the `definition` "TaskId".
 */
export type TaskId = string;
/**
 * This interface was referenced by `IntakePreview`'s JSON-Schema
 * via the `definition` "UnresolvedFact".
 */
export type UnresolvedFact =
  | 'scope_unknown'
  | 'claimed_scope_mismatch'
  | 'attribution_unknown'
  | 'outcome_unknown'
  | 'task_contract_missing'
  | 'criteria_unknown'
  | 'source_spans_unknown'
  | 'task_scope_mismatch'
  | 'delivery_unknown';

export interface IntakePreview {
  authorized_scope: ScopeSnapshot;
  canonical_digest: string;
  format: IntakeFormat;
  /**
   * Binds all source spans, provenance, policy and authorized scope too.
   */
  manifest_digest: string;
  parser_revision: string;
  /**
   * Domain-separated identity of the exact supplied revision and all caps.
   */
  policy_digest: string;
  policy_revision: string;
  provenance: SourceProvenance;
  raw_byte_length: number;
  raw_digest: string;
  requests: RequestPreview[];
  schema_version: number;
}
/**
 * This interface was referenced by `IntakePreview`'s JSON-Schema
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
/**
 * This interface was referenced by `IntakePreview`'s JSON-Schema
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
 * This interface was referenced by `IntakePreview`'s JSON-Schema
 * via the `definition` "OriginReference".
 */
export interface OriginReference {
  record_id: string;
  revision: string;
  system: string;
}
/**
 * This interface was referenced by `IntakePreview`'s JSON-Schema
 * via the `definition` "RequestPreview".
 */
export interface RequestPreview {
  claimed_project_id?: ProjectId | null;
  constraints: string[];
  index: number;
  outcomes: OutcomePreview[];
  rationale?: string | null;
  raw_span: SourceSpan1;
  requested_timing?: string | null;
  text: string;
  title?: string | null;
  unresolved: UnresolvedFact[];
}
/**
 * This interface was referenced by `IntakePreview`'s JSON-Schema
 * via the `definition` "OutcomePreview".
 */
export interface OutcomePreview {
  contract_digest?: string | null;
  outcome: OutcomeInput;
  /**
   * Domain-separated exact outcome including its raw provenance spans.
   */
  outcome_digest: string;
  unresolved: UnresolvedFact[];
}
/**
 * This interface was referenced by `IntakePreview`'s JSON-Schema
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
 * This interface was referenced by `IntakePreview`'s JSON-Schema
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
 * This interface was referenced by `IntakePreview`'s JSON-Schema
 * via the `definition` "DeliveryTarget".
 */
export interface DeliveryTarget {
  id: string;
  requirement: TargetRequirement;
  target_kind: DeliveryTargetKind;
}
/**
 * This interface was referenced by `IntakePreview`'s JSON-Schema
 * via the `definition` "SourceSpan".
 */
export interface SourceSpan {
  end: number;
  start: number;
}
/**
 * This interface was referenced by `IntakePreview`'s JSON-Schema
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
 * This interface was referenced by `IntakePreview`'s JSON-Schema
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
 * This interface was referenced by `IntakePreview`'s JSON-Schema
 * via the `definition` "AcceptanceCriterion".
 */
export interface AcceptanceCriterion {
  check_ids: string[];
  id: string;
  text: string;
}
/**
 * Whole plain-text input or exact raw JSON item object, including syntax.
 */
export interface SourceSpan1 {
  end: number;
  start: number;
}
