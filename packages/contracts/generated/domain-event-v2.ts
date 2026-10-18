/**
 * Generated from schemas/domain-event-v2.schema.json by json-schema-to-typescript@16.0.0.
 * Source SHA-256: 0bf6b4e2e7a38ba563ee48207ec22c35b8fd9958449363d21edfb4371cd2fc48
 * Do not edit. Rust domain validation still applies at runtime.
 */

/**
 * Schema-only v2 union. The journal's `DomainEvent` is the Rust wire decoder;
 * this type deliberately has no serde codec or competing untagged decoder.
 * It retains old journal bytes without extending the frozen v1 schema.
 */
export type DomainEventV2 = DomainEvent | LifecycleRecord;
export type DomainEvent =
  | {
      contract: TaskContract;
      kind: 'task_submitted';
    }
  | {
      attempt_id: AttemptId;
      kind: 'task_claimed';
      owner: PrincipalId;
      revision: number;
      task_id: TaskId;
    }
  | {
      attempt_id: AttemptId;
      kind: 'task_started';
      task_id: TaskId;
    }
  | {
      attempt_id: AttemptId;
      candidate_ref: string;
      kind: 'candidate_submitted';
      task_id: TaskId;
    }
  | {
      kind: 'message_queued';
      message: CoordinationMessage;
    }
  | {
      context_epoch: string;
      kind: 'message_acknowledged';
      message_id: MessageId;
      recipient: PrincipalId;
    };
export type TaskId = string;
export type ProjectId = string;
export type RootId = string;
export type AttemptId = string;
export type PrincipalId = string;
export type MessageId = string;
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
export type LifecycleRecord = {
  command: LifecycleCommand;
  kind: 'task_lifecycle';
};
export type LifecycleAction =
  | {
      accepted: boolean;
      action: 'decide_candidate';
      attempt_id: AttemptId;
      candidate_ref: string;
      evidence: CandidateDecisionEvidence;
    }
  | {
      action: 'close_attempt';
      attempt_id: AttemptId;
      cancelled: boolean;
      closure_digest: string;
      reason: string;
    }
  | {
      action: 'retry';
      attempt_id: AttemptId;
      closure_digest: string;
    }
  | {
      action: 'cancel_pending';
      reason: string;
    }
  | {
      action: 'revise_contract';
      contract: TaskContract;
    };

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
export interface CheckSpec {
  args: string[];
  cwd: string;
  id: string;
  program: string;
  timeout_secs: number;
}
export interface AcceptanceCriterion {
  check_ids: string[];
  id: string;
  text: string;
}
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
 * A CAS binds all task state, including contract, candidate and attempt history.
 * This is untrusted input, never proof of the actor's authority or check execution.
 */
export interface LifecycleCommand {
  action: LifecycleAction;
  actor: PrincipalId;
  expected_task_digest: string;
  protocol_version: number;
  task_id: TaskId;
}
/**
 * Bind trusted verifier execution and custody to the exact candidate. Digests
 * and names alone are claims; Store's configured authority must authenticate
 * these against the runtime's non-deserializable execution evidence.
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
