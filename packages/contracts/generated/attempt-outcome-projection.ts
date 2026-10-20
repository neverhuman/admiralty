/**
 * Generated from schemas/attempt-outcome-projection.schema.json by json-schema-to-typescript@16.0.0.
 * Source SHA-256: b47cfec4d39699affcad51ed13fe6ec944363b65055de62d8475efee7dbd6932
 * Do not edit. Rust domain validation still applies at runtime.
 */

/**
 * This interface was referenced by `AttemptOutcomeProjection`'s JSON-Schema
 * via the `definition` "MessageId".
 */
export type MessageId = string;
/**
 * This interface was referenced by `AttemptOutcomeProjection`'s JSON-Schema
 * via the `definition` "MessagePayload".
 */
export type MessagePayload =
  | {
      contract_digest: string;
      kind: 'task_assigned';
      revision: number;
      task_id: TaskId;
    }
  | {
      kind: 'question';
      options: string[];
      question: string;
      requirement: string;
      task_id: TaskId;
    }
  | {
      answer: string;
      decision_ref?: string | null;
      kind: 'answer';
      question_id: MessageId;
    }
  | {
      artifact_ref: string;
      interface_revision: string;
      kind: 'artifact_ready';
      task_id: TaskId;
    }
  | {
      candidate_ref: string;
      failed_check?: string | null;
      kind: 'handoff';
      next_action: string;
      task_id: TaskId;
    };
/**
 * This interface was referenced by `AttemptOutcomeProjection`'s JSON-Schema
 * via the `definition` "TaskId".
 */
export type TaskId = string;
/**
 * This interface was referenced by `AttemptOutcomeProjection`'s JSON-Schema
 * via the `definition` "ProjectId".
 */
export type ProjectId = string;
/**
 * This interface was referenced by `AttemptOutcomeProjection`'s JSON-Schema
 * via the `definition` "PrincipalId".
 */
export type PrincipalId = string;
/**
 * This interface was referenced by `AttemptOutcomeProjection`'s JSON-Schema
 * via the `definition` "AttemptId".
 */
export type AttemptId = string;
/**
 * This interface was referenced by `AttemptOutcomeProjection`'s JSON-Schema
 * via the `definition` "RootId".
 */
export type RootId = string;
/**
 * This interface was referenced by `AttemptOutcomeProjection`'s JSON-Schema
 * via the `definition` "SchedulingState".
 */
export type SchedulingState =
  | 'blocked'
  | 'ready'
  | 'claimed'
  | 'running'
  | 'awaiting_verification'
  | 'accepted'
  | 'failed'
  | 'cancelled';
/**
 * This interface was referenced by `AttemptOutcomeProjection`'s JSON-Schema
 * via the `definition` "AttemptOutcomeStatus".
 */
export type AttemptOutcomeStatus = 'unknown' | 'succeeded' | 'failed' | 'cancelled';
/**
 * This interface was referenced by `AttemptOutcomeProjection`'s JSON-Schema
 * via the `definition` "TaskSchedulingDimension".
 */
export type TaskSchedulingDimension =
  'blocked' | 'ready' | 'claimed' | 'running' | 'awaiting_decision' | 'closed';

/**
 * Deterministic projection of both the closed v1 `Projection` and the independent
 * per-task `TaskAttemptOutcomeLedger` map across an ordered event stream.
 */
export interface AttemptOutcomeProjection {
  projection: Projection;
  task_ledgers: {
    [k: string]: TaskAttemptOutcomeLedger;
  };
}
/**
 * This interface was referenced by `AttemptOutcomeProjection`'s JSON-Schema
 * via the `definition` "Projection".
 */
export interface Projection {
  acknowledgments: {
    [k: string]: string;
  };
  messages: {
    [k: string]: CoordinationMessage;
  };
  tasks: {
    [k: string]: TaskView;
  };
  [k: string]: unknown;
}
/**
 * This interface was referenced by `AttemptOutcomeProjection`'s JSON-Schema
 * via the `definition` "CoordinationMessage".
 */
export interface CoordinationMessage {
  authority_ref: string;
  causation_id?: MessageId | null;
  context_epoch: string;
  message_id: MessageId;
  payload: MessagePayload;
  project_id: ProjectId;
  recipient: PrincipalId;
  schema_version: number;
  sender: PrincipalId;
}
/**
 * This interface was referenced by `AttemptOutcomeProjection`'s JSON-Schema
 * via the `definition` "TaskView".
 */
export interface TaskView {
  attempt_id?: AttemptId | null;
  candidate_ref?: string | null;
  contract: TaskContract;
  /**
   * Absent for unchanged v1 projections; any native metadata requires v2.
   */
  lifecycle?: TaskLifecycleView | null;
  owner?: PrincipalId | null;
  state: SchedulingState;
  [k: string]: unknown;
}
/**
 * This interface was referenced by `AttemptOutcomeProjection`'s JSON-Schema
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
 * This interface was referenced by `AttemptOutcomeProjection`'s JSON-Schema
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
 * This interface was referenced by `AttemptOutcomeProjection`'s JSON-Schema
 * via the `definition` "AcceptanceCriterion".
 */
export interface AcceptanceCriterion {
  check_ids: string[];
  id: string;
  text: string;
}
/**
 * This interface was referenced by `AttemptOutcomeProjection`'s JSON-Schema
 * via the `definition` "TaskLifecycleView".
 */
export interface TaskLifecycleView {
  attempts: {
    [k: string]: PrincipalId;
  };
  closure_digest?: string | null;
  decision?: CandidateDecisionEvidence | null;
  reason?: string | null;
  revision: number;
}
/**
 * Bind trusted verifier execution and custody to the exact candidate. Digests
 * and names alone are claims; Store's configured authority must authenticate
 * these against the runtime's non-deserializable execution evidence.
 *
 * This interface was referenced by `AttemptOutcomeProjection`'s JSON-Schema
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
/**
 * This interface was referenced by `AttemptOutcomeProjection`'s JSON-Schema
 * via the `definition` "TaskAttemptOutcomeLedger".
 */
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
 * This interface was referenced by `AttemptOutcomeProjection`'s JSON-Schema
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
