/**
 * Generated from schemas/provenance-contribution.schema.json by json-schema-to-typescript@16.0.0.
 * Source SHA-256: d25cf18286258b6254db595cb75d695eeaf4ae681600b9a31943fe1d21dcdc5a
 * Do not edit. Rust domain validation still applies at runtime.
 */

/**
 * This interface was referenced by `ProvenanceContribution`'s JSON-Schema
 * via the `definition` "ProvenanceOrigin".
 */
export type ProvenanceOrigin = 'defaults' | 'user' | 'project' | 'runtime' | 'private';

/**
 * Ordered contributors remain available even when a later public value wins.
 */
export interface ProvenanceContribution {
  origin: ProvenanceOrigin;
  /**
   * Private policy has no public source identity and is never relabeled Runtime.
   */
  source_ref?: string | null;
  [k: string]: unknown;
}
