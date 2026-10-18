/**
 * Generated from schemas/domain-event.schema.json by json-schema-to-typescript@16.0.0.
 * Source SHA-256: 2c3884e9f6fff328a22133fde98d94143cfd753e9a8b88c6047917283071cfc7
 * Do not edit. Rust domain validation still applies at runtime.
 */

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
