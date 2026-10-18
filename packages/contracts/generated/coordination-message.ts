/**
 * Generated from schemas/coordination-message.schema.json by json-schema-to-typescript@16.0.0.
 * Source SHA-256: d256d8affcf471048800130daba6a42651995c7875051ecb0ad0999fb7a79d26
 * Do not edit. Rust domain validation still applies at runtime.
 */

/**
 * This interface was referenced by `CoordinationMessage`'s JSON-Schema
 * via the `definition` "MessageId".
 */
export type MessageId = string;
/**
 * This interface was referenced by `CoordinationMessage`'s JSON-Schema
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
 * This interface was referenced by `CoordinationMessage`'s JSON-Schema
 * via the `definition` "TaskId".
 */
export type TaskId = string;
/**
 * This interface was referenced by `CoordinationMessage`'s JSON-Schema
 * via the `definition` "ProjectId".
 */
export type ProjectId = string;
/**
 * This interface was referenced by `CoordinationMessage`'s JSON-Schema
 * via the `definition` "PrincipalId".
 */
export type PrincipalId = string;

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
