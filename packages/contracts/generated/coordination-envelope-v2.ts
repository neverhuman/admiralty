/**
 * Generated from schemas/coordination-envelope-v2.schema.json by json-schema-to-typescript@16.0.0.
 * Source SHA-256: b655ea99dceef8a099e3eeaef143140c857a5d191a849e02e19f820b0d1ef4af
 * Do not edit. Rust domain validation still applies at runtime.
 */

/**
 * This interface was referenced by `CoordinationEnvelopeV2`'s JSON-Schema
 * via the `definition` "CoordinationAuthorityGeneration".
 */
export type CoordinationAuthorityGeneration = DecimalU64;
/**
 * This interface was referenced by `CoordinationEnvelopeV2`'s JSON-Schema
 * via the `definition` "DecimalU64".
 */
export type DecimalU64 = string;
/**
 * This interface was referenced by `CoordinationEnvelopeV2`'s JSON-Schema
 * via the `definition` "CoordinationSha256Digest".
 */
export type CoordinationSha256Digest = string;
/**
 * This interface was referenced by `CoordinationEnvelopeV2`'s JSON-Schema
 * via the `definition` "ContextEpoch".
 */
export type ContextEpoch = string;
/**
 * This interface was referenced by `CoordinationEnvelopeV2`'s JSON-Schema
 * via the `definition` "CorrelationId".
 */
export type CorrelationId = string;
/**
 * This interface was referenced by `CoordinationEnvelopeV2`'s JSON-Schema
 * via the `definition` "CoordinationKind".
 */
export type CoordinationKind =
  | 'task_assigned'
  | 'task_changed'
  | 'progress_changed'
  | 'artifact_ready'
  | 'interface_proposed'
  | 'interface_accepted'
  | 'question_asked'
  | 'question_answered'
  | 'decision_requested'
  | 'decision_recorded'
  | 'verification_failed'
  | 'handoff_ready'
  | 'claim_conflict'
  | 'budget_warning'
  | 'cancel_requested'
  | 'snapshot_required';
/**
 * Queue classification only. Even `Control` confers no action capability.
 *
 * This interface was referenced by `CoordinationEnvelopeV2`'s JSON-Schema
 * via the `definition` "Priority".
 */
export type Priority = 'control' | 'decision' | 'usage' | 'work' | 'progress';
/**
 * This interface was referenced by `CoordinationEnvelopeV2`'s JSON-Schema
 * via the `definition` "CoordinationProtocolVersion2".
 */
export type CoordinationProtocolVersion2 = 2;
/**
 * This component resolves only a concrete principal, never an implicit group.
 *
 * This interface was referenced by `CoordinationEnvelopeV2`'s JSON-Schema
 * via the `definition` "RecipientScope".
 */
export type RecipientScope = {
  kind: 'principal';
  principal_id: string;
};
/**
 * This interface was referenced by `CoordinationEnvelopeV2`'s JSON-Schema
 * via the `definition` "TopicId".
 */
export type TopicId = string;

export interface CoordinationEnvelopeV2 {
  authority: AuthorityRef;
  baseline_digest: CoordinationSha256Digest | null;
  causation_id: string | null;
  context_epoch: ContextEpoch | null;
  correlation_id: CorrelationId;
  /**
   * Unix milliseconds, supplied as a canonical decimal string, never a float.
   */
  expires_at_unix_ms: DecimalU64 | null;
  kind: CoordinationKind;
  message_id: string;
  payload_ref: PayloadRef;
  priority: Priority;
  project_id: string;
  protected_facts_digest: CoordinationSha256Digest;
  protocol_version: CoordinationProtocolVersion2;
  recipient: RecipientScope;
  root_id: string;
  sender: string;
  source_cursor: Cursor;
  task_revision: TaskRevisionRef | null;
  topic: TopicId;
}
/**
 * A claimed reference to externally checked policy; JSON cannot install policy.
 *
 * This interface was referenced by `CoordinationEnvelopeV2`'s JSON-Schema
 * via the `definition` "AuthorityRef".
 */
export interface AuthorityRef {
  generation: CoordinationAuthorityGeneration;
  policy_digest: CoordinationSha256Digest;
  reference: string;
  scope_digest: CoordinationSha256Digest;
}
/**
 * This interface was referenced by `CoordinationEnvelopeV2`'s JSON-Schema
 * via the `definition` "PayloadRef".
 */
export interface PayloadRef {
  sha256: CoordinationSha256Digest;
  size_bytes: DecimalU64;
}
/**
 * Versioned projection watermark; this is deliberately not a legacy cursor alias.
 *
 * This interface was referenced by `CoordinationEnvelopeV2`'s JSON-Schema
 * via the `definition` "Cursor".
 */
export interface Cursor {
  authorization_scope_digest: CoordinationSha256Digest;
  event_seq: DecimalU64;
  projection_schema: number;
  recovery_generation: DecimalU64;
}
/**
 * This interface was referenced by `CoordinationEnvelopeV2`'s JSON-Schema
 * via the `definition` "TaskRevisionRef".
 */
export interface TaskRevisionRef {
  contract_digest: CoordinationSha256Digest;
  revision: number;
  task_id: string;
}
