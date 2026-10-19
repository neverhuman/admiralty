/**
 * Generated from schemas/work-order-revision.schema.json by json-schema-to-typescript@16.0.0.
 * Source SHA-256: 488e7432b5943d7b1ab8ed128b8446f56484dc2542ff486d33eb63e0722c2cf7
 * Do not edit. Rust domain validation still applies at runtime.
 */

/**
 * This interface was referenced by `WorkOrderRevision`'s JSON-Schema
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
 * This interface was referenced by `WorkOrderRevision`'s JSON-Schema
 * via the `definition` "PrincipalId".
 */
export type PrincipalId = string;
/**
 * This interface was referenced by `WorkOrderRevision`'s JSON-Schema
 * via the `definition` "ProjectId".
 */
export type ProjectId = string;
/**
 * This interface was referenced by `WorkOrderRevision`'s JSON-Schema
 * via the `definition` "RootId".
 */
export type RootId = string;
/**
 * Source references are claims until the Store verifies bytes and an
 * interlocked lifetime pin. Merely decoding an ArtifactClaim grants no custody.
 *
 * This interface was referenced by `WorkOrderRevision`'s JSON-Schema
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
 * This interface was referenced by `WorkOrderRevision`'s JSON-Schema
 * via the `definition` "IntakeFormat".
 */
export type IntakeFormat = 'plain_text' | 'json_batch';

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
 * This interface was referenced by `WorkOrderRevision`'s JSON-Schema
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
 * This interface was referenced by `WorkOrderRevision`'s JSON-Schema
 * via the `definition` "OriginReference".
 */
export interface OriginReference {
  record_id: string;
  revision: string;
  system: string;
}
/**
 * This interface was referenced by `WorkOrderRevision`'s JSON-Schema
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
