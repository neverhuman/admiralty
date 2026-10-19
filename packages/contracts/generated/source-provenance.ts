/**
 * Generated from schemas/source-provenance.schema.json by json-schema-to-typescript@16.0.0.
 * Source SHA-256: fdf044edf34227c4e15c89268a9ed7ef0894b611fd05eb0529263f7310511555
 * Do not edit. Rust domain validation still applies at runtime.
 */

/**
 * This interface was referenced by `SourceProvenance`'s JSON-Schema
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
 * This interface was referenced by `SourceProvenance`'s JSON-Schema
 * via the `definition` "OriginReference".
 */
export interface OriginReference {
  record_id: string;
  revision: string;
  system: string;
}
