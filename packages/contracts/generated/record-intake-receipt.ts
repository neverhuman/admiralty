/**
 * Generated from schemas/record-intake-receipt.schema.json by json-schema-to-typescript@16.0.0.
 * Source SHA-256: 97bea61493a09eb65dbcfe8c1fe6b4afd9ebc314719f3e4a27d1202ccfe4f664
 * Do not edit. Rust domain validation still applies at runtime.
 */

/**
 * This interface was referenced by `RecordIntakeReceipt`'s JSON-Schema
 * via the `definition` "CommandId".
 */
export type CommandId = string;
/**
 * This interface was referenced by `RecordIntakeReceipt`'s JSON-Schema
 * via the `definition` "TaskId".
 */
export type TaskId = string;

export interface RecordIntakeReceipt {
  command_id: CommandId;
  /**
   * True only when the writer returns an earlier identical command receipt.
   */
  duplicate: boolean;
  plan_id: string;
  plan_revision: number;
  preview_manifest_digest: string;
  request_digest: string;
  schema_version: number;
  task_bindings: WorkOrderTaskBinding[];
  work_order_id: string;
  work_order_revision: number;
}
/**
 * This interface was referenced by `RecordIntakeReceipt`'s JSON-Schema
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
 * This interface was referenced by `RecordIntakeReceipt`'s JSON-Schema
 * via the `definition` "SourceSpan".
 */
export interface SourceSpan {
  end: number;
  start: number;
}
