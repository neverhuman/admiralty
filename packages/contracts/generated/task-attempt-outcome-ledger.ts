/**
 * Generated from schemas/task-attempt-outcome-ledger.schema.json by json-schema-to-typescript@16.0.0.
 * Source SHA-256: a080f86d4e2a7b9c06c919b5d5a2758608f9af5945980c6d50a7ec688aee9514
 * Do not edit. Rust domain validation still applies at runtime.
 */

/**
 * This interface was referenced by `TaskAttemptOutcomeLedger`'s JSON-Schema
 * via the `definition` "AttemptId".
 */
export type AttemptId = string;
/**
 * This interface was referenced by `TaskAttemptOutcomeLedger`'s JSON-Schema
 * via the `definition` "PrincipalId".
 */
export type PrincipalId = string;
/**
 * This interface was referenced by `TaskAttemptOutcomeLedger`'s JSON-Schema
 * via the `definition` "ProjectId".
 */
export type ProjectId = string;
/**
 * This interface was referenced by `TaskAttemptOutcomeLedger`'s JSON-Schema
 * via the `definition` "RootId".
 */
export type RootId = string;
/**
 * This interface was referenced by `TaskAttemptOutcomeLedger`'s JSON-Schema
 * via the `definition` "AttemptOutcomeStatus".
 */
export type AttemptOutcomeStatus = 'unknown' | 'succeeded' | 'failed' | 'cancelled';
/**
 * This interface was referenced by `TaskAttemptOutcomeLedger`'s JSON-Schema
 * via the `definition` "TaskId".
 */
export type TaskId = string;
/**
 * This interface was referenced by `TaskAttemptOutcomeLedger`'s JSON-Schema
 * via the `definition` "TaskSchedulingDimension".
 */
export type TaskSchedulingDimension =
  'blocked' | 'ready' | 'claimed' | 'running' | 'awaiting_decision' | 'closed';

export interface TaskAttemptOutcomeLedger {
  active_attempt_id?: AttemptId | null;
  attempt_order: AttemptId[];
  current_contract_digest: string;
  current_contract_revision: number;
  max_attempts: number;
  outcomes: {
    [k: string]: AttemptOutcomeRecord;
  };
  pending_cancel_reason?: string | null;
  project_id: ProjectId;
  root_id: RootId;
  scheduling: TaskSchedulingDimension;
  schema_version: 1;
  task_id: TaskId;
}
/**
 * This interface was referenced by `TaskAttemptOutcomeLedger`'s JSON-Schema
 * via the `definition` "AttemptOutcomeRecord".
 */
export interface AttemptOutcomeRecord {
  attempt_id: AttemptId;
  attempt_index: number;
  candidate_ref?: string | null;
  closure_digest?: string | null;
  contract_digest: string;
  contract_revision: number;
  decision_evidence?: CandidateDecisionEvidence | null;
  owner: PrincipalId;
  project_id: ProjectId;
  reason?: string | null;
  root_id: RootId;
  schema_version: number;
  started: boolean;
  status: AttemptOutcomeStatus;
  superseded_by_retry: boolean;
  task_id: TaskId;
}
/**
 * Bind trusted verifier execution and custody to the exact candidate. Digests
 * and names alone are claims; Store's configured authority must authenticate
 * these against the runtime's non-deserializable execution evidence.
 *
 * This interface was referenced by `TaskAttemptOutcomeLedger`'s JSON-Schema
 * via the `definition` "CandidateDecisionEvidence".
 */
export interface CandidateDecisionEvidence {
  /**
   * Independent supervisor closure/fencing evidence, not the check receipt.
   */
  closure_digest: string;
  contract_digest: string;
  custody_digest: string;
  receipt_digest: string;
  source_commit_sha: string;
  source_tree_sha: string;
  verification_policy_digest: string;
  verifier: PrincipalId;
}
